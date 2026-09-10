# Grid Yank Sequences: `ys` / `yj` / `yq`

**Date:** 2026-09-10
**Status:** Approved for planning
**Repo:** local clone of `yelog/lazydb`
**Goal:** Unify clipboard yank under a `y` prefix in RELATION DATA and SQL Results / Data grids: `ys` copies the selected cell, `yj` copies the current row as a JSON object, and `yq` (Relation only) copies the current row as an INSERT statement. Existing `Y`, `Space Y`, and Relation `yy` stay unchanged.

## Problem

Grid copy today is inconsistent and incomplete:

- SQL Results copies a cell with a bare `y`; Relation Data has no cell-copy key and uses `y` only as the prefix for internal `yy` yank.
- Row clipboard export is TSV only (`Y` / `Space Y`). There is no path to JSON or INSERT SQL.
- Users who want a pasteable INSERT or a single-object JSON row must reconstruct values by hand.

## Success criteria

- In **SQL Results / Data** and **Relation Data** (browse, with a loaded row):
  - `ys` copies the selected cell via the same payload as today's `CopyGridCell` (`CellValue::clipboard_text()`; NULL → empty string).
  - `yj` copies the current row as one compact JSON object: `{"col": value, ...}`.
- In **Relation Data** only:
  - `yq` copies `INSERT INTO <qualified> (<cols>) VALUES (<vals>);` using the relation's qualified name and the active connection dialect.
  - `yy` continues to internal-yank the current row for `p` paste.
- In **SQL Results**, bare `y` no longer copies immediately; it starts the shared yank prefix (same model as Relation).
- **Dashboard Processes** keeps today's immediate `y` (cell) and `Y` (TSV). It is out of product scope for `ys`/`yj`/`yq`; the keymap change must carve it out of the SQL Results pending-`y` path so Processes does not silently lose bare `y`.
- `Y` and Application `Space Y` (TSV / TSV with headers) remain unchanged in SQL Results and Relation Data.
- `yq` is not bound in SQL Results (no unique writable table for arbitrary result sets).
- Empty / missing data yields the existing clipboard warning and does not write the clipboard.
- Help palette rows and `docs/keybindings.md` document the new sequences; keymap and clipboard unit tests cover them; shortcut catalog entries are updated where the project requires shared IDs.

## Non-goals

- INSERT SQL for SQL Results (user chose Relation-only `yq`).
- Moving Dashboard Processes onto `ys`/`yj`/`yq` (keep its immediate `y`/`Y`).
- Multi-row JSON arrays, pretty-printed JSON, or a format-picker overlay.
- Changing Record View mouse copy buttons or text-detail copy.
- Removing or rebinding `Y`, `Space Y`, or Relation `yy`.
- Operator menus or configurable key chords beyond the shared shortcut catalog pattern already used for results-copy-\*.

## Decisions (from brainstorming)

1. **`yq` scope:** Relation Data only; SQL Results gets `ys` / `yj` only.
2. **Bare `y`:** Becomes a pending prefix in both grids; cell copy moves to `ys`.
3. **JSON shape:** Single object (not a one-element array), compact one line.
4. **Existing shortcuts:** Keep `Y`, `Space Y`, and `yy`.

## Approach

Extend the existing clipboard helpers and pending-key machinery rather than introducing a format menu or collapsing every copy variant into one mega-Action.

- Pure formatters live in `src/clipboard.rs` next to `copy_cell` / `copy_row_tsv`.
- A shared `Pending::GridYank` replaces Relation-only yank pending and covers SQL Results.
- Thin Actions mirror `CopyGridCell` / `CopyGridRow`: `CopyGridCell` (from `ys`), `CopyGridRowJson`, `CopyGridRowInsertSql`.
- App handlers resolve `active_record_snapshot()` (and for INSERT, relation descriptor + dialect) then emit `Command::WriteClipboard`.

## Format rules

### Cell (`ys`)

Identical to current `copy_cell`: description `cell {label}` (or NULL note); text from `clipboard_text()`.

### JSON (`yj`)

- One object; keys are column names in grid order.
- Value mapping:

  | `CellValue` | JSON |
  |---|---|
  | `Null` | `null` |
  | `Boolean` | `true` / `false` |
  | `Integer` / `Unsigned` / finite `Float` | JSON number |
  | non-finite `Float` | JSON string of `Display` text (avoid invalid JSON numbers) |
  | `Text` / date-time variants / `Bytes` (`0x…` via `clipboard_text`) / `Unsupported` preview | JSON string |

- Compact serialization (no pretty-print). Duplicate column names overwrite earlier keys if they occur (same as typical object builders); Relation columns are unique by definition.

### INSERT SQL (`yq`, Relation only)

- Shape: `INSERT INTO <qualified_table> (<col>, …) VALUES (<val>, …);`
- Identifiers: existing `quote_identifier` for the active `SqlDialect` (Postgres/SQLite `"…"`, MySQL `` `…` ``, SQL Server `[…]`).
- Table: render `QualifiedName` segments that are present (database / schema / object) each quoted and joined with `.`.
- Literals (aligned with filter-clause conventions where practical):

  | `CellValue` | Literal |
  |---|---|
  | `Null` | `NULL` |
  | `Boolean` | Postgres: `TRUE`/`FALSE`; MySQL/SQLite/SQL Server: `1`/`0` |
  | `Integer` / `Unsigned` / finite `Float` | unquoted number |
  | non-finite `Float` | quoted `clipboard_text()` string |
  | `Text` / date-time | `'…'` with `'` → `''`; MySQL also escapes `\` → `\\` |
  | `Bytes` | Reuse the same dialect table as `bytes_literal` in `src/sql/relation_filter.rs`: MySQL `X'DEADBEEF'`; SQL Server `0xDEADBEEF`; SQLite/`Generic` `x'DEADBEEF'`; Postgres `'\xDEADBEEF'::bytea` |
  | `Unsupported` | quoted preview string |

- Boolean / string / bytes literals must call or share the existing `string_literal` / `bytes_literal` helpers (export or move them to a shared SQL-literal module if needed) so INSERT copy cannot drift from `cell_where_clause`.
- `SqlDialect::Generic` follows the SQLite branch for identifiers and literals (same as `bytes_literal` / filter helpers today).
- Column list and values follow the current grid row (including Relation edit-session current values when present), same snapshot path as TSV copy.
- `ClipboardPayload.description` shapes (parallel to existing helpers): `row: {n} columns as JSON`, `row: INSERT INTO … ({n} columns)`.

## Changes

1. **`src/clipboard.rs`**
   - `copy_row_json(columns, row) -> Option<ClipboardPayload>`
   - `copy_row_insert_sql(dialect, qualified_name, columns, row) -> Option<ClipboardPayload>`
   - Unit tests for NULL, quote escaping, bytes per dialect, and JSON number/string boundaries.

2. **`src/action.rs`**
   - Keep `CopyGridCell`.
   - Add `CopyGridRowJson`.
   - Add `CopyGridRowInsertSql` (no payload; handler resolves relation context or warns).

3. **`src/input/keymap.rs`**
   - Introduce `Pending::GridYank` (or rename/generalize `RelationYank` to the shared name).
   - Split today's combined `(is_sql_grid_focus || is_read_only_grid_focus)` bare-`y` binding:
     - **SQL Results Data** (`is_sql_grid_focus`): `y` sets `Pending::GridYank` (no immediate copy).
     - **Dashboard Processes** (`is_read_only_grid_focus` and not SQL): keep immediate `y` → `CopyGridCell` and `Y` → TSV.
   - Relation browse: `y` sets the same `Pending::GridYank` (replacing `Pending::RelationYank`).
   - Continuations on `GridYank`: `s` → `CopyGridCell`, `j` → `CopyGridRowJson`; `q` → `CopyGridRowInsertSql` and `y` → `RelationYank` only when `relation_grid_is_browse(app)`.
   - Invalid second keys clear pending without side effects (existing pending behavior).
   - `Y` / leader `Space Y` unchanged on SQL Results and Relation.

4. **`src/app.rs`**
   - `CopyGridRowJson` → `copy_row_json` + `WriteClipboard` (sensitive flag via `active_process_grid()` like other grid copies).
   - `CopyGridRowInsertSql` → only on Relation Data with descriptor + dialect; otherwise `notify_warning`; on success write INSERT payload.
   - Help ID dispatch for the new shortcut rows.

5. **`src/help.rs`**, **`docs/keybindings.md`**, shortcut catalog / `config` aliases
   - Document `ys`, `yj` for both SqlResultsData and RelationDataBrowse.
   - Document `yq` for RelationDataBrowse only.
   - Update the SQL Results note that bare `y` is no longer immediate cell copy; Relation note that `ys` now covers cell copy.
   - Add/adjust shared shortcut IDs (e.g. `results-copy-cell` may map to `ys` display; add IDs for JSON / INSERT as needed for Help and configurable bindings consistency).

6. **Tests**
   - `src/clipboard.rs` unit tests for formatters.
   - `tests/keymap.rs` for pending sequences in both contexts, including: SQL Results bare `y` does not emit `CopyGridCell`; `ys`/`yj` do; Relation `yq`/`yy` still work; SQL Results `yq` does not fire.
   - Optional thin app test if clipboard command wiring needs regression coverage beyond keymap.

## Error handling

| Situation | Behavior |
|---|---|
| No active Data snapshot / empty columns | Warning; no clipboard write (same as today) |
| `yq` outside Relation browse | Binding absent; if Action somehow dispatched, warning |
| Pending `y` then unrelated key / Esc | Clear pending; no copy |
| Process grid (Dashboard) sensitive rows | Preserve `sensitive: true` on payloads when `active_process_grid()` |

## Testing plan

1. Clipboard pure tests prove JSON and INSERT strings for representative `CellValue`s and dialects.
2. Keymap tests prove sequence mapping and negative cases (`yq` in SQL Results, bare `y` no longer immediate).
3. Manual smoke in a real terminal (clipboard delivery is not fully proven by automated tests per existing docs note): Relation `ys`/`yj`/`yq`, SQL Results `ys`/`yj`, and regression on `Y` / `yy`.

## Open implementation notes (non-blocking)

- Prefer renaming `Pending::RelationYank` → `Pending::GridYank` in the same change if call sites are few; keep Help prefix labeling clear for `yy` vs new sequences.
- Reuse `quote_identifier` from `src/sql/completion.rs` (already used by relation filter) rather than duplicating quote logic.
- Prefer exporting or relocating `string_literal` / `bytes_literal` from `relation_filter.rs` over re-implementing them inside `clipboard.rs`.
