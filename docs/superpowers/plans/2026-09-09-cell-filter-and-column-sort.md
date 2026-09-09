# Cell Filter (`F`) and Column Sort Cycle (`O`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `F` (filter the grid by the selected cell's value, replacing the WHERE draft and resubmitting) and `O` (cycle the selected column through DESC → ASC → unsorted) to both the RELATION DATA and SQL results grids.

**Architecture:** A pure clause generator lives in `src/sql/relation_filter.rs` next to `cycle_relation_column_sort`. Two new `Action` variants resolve the cursor's cell/column in `src/app.rs`, write into the query-bar `TextInput`s, and delegate to the existing `Action::SubmitDataQuery` / `Action::CycleDataColumnSort` paths. A shared `data_grid_query_context` helper replaces the tab matching currently inlined in the `CycleDataColumnSort` handler. Keys are hardcoded in the shared `map_data_query` Results branch beside `/` and `s`.

**Tech Stack:** Rust (ratatui/crossterm TUI, existing codebase patterns; `sqlparser` for clause validation already in place).

**Spec:** `docs/superpowers/specs/2026-09-09-cell-filter-and-column-sort-design.md`

**Verification commands used throughout:** `cargo test --lib <filter>` for targeted tests, `cargo test --lib` for the suite, `cargo fmt --all` before commits. Run all commands from the repo root.

---

### Task 1: `cell_where_clause` clause generator

**Files:**
- Modify: `src/sql/relation_filter.rs` (add import near line 7, function after `cycle_relation_column_sort` ~line 97, tests inside `mod tests` ~line 320)

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests` in `src/sql/relation_filter.rs` (the module already has `use super::*;` via its existing tests; add chrono imports at the top of the test module if not present):

```rust
    #[test]
    fn cell_where_clause_generates_literals_per_dialect() {
        use crate::db::value::CellValue;

        // NULL becomes IS NULL and needs no literal.
        assert_eq!(
            cell_where_clause("status", &CellValue::Null, SqlDialect::Postgres).unwrap(),
            "\"status\" IS NULL"
        );
        // Booleans: TRUE/FALSE on Postgres, 1/0 elsewhere.
        assert_eq!(
            cell_where_clause("flag", &CellValue::Boolean(true), SqlDialect::Postgres).unwrap(),
            "\"flag\" = TRUE"
        );
        assert_eq!(
            cell_where_clause("flag", &CellValue::Boolean(false), SqlDialect::MySql).unwrap(),
            "`flag` = 0"
        );
        assert_eq!(
            cell_where_clause("flag", &CellValue::Boolean(true), SqlDialect::Sqlite).unwrap(),
            "\"flag\" = 1"
        );
        assert_eq!(
            cell_where_clause("flag", &CellValue::Boolean(true), SqlDialect::SqlServer).unwrap(),
            "[flag] = 1"
        );
        // Numbers stay unquoted.
        assert_eq!(
            cell_where_clause("id", &CellValue::Integer(42), SqlDialect::Postgres).unwrap(),
            "\"id\" = 42"
        );
        assert_eq!(
            cell_where_clause("n", &CellValue::Float(3.5), SqlDialect::Postgres).unwrap(),
            "\"n\" = 3.5"
        );
        // Text: '' escaping everywhere, backslash doubling on MySQL.
        assert_eq!(
            cell_where_clause("name", &CellValue::Text("it's".into()), SqlDialect::Postgres).unwrap(),
            "\"name\" = 'it''s'"
        );
        assert_eq!(
            cell_where_clause("name", &CellValue::Text("a\\b".into()), SqlDialect::MySql).unwrap(),
            "`name` = 'a\\\\b'"
        );
        // Bytes literals per dialect.
        assert_eq!(
            cell_where_clause("data", &CellValue::Bytes(vec![0xDE, 0xAD]), SqlDialect::MySql).unwrap(),
            "`data` = 0xDEAD"
        );
        assert_eq!(
            cell_where_clause("data", &CellValue::Bytes(vec![0xDE, 0xAD]), SqlDialect::SqlServer).unwrap(),
            "[data] = 0xDEAD"
        );
        assert_eq!(
            cell_where_clause("data", &CellValue::Bytes(vec![0xDE, 0xAD]), SqlDialect::Sqlite).unwrap(),
            "\"data\" = x'DEAD'"
        );
        assert_eq!(
            cell_where_clause("data", &CellValue::Bytes(vec![0xDE, 0xAD]), SqlDialect::Postgres).unwrap(),
            "\"data\" = '\\xDEAD'::bytea"
        );
    }

    #[test]
    fn cell_where_clause_quotes_temporal_values_and_rejects_unusable_values() {
        use crate::db::value::CellValue;
        use chrono::{NaiveDate, NaiveDateTime, NaiveTime};

        let date = NaiveDate::from_ymd_opt(2026, 8, 28).unwrap();
        assert_eq!(
            cell_where_clause("born", &CellValue::Date(date), SqlDialect::Postgres).unwrap(),
            "\"born\" = '2026-08-28'"
        );
        let datetime = NaiveDateTime::new(
            date,
            NaiveTime::from_hms_opt(10, 20, 31).unwrap(),
        );
        assert_eq!(
            cell_where_clause("at", &CellValue::DateTime(datetime), SqlDialect::Postgres).unwrap(),
            "\"at\" = '2026-08-28 10:20:31'"
        );
        assert!(cell_where_clause(
            "n",
            &CellValue::Float(f64::NAN),
            SqlDialect::Postgres
        )
        .is_err());
        assert!(cell_where_clause(
            "doc",
            &CellValue::Unsupported {
                type_name: "xml".into(),
                preview: "<a/>".into()
            },
            SqlDialect::Postgres
        )
        .is_err());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib cell_where_clause`
Expected: FAIL to compile — `cannot find function cell_where_clause in this scope`.

- [ ] **Step 3: Implement the generator**

In `src/sql/relation_filter.rs`, extend the imports at the top (after the existing `use crate::model::relation::RelationPreviewOptions;` line 7):

```rust
use crate::db::value::CellValue;
```

Add after `cycle_relation_column_sort` (ends ~line 97):

```rust
pub fn cell_where_clause(
    column: &str,
    value: &CellValue,
    dialect: SqlDialect,
) -> Result<String, RelationFilterError> {
    let quoted = quote_identifier(column, dialect);
    let literal = match value {
        CellValue::Null => return Ok(format!("{quoted} IS NULL")),
        CellValue::Boolean(inner) => match dialect {
            SqlDialect::Postgres => if *inner { "TRUE" } else { "FALSE" }.to_owned(),
            _ => u8::from(*inner).to_string(),
        },
        CellValue::Integer(_) | CellValue::Unsigned(_) => value.clipboard_text(),
        CellValue::Float(inner) if inner.is_finite() => value.clipboard_text(),
        CellValue::Float(_) => {
            return Err(RelationFilterError(
                "non-finite float values have no portable SQL literal".into(),
            ))
        }
        CellValue::Text(inner) => string_literal(inner, dialect),
        CellValue::Bytes(inner) => bytes_literal(inner, dialect),
        CellValue::Date(_)
        | CellValue::Time(_)
        | CellValue::DateTime(_)
        | CellValue::Timestamp(_) => string_literal(&value.clipboard_text(), dialect),
        CellValue::Unsupported { type_name, .. } => {
            return Err(RelationFilterError(format!(
                "values of type `{type_name}` cannot be filtered"
            )))
        }
    };
    Ok(format!("{quoted} = {literal}"))
}

fn string_literal(value: &str, dialect: SqlDialect) -> String {
    let escaped = value.replace('\'', "''");
    let escaped = if dialect == SqlDialect::MySql {
        escaped.replace('\\', "\\\\")
    } else {
        escaped
    };
    format!("'{escaped}'")
}

fn bytes_literal(value: &[u8], dialect: SqlDialect) -> String {
    let hex = value
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<String>();
    match dialect {
        SqlDialect::MySql | SqlDialect::SqlServer => format!("0x{hex}"),
        SqlDialect::Postgres => format!("'\\x{hex}'::bytea"),
        SqlDialect::Sqlite | SqlDialect::Generic => format!("x'{hex}'"),
    }
}
```

Notes for the implementer:
- `quote_identifier` is already imported in this file (line 6, `use super::{SqlDialect, dialect::parser_dialect, quote_identifier};`).
- Temporal values reuse `CellValue::clipboard_text()` (public, `src/db/value.rs:28`) which already emits database-friendly ISO formats.
- Export `cell_where_clause` from the sql module: in `src/sql/mod.rs` line ~55 the `pub use relation_filter::{...}` list already exports `cycle_relation_column_sort`; add `cell_where_clause` to that list.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib cell_where_clause`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/sql/relation_filter.rs src/sql/mod.rs
git commit -m "feat(sql): cell value to WHERE clause generator"
```

---

### Task 2: Shared `data_grid_query_context` helper (behavior-preserving refactor)

**Files:**
- Modify: `src/app.rs` (`Action::CycleDataColumnSort` handler at ~line 8262; new helper + failure enum near it)

- [ ] **Step 1: Add the helper and refactor the handler**

In `src/app.rs`, add above the `Action::CycleDataColumnSort(column)` match arm (~line 8262):

```rust
    enum DataGridQueryContextFailure {
        Unavailable,
        Loading,
        NoSucceededExecution,
        AmbiguousColumns,
    }

    fn data_grid_query_context(
        &self,
    ) -> Result<(String, Vec<ColumnMeta>, SqlDialect), DataGridQueryContextFailure> {
        let Self { tabs, active_tab, .. } = self;
        match tabs.get(*active_tab) {
            Some(WorkspaceTab::Relation(tab)) if tab.view == RelationView::Data => {
                if matches!(tab.data, RelationLoad::Loading { .. }) {
                    return Err(DataGridQueryContextFailure::Loading);
                }
                let columns = self
                    .relation_result()
                    .ok_or(DataGridQueryContextFailure::Unavailable)?
                    .columns;
                Ok((
                    tab.query.order_by_input.value().to_owned(),
                    columns,
                    self.sql_dialect(),
                ))
            }
            Some(WorkspaceTab::Sql(tab))
                if tab.result_view == ResultView::Data
                    && matches!(tab.query.capability, DataQueryCapability::Sql) =>
            {
                if tab.query_status == QueryStatus::Running
                    || tab.derived.as_ref().is_some_and(|derived| derived.running)
                {
                    return Err(DataGridQueryContextFailure::Loading);
                }
                let last = tab
                    .last_execution
                    .as_ref()
                    .filter(|last| last.result == ExecutionResult::Succeeded)
                    .ok_or(DataGridQueryContextFailure::NoSucceededExecution)?;
                let result = tab
                    .derived
                    .as_ref()
                    .and_then(|derived| derived.outcome.as_ref())
                    .or(tab.outcome.as_ref())
                    .and_then(|outcome| outcome.result_sets.last())
                    .ok_or(DataGridQueryContextFailure::NoSucceededExecution)?;
                if result
                    .columns
                    .iter()
                    .map(|column| column.name.to_lowercase())
                    .collect::<HashSet<_>>()
                    .len()
                    != result.columns.len()
                {
                    return Err(DataGridQueryContextFailure::AmbiguousColumns);
                }
                Ok((
                    tab.query.order_by_input.value().to_owned(),
                    result.columns.clone(),
                    last.draft.dialect,
                ))
            }
            _ => Err(DataGridQueryContextFailure::Unavailable),
        }
    }

    fn data_query_failure_message(failure: &DataGridQueryContextFailure) -> &'static str {
        match failure {
            DataGridQueryContextFailure::Unavailable => "No data query is available here",
            DataGridQueryContextFailure::Loading => "Data is still loading",
            DataGridQueryContextFailure::NoSucceededExecution => {
                "Run a query successfully first"
            }
            DataGridQueryContextFailure::AmbiguousColumns => {
                "Result column names are ambiguous; use unique aliases"
            }
        }
    }
```

Then replace the body of the `Action::CycleDataColumnSort(column)` arm (currently lines ~8262-8331: the `let Some((order_by, columns, dialect)) = self.tabs.get(...)` block plus the set-and-submit tail) with:

```rust
            Action::CycleDataColumnSort(column) => {
                let Ok((order_by, columns, dialect)) = self.data_grid_query_context() else {
                    return Vec::new();
                };
                let column_names = columns
                    .iter()
                    .map(|column| column.name.as_str())
                    .collect::<Vec<_>>();
                let Ok(next_order_by) =
                    sql::cycle_relation_column_sort(&order_by, &column_names, column, dialect)
                else {
                    return Vec::new();
                };
                match self.tabs.get_mut(self.active_tab) {
                    Some(WorkspaceTab::Relation(tab)) => {
                        tab.query.order_by_input.set(next_order_by)
                    }
                    Some(WorkspaceTab::Sql(tab)) => tab.query.order_by_input.set(next_order_by),
                    _ => return Vec::new(),
                }
                self.update(Action::SubmitDataQuery)
            }
```

Behavior note: every failure maps to a silent `Vec::new()` exactly as the inlined code did today (relation `Loading` → None, `relation_result()` empty → None, SQL running/missing execution/ambiguous → None). `data_query_failure_message` is unused in this task — add `#[allow(dead_code)]` on it or on the enum variant level; it becomes used in Tasks 3–4. Prefer `#[allow(dead_code)]` above `fn data_query_failure_message` and remove the attribute in Task 4.

- [ ] **Step 2: Run the existing sort tests to verify no behavior change**

Run: `cargo test --lib sort`
Expected: PASS, including `sql_result_header_sort_submits_a_derived_query` and all `relation_filter` cycle tests. Also run `cargo build --lib` to confirm no borrow errors from the destructure (`let Self { tabs, active_tab, .. } = self;` — all shared borrows; `self.relation_result()` and `self.sql_dialect()` take `&self`).

- [ ] **Step 3: Commit**

```bash
git add src/app.rs
git commit -m "refactor(app): shared data grid query context helper"
```

---

### Task 3: `O` — `CycleSelectedColumnSort`

**Files:**
- Modify: `src/action.rs` (enum, after `CycleDataColumnSort(usize)` ~line 629)
- Modify: `src/app.rs` (guard lists at ~line 2202 and ~line 2355; handler near the `CycleDataColumnSort` arm; `Id` dispatch near line 2063 lands in Task 5)
- Modify: `src/input/keymap.rs` (shared Results branch in `map_data_query`, ~line 3188; test module)
- Test: `src/input/keymap.rs` tests, `src/app.rs` tests

- [ ] **Step 1: Write the failing keymap test**

In `src/input/keymap.rs` test module (near the existing `relation_app` helper ~line 3244):

```rust
    #[test]
    fn results_sort_and_filter_keys_map_from_relation_data() {
        let mut app = App::new(Vec::new());
        let tab = RelationTab::new("users");
        app.tabs.push(WorkspaceTab::Relation(tab));
        app.active_tab = app.tabs.len() - 1;
        app.focus = Focus::Results;
        let mut keymap = Keymap::default();

        assert_eq!(
            keymap.map(KeyEvent::new(KeyCode::Char('O'), KeyModifiers::NONE), &app),
            Some(Action::CycleSelectedColumnSort)
        );
    }
```

(`RelationTab::new` already defaults to `RelationView::Data` with `DataQueryCapability::Relation`, so `map_data_query` accepts the tab. The `F` assertion is added in Task 4 to keep this task compiling.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib results_sort_and_filter_keys_map_from_relation_data`
Expected: FAIL to compile — no `Action::CycleSelectedColumnSort`.

- [ ] **Step 3: Implement the action, handler, guards, and keymap**

1. `src/action.rs`, after `CycleDataColumnSort(usize),` (~line 629):

```rust
    CycleSelectedColumnSort,
```

2. `src/app.rs` — add to **both** guard lists in `update()`'s entry guard, directly below each existing `| Action::CycleDataColumnSort(_)` entry (~line 2202 and ~line 2355):

```rust
                        | Action::CycleSelectedColumnSort
```

(match the surrounding indentation of each list: list A uses 24 spaces before `|`, list B uses 20.)

3. `src/app.rs` — handler arm directly above `Action::CycleDataColumnSort(column)`:

```rust
            Action::CycleSelectedColumnSort => {
                let (columns, _) = match self.data_grid_query_context() {
                    Ok((_, columns, dialect)) => (columns, dialect),
                    Err(DataGridQueryContextFailure::Unavailable) => return Vec::new(),
                    Err(failure) => {
                        self.notify_warning("Sort", data_query_failure_message(&failure));
                        return Vec::new();
                    }
                };
                let column = self.active_grid_column();
                if column >= columns.len() {
                    return Vec::new();
                }
                self.update(Action::CycleDataColumnSort(column))
            }
```

Place `Action::CycleSelectedColumnSort` and `Action::FilterGridCellByValue` arms **before** the `Action::CycleDataColumnSort(column)` arm (order in the match is irrelevant, but keep related arms adjacent).

4. `src/input/keymap.rs` — in `map_data_query`'s final branch (~line 3188), extend the match:

```rust
    if app.focus == Focus::Results {
        return match event.code {
            KeyCode::Char('/') => Some(Action::FocusDataQueryInput(
                crate::model::data_query::DataQueryInput::Where,
            )),
            KeyCode::Char('s') => Some(Action::FocusDataQueryInput(
                crate::model::data_query::DataQueryInput::OrderBy,
            )),
            KeyCode::Char('F') => Some(Action::FilterGridCellByValue),
            KeyCode::Char('O') => Some(Action::CycleSelectedColumnSort),
            _ => None,
        };
    }
```

Adding the `Char('F')` line now requires `Action::FilterGridCellByValue` to exist — therefore **Tasks 3 and 4 both add their Action variants and guard-list entries in this step**, but only their own keymap line and handler. In this task add to `src/action.rs` after `CycleSelectedColumnSort,`:

```rust
    FilterGridCellByValue,
```

and to both guard lists in `src/app.rs` below the `CycleSelectedColumnSort` line:

```rust
                        | Action::FilterGridCellByValue
```

and a temporary no-op handler arm in `src/app.rs` (replaced in Task 4):

```rust
            Action::FilterGridCellByValue => Vec::new(),
```

5. Remove the `#[allow(dead_code)]` from `data_query_failure_message` (now used).

- [ ] **Step 4: Write the failing app integration test (O cycles desc → asc → none)**

In `src/app.rs` test module, next to `sql_result_header_sort_submits_a_derived_query` (~line 18242). Reuse its fixture style — the helper `connected_query_app` and the imports it already uses (`ResultSet`, `ColumnMeta`, `CellValue`, `QueryOutcome`, `QueryStats`, `Duration`):

```rust
    #[test]
    fn selected_column_sort_cycles_desc_asc_none_from_results() {
        let (mut app, tab_id, generation) = connected_query_app("SELECT id, name FROM users");
        let connection = app.connection.active_identity().unwrap();
        app.update(Action::QueryFinished {
            tab_id,
            generation,
            connection,
            outcome: QueryOutcome {
                result_sets: vec![ResultSet {
                    columns: vec![
                        ColumnMeta {
                            name: "id".into(),
                            type_name: "bigint".into(),
                        },
                        ColumnMeta {
                            name: "name".into(),
                            type_name: "text".into(),
                        },
                    ],
                    rows: vec![vec![CellValue::Integer(1), CellValue::Text("one".into())]],
                    affected_rows: 0,
                }],
                stats: QueryStats::new(Duration::ZERO, Duration::ZERO, 1),
            },
        });

        let commands = app.update(Action::CycleSelectedColumnSort);
        assert_eq!(
            app.active_console().query.order_by_input.value(),
            "\"id\" DESC"
        );
        assert!(matches!(&commands[..], [Command::RunDerivedQueryPage { .. }]));

        app.update(Action::QueryFinished {
            tab_id,
            generation: generation + 1,
            connection: app.connection.active_identity().unwrap(),
            outcome: QueryOutcome {
                result_sets: vec![ResultSet {
                    columns: vec![
                        ColumnMeta {
                            name: "id".into(),
                            type_name: "bigint".into(),
                        },
                        ColumnMeta {
                            name: "name".into(),
                            type_name: "text".into(),
                        },
                    ],
                    rows: vec![vec![CellValue::Integer(1), CellValue::Text("one".into())]],
                    affected_rows: 0,
                }],
                stats: QueryStats::new(Duration::ZERO, Duration::ZERO, 1),
            },
        });
        app.active_console_mut().query.order_by_input.set("\"id\" DESC");

        let commands = app.update(Action::CycleSelectedColumnSort);
        assert_eq!(
            app.active_console().query.order_by_input.value(),
            "\"id\" ASC"
        );
        assert!(matches!(&commands[..], [Command::RunDerivedQueryPage { .. }]));

        app.update(Action::CycleSelectedColumnSort);
        assert_eq!(app.active_console().query.order_by_input.value(), "");
    }
```

Note the second/third presses re-derive from `data_grid_query_context`'s SQL branch, which prefers `derived.outcome`; if `connected_query_app`'s derived flow rejects the second `QueryFinished` generation, instead assert only the first DESC press and the input-cycling (`DESC` → `ASC` → ``) by calling `Action::CycleDataColumnSort(0)` directly for presses 2–3 — the wrapper adds only the cursor lookup and notifications. The essential assertions for THIS task are: keypress resolves cursor column 0, produces `"id" DESC`, and submits a derived query.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib selected_column_sort_cycles && cargo test --lib results_sort_and_filter_keys_map`
Expected: PASS. Also `cargo build --lib` (guard lists and no-op arm keep the build green).

- [ ] **Step 6: Commit**

```bash
git add src/action.rs src/app.rs src/input/keymap.rs
git commit -m "feat(ui): O cycles selected column sort"
```

---

### Task 4: `F` — `FilterGridCellByValue`

**Files:**
- Modify: `src/app.rs` (replace the no-op handler from Task 3; extend the keymap test)
- Modify: `src/input/keymap.rs` (extend the Task 3 test with the `F` assertion)
- Test: `src/app.rs` tests

- [ ] **Step 1: Extend the keymap test with the failing `F` assertion**

In `src/input/keymap.rs`, extend `results_sort_and_filter_keys_map_from_relation_data`:

```rust
        assert_eq!(
            keymap.map(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::NONE), &app),
            Some(Action::FilterGridCellByValue)
        );
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib results_sort_and_filter_keys_map`
Expected: FAIL — `keymap.map(...)` returns `None` for `F` (handler is still the no-op; the keymap line from Task 3 already routes `Char('F')`, so if this passes early the keymap line is already correct and only the handler is missing — proceed to Step 3 and verify via the app integration test instead).

- [ ] **Step 3: Implement the handler**

Replace the no-op arm `Action::FilterGridCellByValue => Vec::new(),` in `src/app.rs` with:

```rust
            Action::FilterGridCellByValue => {
                let (_, columns, dialect) = match self.data_grid_query_context() {
                    Ok(context) => context,
                    Err(DataGridQueryContextFailure::Unavailable) => return Vec::new(),
                    Err(failure) => {
                        self.notify_warning("Filter", data_query_failure_message(&failure));
                        return Vec::new();
                    }
                };
                let column_index = self.active_grid_column();
                let Some(column) = columns.get(column_index) else {
                    return Vec::new();
                };
                let Some((_, row, _, _)) = self.active_record_snapshot() else {
                    return Vec::new();
                };
                let Some(value) = row.get(column_index) else {
                    return Vec::new();
                };
                match sql::cell_where_clause(&column.name, value, dialect) {
                    Ok(clause) => {
                        match self.tabs.get_mut(self.active_tab) {
                            Some(WorkspaceTab::Relation(tab)) => {
                                tab.query.where_input.set(clause)
                            }
                            Some(WorkspaceTab::Sql(tab)) => tab.query.where_input.set(clause),
                            _ => return Vec::new(),
                        }
                        self.update(Action::SubmitDataQuery)
                    }
                    Err(error) => {
                        self.notify_warning("Filter", error.to_string());
                        Vec::new()
                    }
                }
            }
```

- [ ] **Step 4: Write the failing-then-passing app integration tests**

In `src/app.rs` test module:

**SQL tab (derived query path):**

```rust
    #[test]
    fn filter_selected_cell_submits_derived_where_clause() {
        let (mut app, tab_id, generation) = connected_query_app("SELECT id, name FROM users");
        let connection = app.connection.active_identity().unwrap();
        app.update(Action::QueryFinished {
            tab_id,
            generation,
            connection,
            outcome: QueryOutcome {
                result_sets: vec![ResultSet {
                    columns: vec![
                        ColumnMeta {
                            name: "id".into(),
                            type_name: "bigint".into(),
                        },
                        ColumnMeta {
                            name: "name".into(),
                            type_name: "text".into(),
                        },
                    ],
                    rows: vec![vec![CellValue::Integer(1), CellValue::Text("one".into())]],
                    affected_rows: 0,
                }],
                stats: QueryStats::new(Duration::ZERO, Duration::ZERO, 1),
            },
        });

        let commands = app.update(Action::FilterGridCellByValue);

        assert_eq!(
            app.active_console().query.where_input.value(),
            "\"id\" = 1"
        );
        assert!(matches!(
            commands.as_slice(),
            [Command::RunDerivedQueryPage { where_clause, .. }] if where_clause == "\"id\" = 1"
        ));
    }
```

**Relation tab (preview reload path, NULL cell):** build on the fixture style at `src/app.rs:18870-18925` (profile + `ConnectionIdentity` + `ConnectionStatus::Connected` + `ExecutionTarget` + `RelationTab::with_descriptor` + `RelationLoad::Ready`). Selected cell (0,1) is NULL:

```rust
    #[test]
    fn filter_selected_null_cell_filters_with_is_null() {
        // Fixture: profile/connection/execution-target setup copied from the
        // relation edit fixture at src/app.rs:18870-18910, with columns
        // ("name" text, "id" bigint) and one row
        // [Text("one"), Null]. Set tab.grid.selected_column = 1 (id column is
        // index 1 with NULL), then:
        let commands = app.update(Action::FilterGridCellByValue);

        let Some(WorkspaceTab::Relation(tab)) = app.tabs.get(app.active_tab) else {
            panic!("expected relation tab");
        };
        assert_eq!(tab.query.where_input.value(), "\"id\" IS NULL");
        assert!(matches!(&commands[..], [Command::LoadRelationPreview(request)]
            if request.options.where_clause.as_deref() == Some("\"id\" IS NULL")));
    }
```

Copy the full fixture code from the existing test rather than referencing it — the plan requires self-contained tests. Concretely mirror `successful_relation_commit...`'s imports (`RelationTab`, `RelationDescriptor`, `RelationKey`, `QualifiedName`, `CatalogKind`, `CatalogId`, `OwnedSnapshot`, `SnapshotAttribution`, `RelationLoad`, `RelationView`, `ConnectionStatus`, `ExecutionTarget`) and the profile construction from the fixture at 18870-18910, replacing the edit-session setup with `rows: vec![vec![CellValue::Text("one".into()), CellValue::Null]]` and `row_versions: None`, and setting `app.focus = Focus::Results;` plus `tab.grid.selected_column = 1;` before pushing the tab.

**Unsupported value (warning, no submission):** same relation fixture but cell (0,0) set to:

```rust
        CellValue::Unsupported {
            type_name: "xml".into(),
            preview: "<a/>".into(),
        }
```

assert `commands.is_empty()`, `tab.query.where_input.value().is_empty()`, and:

```rust
        assert_eq!(
            app.notifications.history().next().unwrap().title,
            "Filter"
        );
```

**Ambiguous SQL columns (warning):** SQL-tab fixture where both columns are named `id`; assert `commands.is_empty()` and the notification title is `"Filter"` with body containing `ambiguous`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib filter_selected`
Expected: PASS (all four tests).

- [ ] **Step 6: Commit**

```bash
git add src/app.rs src/input/keymap.rs
git commit -m "feat(ui): F filters grid by selected cell value"
```

---

### Task 5: Help palette registration

**Files:**
- Modify: `src/help.rs` (enum ~line 374, catalog rows ~line 1587, `footer_priority` ~line 637)
- Modify: `src/app.rs` (`Id` dispatch near line 2086)
- Test: `src/help.rs` tests

- [ ] **Step 1: Write the failing help test**

In `src/help.rs` test module (near `filtering_and_non_first_selection_use_stable_ids` ~line 3673):

```rust
    #[test]
    fn filter_and_sort_shortcuts_listed_for_both_grid_contexts() {
        for context in [
            ShortcutContext::RelationDataBrowse,
            ShortcutContext::SqlResultsData,
        ] {
            let shortcuts = shortcuts_for_context(context, ShortcutCapabilities::all());
            let filter = shortcuts
                .iter()
                .find(|shortcut| shortcut.id == HelpShortcutId::ResultsFilterCell)
                .expect("filter shortcut listed");
            assert_eq!(filter.sequence, "F");
            let sort = shortcuts
                .iter()
                .find(|shortcut| shortcut.id == HelpShortcutId::ResultsSortColumn)
                .expect("sort shortcut listed");
            assert_eq!(sort.sequence, "O");
        }
    }
```

Before writing it, check how existing help tests enumerate shortcuts for a context (e.g. `filtered_shortcuts(context, capabilities, "")` from `src/help.rs:2781` — use whichever helper existing tests use; adapt the call signature accordingly). If `ShortcutCapabilities::all()` does not exist, construct capabilities the way neighboring tests do so that `data_query_available` is true (`ShortcutRequirement::DataQueryAvailable` gates the rows, `src/help.rs:2752`).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib filter_and_sort_shortcuts_listed`
Expected: FAIL to compile — no `HelpShortcutId::ResultsFilterCell`.

- [ ] **Step 3: Implement the registrations**

1. `src/help.rs` enum, below `ResultsToggleView,` (~line 374):

```rust
    ResultsFilterCell,
    ResultsSortColumn,
```

2. `src/help.rs` catalog, directly after the `RelationOrderBy` row (~line 1594):

```rust
    row!(
        ResultsFilterCell,
        [SqlResultsData, RelationDataBrowse],
        "F",
        "filter rows by selected cell value",
        DataQueryAvailable,
        executable
    ),
    row!(
        ResultsSortColumn,
        [SqlResultsData, RelationDataBrowse],
        "O",
        "sort by selected column (desc/asc/none)",
        DataQueryAvailable,
        executable
    ),
```

(Verify the macro arm `($id, [..], $sequence, $description, $requirement, executable)` exists by matching the `RelationWhere` row's shape at ~line 1579-1586 — copy that exact arm form.)

3. `src/help.rs` `footer_priority` (~line 637): extend the tier-8 arm to keep them out of the default footer or give them a tier — add both ids to the arm containing `ResultsToggleView`:

```rust
        ResultsToggleView | ResultsFilterCell | ResultsSortColumn | RelationPaste
            | ExplorerRefresh
            | RelationBusyData => 8,
```

4. `src/app.rs` `Id` dispatch, below `Id::ResultsToggleView => ...` (~line 2070):

```rust
            Id::ResultsFilterCell => vec![Action::FilterGridCellByValue],
            Id::ResultsSortColumn => vec![Action::CycleSelectedColumnSort],
```

(The `Id` enum is `HelpShortcutId` re-exported — adding the variants in step 1 is what this match needs. The dispatch arm's catch-all is `_ => unreachable!("display-only shortcut passed execution guard")` at ~line 2100, so executable rows must map here.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib filter_and_sort_shortcuts_listed && cargo test --lib help`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/help.rs src/app.rs
git commit -m "feat(help): list F filter and O sort shortcuts"
```

---

### Task 6: Docs and full verification

**Files:**
- Modify: `docs/keybindings.md` (SQL Results Data table ~line 387; Relation Data Browse table ~line 460)

- [ ] **Step 1: Add doc rows**

In `docs/keybindings.md`, SQL Results Data table — after the `/` / `s` row (~line 390):

```markdown
| `F` | Filter rows by the selected cell's value (`col = value`, `IS NULL` for NULL cells); replaces the WHERE input and reruns the derived query from page one |
| `O` | Cycle the selected column's sort: `DESC`, `ASC`, unsorted; updates the ORDER BY input and reruns like a header click |
```

In the same section's header-sorting paragraph (~line 392-404), append one sentence after "...the same cycle as Relation Data: `DESC`, `ASC`, and unsorted.":

```markdown
`O` triggers the same cycle for the cursor's column without reaching for the mouse.
```

Relation Data → Browse table — after the `/` / `s` row (~line 462):

```markdown
| `F` | Filter rows by the selected cell's value (`col = value`, `IS NULL` for NULL cells); replaces the WHERE input and submits the preview |
| `O` | Cycle the selected column's sort: `DESC`, `ASC`, unsorted; same as clicking the column header |
```

- [ ] **Step 2: Full suite, fmt, and final commit**

Run: `cargo fmt --all && cargo test --lib`
Expected: all tests PASS.

```bash
git add docs/keybindings.md
git commit -m "docs: F cell filter and O column sort keys"
```

---

## Self-Review (completed)

- **Spec coverage:** clause generator (Task 1), shared helper + refactor (Task 2), `O` incl. keymap + integration (Task 3), `F` incl. NULL/Unsupported/ambiguity/notification paths (Task 4), help + Id dispatch + footer (Task 5), docs (Task 6). Guard-list updates for both actions land in Task 3 (both variants together, keeping the build green). ✔
- **Placeholder scan:** Task 4's relation fixture deliberately references the existing fixture at `src/app.rs:18870-18910` with explicit instructions on what to copy and change — the implementing agent must inline the fixture code into the test; assertions themselves are complete. ✔
- **Type consistency:** `data_grid_query_context` returns `(String, Vec<ColumnMeta>, SqlDialect)` in Task 2 and is destructured identically in Tasks 3–4. `cell_where_clause(column: &str, value: &CellValue, dialect: SqlDialect) -> Result<String, RelationFilterError>` used in Task 4 matches Task 1's definition. Action names `FilterGridCellByValue` / `CycleSelectedColumnSort` consistent across action.rs, app.rs, keymap.rs, help.rs. ✔
