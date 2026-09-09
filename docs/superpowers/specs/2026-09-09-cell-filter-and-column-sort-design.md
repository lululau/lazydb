# One-key WHERE Filter (`F`) and Column Sort Cycle (`O`)

**Date:** 2026-09-09
**Status:** Approved for planning
**Repo:** local clone of `yelog/lazydb`
**Goal:** In the RELATION DATA and SQL results grids, `F` filters rows by the selected cell's value (`col = value` / `col IS NULL`), and `O` cycles the selected column through desc → asc → no sort. Both fill the visible query-bar inputs and resubmit immediately.

## Problem

Today the data query bar is the only keyboard path to filtering: `/` focuses the WHERE input and `s` focuses ORDER BY, but the user must type the column name, operator, and literal by hand. The mouse can already click a column header to cycle sorting (`Action::CycleDataColumnSort`, `src/app.rs:8262`), but there is no keyboard equivalent, and no way at all — mouse or keyboard — to filter by the value under the cursor.

## Success criteria

- In a RELATION DATA tab (Data view, loaded) or a SQL results tab (successful execution, Data view), pressing `F` on any cell:
  - builds `«col» = literal` for the cell's column and value (`«col» IS NULL` when the cell is NULL),
  - **replaces** the WHERE input content (visible in the query bar, editable later via `/`),
  - resubmits the query immediately.
- Pressing `O` on any column cycles that column's sort `none → DESC → ASC → none`, fills the ORDER BY input, and resubmits — identical sequencing to mouse header clicks (`cycle_relation_column_sort`, `src/sql/relation_filter.rs:55`).
- Both keys work in both panel types; other existing keys (including `o` "switch Data/Output" in the SQL tab) are unchanged.
- Failure states give a visible notification instead of silently doing nothing.
- Generated clauses still pass `validate_relation_preview_options` before submission (existing injection guard).
- Help palette rows and `docs/keybindings.md` list both keys for both contexts; keymap and app tests cover them.

## Non-goals

- Operator menus (choose `!=` / `LIKE` / `>` after the keypress) — the generated clause is always `=`; users edit via `/`.
- `AND`-stacking onto an existing WHERE clause — `F` replaces.
- Configurable `config/default.toml` entries for `F`/`O` — they are hardcoded like the closest precedents `/` and `s` (`src/input/keymap.rs:3188-3200`).
- Making the SQL-tab ambiguous-column path of the mouse sort notify — mouse behavior stays as-is; only the new keyboard path notifies.
- Changing `o`/`O` pairing concerns (SQL-tab `o` = switch Data/Output stays).

## Approach

Follow the established `CycleDataColumnSort` pattern: a pure function in `src/sql/relation_filter.rs` builds the clause; a thin `Action` handler resolves the selected cell/column, writes the clause into the query-bar `TextInput`, and delegates to `Action::SubmitDataQuery`. A shared helper extracts the tab context both new handlers need. Keyboard failure paths notify (`notify_warning`); the mouse path is untouched.

## Changes

1. **Clause generator — `src/sql/relation_filter.rs`**
   New pure function next to `cycle_relation_column_sort`:
   ```rust
   pub fn cell_where_clause(
       column: &str,
       value: &crate::db::value::CellValue,
       dialect: SqlDialect,
   ) -> Result<String, RelationFilterError>
   ```
   Column name via existing `quote_identifier`. Value → literal rules:

   | `CellValue` | Clause |
   |---|---|
   | `Null` | `«col» IS NULL` |
   | `Boolean(b)` | PG: `= TRUE/FALSE`; MySQL/SQLite/SqlServer: `= 1/0` |
   | `Integer`/`Unsigned` | `= 42` unquoted |
   | `Float` (finite) | `= 3.14` (Rust shortest round-trip `Display`) |
   | `Float` (NaN/±inf) | `Err` — no portable literal |
   | `Text` | `= 'it''s'`; `'`→`''`, and `\`→`\\` under MySQL |
   | `Bytes` | MySQL/MSSQL `0xDEADBEEF`; SQLite `x'DEADBEEF'`; PG `'\xDEADBEEF'::bytea` |
   | `Date`/`Time`/`DateTime`/`Timestamp` | `= '…'` ISO string via `CellValue::clipboard_text()` (already database-friendly formats) |
   | `Unsupported` | `Err` — cannot rebuild a literal from a preview |

2. **Actions — `src/action.rs`**
   - `FilterGridCellByValue` — resolves the selected cell; no payload.
   - `CycleSelectedColumnSort` — resolves the selected column, delegates to existing `Action::CycleDataColumnSort(column)` (delegation style of `RelationQueryInsert` → `DataQueryInsert`, `src/app.rs:8249`).

3. **Handlers — `src/app.rs`**
   - Shared helper (near `CycleDataColumnSort`, `src/app.rs:8262`) returning the active tab's query context — column names, dialect — or a failure reason (`Loading`, `Running`, `NoSucceededExecution`, `AmbiguousColumns`). `AmbiguousColumns` applies only to the SQL tab (case-insensitive duplicate output names make the outer WHERE/ORDER BY reference ambiguous; relation columns are unique by definition) — the same check the mouse path performs today (`src/app.rs:8297-8306`). Both new handlers use the helper; `CycleDataColumnSort`'s tab matching is refactored onto it without behavior change.
   - `FilterGridCellByValue`: resolve `active_record_snapshot()` + `active_grid_column()` (pattern of `copy_grid_cell`, `src/app.rs:1135`); build clause; `where_input.set(clause)`; `update(Action::SubmitDataQuery)`. `Err` from the generator or an unresolvable cell → `notify_warning("Filter", …)`.
   - `CycleSelectedColumnSort`: map failure reasons to `notify_warning`; on success `update(Action::CycleDataColumnSort(column))`.
   - Id dispatch (near `src/app.rs:2063`): `Id::ResultsFilterCell` / `Id::ResultsSortColumn` → the new actions.

4. **Keymap — `src/input/keymap.rs`**
   In the shared `Focus::Results` branch that already binds `/` and `s` (~line 3188): `Char('F') => FilterGridCellByValue`, `Char('O') => CycleSelectedColumnSort`. Both panels reach this branch; uppercase normalization follows the existing `Y`/`P`/`V` precedent.

5. **Help palette — `src/help.rs`**
   Two rows, contexts `[SqlResultsData, RelationDataBrowse]`, requirement `DataQueryAvailable` (as `RelationWhere`/`RelationOrderBy` rows):
   - `ResultsFilterCell` — `F` — "filter rows by selected cell value"
   - `ResultsSortColumn` — `O` — "sort by selected column (desc/asc/none)"

6. **Docs — `docs/keybindings.md`**
   One row per key in the RELATION DATA and SQL results tables.

## Testing

- Unit (`relation_filter.rs`): every `CellValue` variant × dialect-relevant cases — quoting/escaping edges (embedded `'`, embedded `\`, empty bytes, NaN), boolean per dialect, temporal formats, `Unsupported`/NaN errors.
- App integration (pattern: `sql_result_header_sort_submits_a_derived_query`, `src/app.rs:18242`):
  - `F` sets `where_input` to the expected clause and submits (relation tab, and SQL tab via derived query).
  - NULL cell produces `IS NULL`; `Unsupported` cell yields a warning and no submission.
  - `O` pressed three times cycles `DESC` → `ASC` → removed in the ORDER BY input, submitting each time.
  - Failure reasons (no succeeded execution, ambiguous columns) produce notifications, not silence.
- Keymap: `F`/`O` map to the new actions under `Focus::Results`; `o` still maps to `ToggleResultView` on SQL tabs.
- Help: both rows listed under `SqlResultsData` and `RelationDataBrowse`.

## Risks

- Uppercase char events may arrive with or without `SHIFT` depending on terminal protocol; mitigated by following the existing uppercase-key handling (same exposure as `Y`/`V`/`P`).
- Text escaping differs per backend (MySQL backslashes); covered by unit tests rather than assuming ANSI-only quoting.
