# Grid Yank Sequences (`ys` / `yj` / `yq`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unify clipboard yank under a shared `y` prefix in SQL Results and Relation Data: `ys` copies the selected cell, `yj` copies the row as JSON, and Relation-only `yq` copies the row as INSERT SQL, while keeping `Y` / `Space Y` / `yy` and Dashboard Processes bare `y`/`Y`.

**Architecture:** Pure formatters in `src/clipboard.rs` (JSON + INSERT) reuse exported SQL literal helpers from `src/sql/relation_filter.rs`. Keymap uses one `Pending::GridYank` for SQL Results and Relation browse. Thin `Action` variants wire through existing `Command::WriteClipboard` in `src/app.rs`. Help/docs/catalog stay in sync.

**Tech Stack:** Rust (existing lazydb TUI), `serde_json` (already in `Cargo.toml`), existing `quote_identifier` / dialect helpers.

**Spec:** `docs/superpowers/specs/2026-09-10-grid-yank-json-insert-design.md`

**Verification commands used throughout:** `cargo test --lib <filter>` for unit tests, `cargo test --test keymap <filter>` for keymap integration, `cargo fmt --all` before commits. Run from repo root.

---

## File map

| File | Responsibility |
|---|---|
| `src/sql/relation_filter.rs` | Export `string_literal` / `bytes_literal`; add `cell_sql_literal` for INSERT value rendering |
| `src/clipboard.rs` | `copy_row_json`, `copy_row_insert_sql`, unit tests |
| `src/action.rs` | `CopyGridRowJson`, `CopyGridRowInsertSql` |
| `src/app.rs` | Handlers + Help ID dispatch |
| `src/input/keymap.rs` | `Pending::GridYank`; SQL Results pending `y`; Dashboard keeps immediate `y`; continuations |
| `src/help.rs` | Catalog rows + prefix ranks for `ys`/`yj`/`yq` |
| `src/config.rs` | Known command IDs for JSON / INSERT |
| `docs/keybindings.md`, `docs/configuration.md` | User-facing docs |
| `tests/keymap.rs` | Sequence mapping tests |

---

### Task 1: Export SQL literals + `cell_sql_literal`

**Files:**
- Modify: `src/sql/relation_filter.rs`

- [ ] **Step 1: Write the failing test for `cell_sql_literal`**

Append inside `mod tests` in `src/sql/relation_filter.rs`:

```rust
    #[test]
    fn cell_sql_literal_matches_filter_helpers_and_quotes_unfilterable_values() {
        use crate::db::value::CellValue;

        assert_eq!(
            cell_sql_literal(&CellValue::Null, SqlDialect::Postgres),
            "NULL"
        );
        assert_eq!(
            cell_sql_literal(&CellValue::Boolean(true), SqlDialect::Postgres),
            "TRUE"
        );
        assert_eq!(
            cell_sql_literal(&CellValue::Boolean(false), SqlDialect::MySql),
            "0"
        );
        assert_eq!(
            cell_sql_literal(&CellValue::Integer(42), SqlDialect::Sqlite),
            "42"
        );
        assert_eq!(
            cell_sql_literal(&CellValue::Text("it's".into()), SqlDialect::Postgres),
            "'it''s'"
        );
        assert_eq!(
            cell_sql_literal(&CellValue::Bytes(vec![0xDE, 0xAD]), SqlDialect::MySql),
            "X'DEAD'"
        );
        assert_eq!(
            cell_sql_literal(&CellValue::Bytes(vec![0xDE, 0xAD]), SqlDialect::SqlServer),
            "0xDEAD"
        );
        assert_eq!(
            cell_sql_literal(&CellValue::Bytes(vec![0xDE, 0xAD]), SqlDialect::Postgres),
            "'\\xDEAD'::bytea"
        );
        assert_eq!(
            cell_sql_literal(&CellValue::Bytes(vec![0xDE, 0xAD]), SqlDialect::Generic),
            "x'DEAD'"
        );
        assert_eq!(
            cell_sql_literal(&CellValue::Float(f64::NAN), SqlDialect::Postgres),
            format!("'{}'", CellValue::Float(f64::NAN).clipboard_text())
        );
        assert_eq!(
            cell_sql_literal(
                &CellValue::Unsupported {
                    type_name: "xml".into(),
                    preview: "<a/>".into(),
                },
                SqlDialect::Postgres
            ),
            "'<a/>'"
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib cell_sql_literal_matches_filter_helpers -- --nocapture`  
Expected: compile error (`cell_sql_literal` not found) or FAIL.

- [ ] **Step 3: Implement minimal helpers**

Change `string_literal` and `bytes_literal` from `fn` to `pub(crate) fn`.

Add after `bytes_literal`:

```rust
pub(crate) fn cell_sql_literal(value: &CellValue, dialect: SqlDialect) -> String {
    match value {
        CellValue::Null => "NULL".into(),
        CellValue::Boolean(inner) => match dialect {
            SqlDialect::Postgres => if *inner { "TRUE" } else { "FALSE" }.into(),
            _ => u8::from(*inner).to_string(),
        },
        CellValue::Integer(_) | CellValue::Unsigned(_) => value.clipboard_text(),
        CellValue::Float(inner) if inner.is_finite() => value.clipboard_text(),
        CellValue::Float(_) => string_literal(&value.clipboard_text(), dialect),
        CellValue::Text(inner) => string_literal(inner, dialect),
        CellValue::Bytes(inner) => bytes_literal(inner, dialect),
        CellValue::Date(_)
        | CellValue::Time(_)
        | CellValue::DateTime(_)
        | CellValue::Timestamp(_) => string_literal(&value.clipboard_text(), dialect),
        CellValue::Unsupported { preview, .. } => string_literal(preview, dialect),
    }
}
```

Keep `cell_where_clause` calling the now-`pub(crate)` helpers unchanged.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib cell_sql_literal_matches_filter_helpers -- --nocapture`  
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/sql/relation_filter.rs
git commit -m "feat(sql): add cell_sql_literal for INSERT clipboard copy"
```

---

### Task 2: `copy_row_json`

**Files:**
- Modify: `src/clipboard.rs`

- [ ] **Step 1: Write the failing JSON tests**

In `src/clipboard.rs` `mod tests`, update the import line to include `copy_row_json`, then append:

```rust
    #[test]
    fn json_copies_one_object_with_typed_values() {
        let columns = vec![
            ColumnMeta {
                name: "id".into(),
                type_name: "INT".into(),
            },
            ColumnMeta {
                name: "name".into(),
                type_name: "TEXT".into(),
            },
            ColumnMeta {
                name: "note".into(),
                type_name: "TEXT".into(),
            },
            ColumnMeta {
                name: "flag".into(),
                type_name: "BOOL".into(),
            },
        ];
        let row = vec![
            CellValue::Integer(1),
            CellValue::Text("Ada".into()),
            CellValue::Null,
            CellValue::Boolean(true),
        ];
        let payload = copy_row_json(&columns, &row).unwrap();
        assert_eq!(payload.text, r#"{"id":1,"name":"Ada","note":null,"flag":true}"#);
        assert_eq!(payload.description, "row: 4 columns as JSON");
        assert!(!payload.sensitive);
    }

    #[test]
    fn json_returns_none_for_empty_columns() {
        assert!(copy_row_json(&[], &[]).is_none());
    }

    #[test]
    fn json_stringifies_non_finite_floats_and_bytes() {
        let columns = vec![
            ColumnMeta {
                name: "n".into(),
                type_name: "FLOAT".into(),
            },
            ColumnMeta {
                name: "blob".into(),
                type_name: "BYTEA".into(),
            },
        ];
        let row = vec![
            CellValue::Float(f64::INFINITY),
            CellValue::Bytes(vec![0x01, 0xFF]),
        ];
        let payload = copy_row_json(&columns, &row).unwrap();
        let value: serde_json::Value = serde_json::from_str(&payload.text).unwrap();
        assert_eq!(value["n"], serde_json::Value::String("inf".into()));
        assert_eq!(value["blob"], serde_json::Value::String("0x01FF".into()));
    }
```

Note: assert the exact `clipboard_text()` for infinity if Display differs (`inf` vs `Infinity`); prefer parsing JSON and comparing the string form from `CellValue::Float(f64::INFINITY).clipboard_text()`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib json_copies_one_object -- --nocapture`  
Expected: compile error (`copy_row_json` not found).

- [ ] **Step 3: Implement `copy_row_json`**

Add after `copy_row_tsv` in `src/clipboard.rs`:

```rust
pub fn copy_row_json(
    columns: &[ColumnMeta],
    row: &[CellValue],
) -> Option<ClipboardPayload> {
    if columns.is_empty() {
        return None;
    }
    let mut map = serde_json::Map::new();
    for (index, column) in columns.iter().enumerate() {
        let value = row.get(index).unwrap_or(&CellValue::Null);
        map.insert(column.name.clone(), cell_json_value(value));
    }
    Some(ClipboardPayload {
        text: serde_json::Value::Object(map).to_string(),
        description: format!("row: {} columns as JSON", columns.len()),
        sensitive: false,
    })
}

fn cell_json_value(value: &CellValue) -> serde_json::Value {
    match value {
        CellValue::Null => serde_json::Value::Null,
        CellValue::Boolean(inner) => serde_json::Value::Bool(*inner),
        CellValue::Integer(inner) => serde_json::json!(*inner),
        CellValue::Unsigned(inner) => serde_json::json!(*inner),
        CellValue::Float(inner) if inner.is_finite() => serde_json::json!(*inner),
        other => serde_json::Value::String(other.clipboard_text()),
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib json_ -- --nocapture`  
Expected: PASS (existing clipboard tests still pass).

- [ ] **Step 5: Commit**

```bash
git add src/clipboard.rs
git commit -m "feat(clipboard): copy grid row as JSON object"
```

---

### Task 3: `copy_row_insert_sql`

**Files:**
- Modify: `src/clipboard.rs`

- [ ] **Step 1: Write the failing INSERT tests**

```rust
    #[test]
    fn insert_sql_quotes_identifiers_and_literals_per_dialect() {
        use crate::db::catalog::QualifiedName;
        use crate::sql::SqlDialect;

        let columns = vec![
            ColumnMeta {
                name: "id".into(),
                type_name: "INT".into(),
            },
            ColumnMeta {
                name: "name".into(),
                type_name: "TEXT".into(),
            },
            ColumnMeta {
                name: "note".into(),
                type_name: "TEXT".into(),
            },
        ];
        let row = vec![
            CellValue::Integer(1),
            CellValue::Text("O'Hara".into()),
            CellValue::Null,
        ];
        let name = QualifiedName {
            database: None,
            schema: Some("public".into()),
            object: "users".into(),
        };
        let payload =
            copy_row_insert_sql(SqlDialect::Postgres, &name, &columns, &row).unwrap();
        assert_eq!(
            payload.text,
            r#"INSERT INTO "public"."users" ("id", "name", "note") VALUES (1, 'O''Hara', NULL);"#
        );
        assert!(payload.description.contains("INSERT INTO"));
        assert!(payload.description.contains("3 columns"));

        let mysql = copy_row_insert_sql(
            SqlDialect::MySql,
            &QualifiedName {
                database: Some("app".into()),
                schema: None,
                object: "users".into(),
            },
            &columns,
            &row,
        )
        .unwrap();
        assert_eq!(
            mysql.text,
            "INSERT INTO `app`.`users` (`id`, `name`, `note`) VALUES (1, 'O''Hara', NULL);"
        );
    }

    #[test]
    fn insert_sql_returns_none_for_empty_columns() {
        use crate::db::catalog::QualifiedName;
        use crate::sql::SqlDialect;
        assert!(copy_row_insert_sql(
            SqlDialect::Sqlite,
            &QualifiedName {
                database: None,
                schema: None,
                object: "t".into(),
            },
            &[],
            &[]
        )
        .is_none());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib insert_sql_quotes_identifiers -- --nocapture`  
Expected: compile error.

- [ ] **Step 3: Implement `copy_row_insert_sql`**

Add imports at top of `src/clipboard.rs`:

```rust
use crate::db::catalog::QualifiedName;
use crate::sql::SqlDialect;
use crate::sql::completion::quote_identifier;
use crate::sql::relation_filter::cell_sql_literal;
```

(Adjust paths to match how `sql` modules are re-exported from `crate::sql` — prefer existing public re-exports; if `relation_filter::cell_sql_literal` is `pub(crate)`, `clipboard` in the same crate can import it.)

Implement:

```rust
pub fn copy_row_insert_sql(
    dialect: SqlDialect,
    qualified_name: &QualifiedName,
    columns: &[ColumnMeta],
    row: &[CellValue],
) -> Option<ClipboardPayload> {
    if columns.is_empty() {
        return None;
    }
    let table = quote_qualified_name(qualified_name, dialect);
    let cols = columns
        .iter()
        .map(|column| quote_identifier(&column.name, dialect))
        .collect::<Vec<_>>()
        .join(", ");
    let values = columns
        .iter()
        .enumerate()
        .map(|(index, _)| {
            cell_sql_literal(row.get(index).unwrap_or(&CellValue::Null), dialect)
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(ClipboardPayload {
        text: format!("INSERT INTO {table} ({cols}) VALUES ({values});"),
        description: format!("row: INSERT INTO {table} ({} columns)", columns.len()),
        sensitive: false,
    })
}

fn quote_qualified_name(name: &QualifiedName, dialect: SqlDialect) -> String {
    let mut parts = Vec::new();
    if let Some(database) = &name.database {
        parts.push(quote_identifier(database, dialect));
    }
    if let Some(schema) = &name.schema {
        parts.push(quote_identifier(schema, dialect));
    }
    parts.push(quote_identifier(&name.object, dialect));
    parts.join(".")
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib insert_sql_ -- --nocapture`  
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/clipboard.rs
git commit -m "feat(clipboard): copy relation row as INSERT SQL"
```

---

### Task 4: Actions + app handlers

**Files:**
- Modify: `src/action.rs`
- Modify: `src/app.rs`

- [ ] **Step 1: Add Action variants**

In `src/action.rs` next to `CopyGridRow`:

```rust
    CopyGridRowJson,
    CopyGridRowInsertSql,
```

- [ ] **Step 2: Wire handlers in `src/app.rs`**

Update the import near line 14:

```rust
    clipboard::{ClipboardPayload, copy_cell, copy_row_insert_sql, copy_row_json, copy_row_tsv},
```

Extend the `Action::CopyGridCell` / `CopyGridRow` match arm (~7280):

```rust
            Action::CopyGridCell => self.copy_grid_cell(),
            Action::CopyGridRow { include_headers } => self.copy_grid_row(include_headers),
            Action::CopyGridRowJson => self.copy_grid_row_json(),
            Action::CopyGridRowInsertSql => self.copy_grid_row_insert_sql(),
```

Also include the new actions in any exhaustive lists that already mention `CopyGridCell` / `CopyGridRow` (Help executable allow-lists ~2160).

Add methods next to `copy_grid_row`:

```rust
    fn copy_grid_row_json(&mut self) -> Vec<Command> {
        let Some((columns, row, _, _)) = self.active_record_snapshot() else {
            self.notify_warning("Clipboard", "Nothing to copy in the current Data view");
            return Vec::new();
        };
        copy_row_json(&columns, &row)
            .map(|mut payload| {
                payload.sensitive = self.active_process_grid();
                Command::WriteClipboard(payload)
            })
            .into_iter()
            .collect()
    }

    fn copy_grid_row_insert_sql(&mut self) -> Vec<Command> {
        let Some(crate::model::tab::WorkspaceTab::Relation(tab)) = self.tabs.get(self.active_tab)
        else {
            self.notify_warning("Clipboard", "INSERT SQL copy is only available in Relation Data");
            return Vec::new();
        };
        if tab.view != crate::model::relation::RelationView::Data {
            self.notify_warning("Clipboard", "INSERT SQL copy is only available in Relation Data");
            return Vec::new();
        }
        let qualified_name = tab.descriptor.qualified_name.clone();
        let dialect = self.sql_dialect();
        let Some((columns, row, _, _)) = self.active_record_snapshot() else {
            self.notify_warning("Clipboard", "Nothing to copy in the current Data view");
            return Vec::new();
        };
        copy_row_insert_sql(dialect, &qualified_name, &columns, &row)
            .map(|mut payload| {
                payload.sensitive = self.active_process_grid();
                Command::WriteClipboard(payload)
            })
            .into_iter()
            .collect()
    }
```

- [ ] **Step 3: Compile check**

Run: `cargo test --lib --no-run`  
Expected: success (warnings OK).

- [ ] **Step 4: Commit**

```bash
git add src/action.rs src/app.rs
git commit -m "feat(app): handle JSON and INSERT grid copy actions"
```

---

### Task 5: Keymap — shared `GridYank` pending

**Files:**
- Modify: `src/input/keymap.rs`
- Modify: `src/help.rs` (only if `ShortcutPrefix::RelationYank` rename requires it in the same commit — prefer renaming pending + prefix together in Task 5/6; this task focuses on keymap behavior)

- [ ] **Step 1: Write failing keymap tests first**

In `tests/keymap.rs`, add (near `relation_browse_yy_maps_the_catalog_yank_row_binding` and `shifted_y_copies_sql_result_rows`):

```rust
#[test]
fn sql_results_ys_and_yj_use_grid_yank_prefix() {
    let mut app = App::new(Vec::new());
    app.focus = Focus::Results;
    let mut keymap = Keymap::default();

    assert_eq!(keymap.map(key(KeyCode::Char('y')), &app), None);
    assert_eq!(
        keymap.map(key(KeyCode::Char('s')), &app),
        Some(Action::CopyGridCell)
    );

    assert_eq!(keymap.map(key(KeyCode::Char('y')), &app), None);
    assert_eq!(
        keymap.map(key(KeyCode::Char('j')), &app),
        Some(Action::CopyGridRowJson)
    );

    assert_eq!(keymap.map(key(KeyCode::Char('y')), &app), None);
    assert_eq!(keymap.map(key(KeyCode::Char('q')), &app), None);
}

#[test]
fn relation_browse_ys_yj_yq_and_yy_share_grid_yank_prefix() {
    let mut app = App::new(Vec::new());
    app.tabs
        .push(WorkspaceTab::Relation(RelationTab::new("users")));
    app.active_tab = 1;
    app.focus = Focus::Results;
    let mut keymap = Keymap::default();

    assert_eq!(keymap.map(key(KeyCode::Char('y')), &app), None);
    assert_eq!(
        keymap.map(key(KeyCode::Char('s')), &app),
        Some(Action::CopyGridCell)
    );

    assert_eq!(keymap.map(key(KeyCode::Char('y')), &app), None);
    assert_eq!(
        keymap.map(key(KeyCode::Char('j')), &app),
        Some(Action::CopyGridRowJson)
    );

    assert_eq!(keymap.map(key(KeyCode::Char('y')), &app), None);
    assert_eq!(
        keymap.map(key(KeyCode::Char('q')), &app),
        Some(Action::CopyGridRowInsertSql)
    );

    assert_eq!(keymap.map(key(KeyCode::Char('y')), &app), None);
    assert_eq!(
        keymap.map(key(KeyCode::Char('y')), &app),
        Some(Action::RelationYank)
    );
}

#[test]
fn dashboard_processes_keeps_immediate_y_cell_copy() {
    let mut app = App::new(Vec::new());
    app.tabs.clear();
    let mut dashboard = lazydb::model::dashboard::DashboardTab::new();
    dashboard.page = lazydb::model::dashboard::DashboardPage::Processes;
    app.tabs.push(WorkspaceTab::Dashboard(dashboard));
    app.active_tab = 0;
    app.focus = Focus::Results;
    let mut keymap = Keymap::default();

    assert_eq!(
        keymap.map(key(KeyCode::Char('y')), &app),
        Some(Action::CopyGridCell)
    );
    assert_eq!(
        keymap.map(
            KeyEvent::new(KeyCode::Char('Y'), KeyModifiers::SHIFT),
            &app
        ),
        Some(Action::CopyGridRow {
            include_headers: false,
        })
    );
}
```

Update `relation_browse_yy_maps_the_catalog_yank_row_binding` if needed (behavior should still pass).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test keymap sql_results_ys_and_yj -- --nocapture`  
Expected: FAIL (bare `y` still returns `CopyGridCell` on SQL Results).

- [ ] **Step 3: Implement keymap changes**

1. Rename `Pending::RelationYank` → `Pending::GridYank` everywhere in `src/input/keymap.rs`.
2. Replace the SQL/read-only block (~1198–1210) with a split:

```rust
        if is_sql_grid_focus(app)
            && (event.modifiers.is_empty() || event.modifiers == KeyModifiers::SHIFT)
        {
            match event.code {
                KeyCode::Char('y') => {
                    self.set_pending(Pending::GridYank, app);
                    return None;
                }
                KeyCode::Char('Y') => {
                    return Some(Action::CopyGridRow {
                        include_headers: false,
                    });
                }
                _ => {}
            }
        }

        if is_read_only_grid_focus(app)
            && (event.modifiers.is_empty() || event.modifiers == KeyModifiers::SHIFT)
        {
            match event.code {
                KeyCode::Char('y') => return Some(Action::CopyGridCell),
                KeyCode::Char('Y') => {
                    return Some(Action::CopyGridRow {
                        include_headers: false,
                    });
                }
                _ => {}
            }
        }
```

3. In Relation browse, change `set_pending(Pending::RelationYank` → `Pending::GridYank` (keep `Y` TSV).
4. Update pending hint mapping to `ShortcutPrefix::GridYank` (or keep `RelationYank` temporarily — Task 6 renames Help prefix; if Help still has `RelationYank`, map pending to that prefix until Task 6, then rename both together preferred).
5. Replace pending continuation match:

```rust
        (Pending::GridYank, KeyCode::Char('s')) => Some(Action::CopyGridCell),
        (Pending::GridYank, KeyCode::Char('j')) => Some(Action::CopyGridRowJson),
        (Pending::GridYank, KeyCode::Char('q')) if relation_grid_is_browse(app) => {
            Some(Action::CopyGridRowInsertSql)
        }
        (Pending::GridYank, KeyCode::Char('y')) if relation_grid_is_browse(app) => {
            Some(Action::RelationYank)
        }
```

6. Leave configurable `bindings.matches("results-copy-cell", event)` as an alternate single-event binding (still valid for remaps).

- [ ] **Step 4: Run keymap tests**

Run: `cargo test --test keymap sql_results_ys_and_yj relation_browse_ys_yj_yq dashboard_processes_keeps -- --nocapture`  
Expected: PASS. Also run `cargo test --test keymap relation_browse_yy -- --nocapture`.

- [ ] **Step 5: Commit**

```bash
git add src/input/keymap.rs tests/keymap.rs
git commit -m "feat(input): add ys/yj/yq grid yank sequences"
```

---

### Task 6: Help catalog + docs + config IDs

**Files:**
- Modify: `src/help.rs`
- Modify: `src/config.rs`
- Modify: `src/app.rs` (Help ID → Action dispatch)
- Modify: `docs/keybindings.md`
- Modify: `docs/configuration.md`

- [ ] **Step 1: Update Help IDs and rows**

In `HelpShortcutId` enum (near `ResultsCopyCell`):

```rust
    ResultsCopyRowJson,
    RelationCopyRowInsertSql,
```

Rename `ShortcutPrefix::RelationYank` → `ShortcutPrefix::GridYank` (display still `"y"`). Update `display`, `prefix_rank`, and all `RelationYank` prefix references (including `yy` row) to `GridYank`.

Change rows:

```rust
    row!(
        ResultsCopyCell,
        [SqlResultsData, RelationDataBrowse],
        "ys",
        "copy selected cell",
        GridYank,
        "s"
    ),
    row!(
        ResultsCopyRowJson,
        [SqlResultsData, RelationDataBrowse],
        "yj",
        "copy selected row as JSON",
        GridYank,
        "j"
    ),
    row!(
        RelationCopyRowInsertSql,
        [RelationDataBrowse],
        "yq",
        "copy selected row as INSERT SQL",
        GridYank,
        "q"
    ),
    row!(
        RelationYankRow,
        [RelationDataBrowse],
        "yy",
        "yank row",
        GridYank,
        "y"
    ),
```

Update `prefix_rank` for `GridYank` to order: `ys`/`ResultsCopyCell`, `yj`, `yq`, `yy` with stable ranks.

Map command aliases:

```rust
        HelpShortcutId::ResultsCopyCell => Some("results-copy-cell"),
        HelpShortcutId::ResultsCopyRowJson => Some("results-copy-row-json"),
        HelpShortcutId::RelationCopyRowInsertSql => Some("results-copy-row-insert-sql"),
```

In `src/config.rs` `KNOWN_COMMANDS`, add `"results-copy-row-json"` and `"results-copy-row-insert-sql"` next to the other results-copy-\* entries.

In `src/app.rs` Help execution dispatch (~2083):

```rust
            Id::ResultsCopyCell => vec![Action::CopyGridCell],
            Id::ResultsCopyRowJson => vec![Action::CopyGridRowJson],
            Id::RelationCopyRowInsertSql => vec![Action::CopyGridRowInsertSql],
```

In `src/input/keymap.rs` `configured_command_action` / bindings match, add:

```rust
        if bindings.matches("results-copy-row-json", event) {
            return Some(Action::CopyGridRowJson);
        }
        if bindings.matches("results-copy-row-insert-sql", event) {
            return Some(Action::CopyGridRowInsertSql);
        }
```

(Place beside existing `results-copy-cell` matches; INSERT binding may no-op warn via app handler outside Relation.)

- [ ] **Step 2: Update docs**

In `docs/keybindings.md`:

**SQL results table:** replace `| y | Copy selected cell |` with:

```markdown
| `ys` | Copy selected cell |
| `yj` | Copy selected row as JSON |
```

Keep `Y` / `Space Y` rows. Note that bare `y` starts the yank prefix.

**Relation Data table:** add:

```markdown
| `ys` | Copy selected cell |
| `yj` | Copy selected row as JSON |
| `yq` | Copy selected row as INSERT SQL |
```

Keep `yy` / `Y`. Update the note that Relation browse now shares `ys` cell copy with SQL Results (remove “uses `yy`/yank row, not SQL Results `y`/copy cell” wording; replace with shared `y` prefix description).

In `docs/configuration.md`, list the new command IDs next to other `results-copy-*` commands.

- [ ] **Step 3: Fix Help unit tests**

Run: `cargo test --lib help -- --nocapture`  
Fix any assertions that expect `ResultsCopyCell` keys `"y"` or missing Relation context / prefix ranks.

- [ ] **Step 4: Commit**

```bash
git add src/help.rs src/config.rs src/app.rs src/input/keymap.rs docs/keybindings.md docs/configuration.md
git commit -m "docs(help): document ys/yj/yq grid yank shortcuts"
```

---

### Task 7: Final verification

- [ ] **Step 1: Run focused suites**

```bash
cargo test --lib cell_sql_literal -- --nocapture
cargo test --lib json_ -- --nocapture
cargo test --lib insert_sql_ -- --nocapture
cargo test --test keymap sql_results_ys_and_yj relation_browse_ys_yj_yq relation_browse_yy dashboard_processes_keeps shifted_y_copies -- --nocapture
cargo test --lib help -- --nocapture
```

Expected: all PASS.

- [ ] **Step 2: Format**

```bash
cargo fmt --all
```

- [ ] **Step 3: Manual smoke (human / agent with real terminal)**

In a real terminal session (automated tests do not prove clipboard delivery):

1. SQL Results: `ys` cell, `yj` JSON, `Y` TSV still works; bare `y` alone does nothing until second key.
2. Relation Data: `ys`, `yj`, `yq`, `yy`+`p`, `Y`.
3. Dashboard Processes: bare `y` still copies cell.

- [ ] **Step 4: Commit any leftover doc/test fixes if needed**

---

## Spec coverage check

| Spec requirement | Task |
|---|---|
| `ys` cell copy both grids | 2 (reuse), 4, 5, 6 |
| `yj` JSON object both grids | 2, 4, 5, 6 |
| `yq` INSERT Relation only | 1, 3, 4, 5, 6 |
| Bare `y` pending in SQL Results | 5 |
| Dashboard keeps immediate `y`/`Y` | 5 |
| Keep `Y` / `Space Y` / `yy` | 5 (unchanged paths) |
| Literal reuse / MySQL `X'…'` | 1, 3 |
| Help + keybindings docs | 6 |
| Config catalog IDs | 6 |
| Empty snapshot warning | 4 |

## Placeholder scan

No TBD/TODO steps. Dashboard test setup explicitly says reuse existing fixture patterns rather than leaving “implement later.”
