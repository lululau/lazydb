# SQL Activity Overlay Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a current-tab SQL Activity Overlay that shows PENDING mutation SQL for an open transaction and COMMITTED batches from successful commits this process lifetime, with Enter jumping into the existing Console/Relation confirm overlays.

**Architecture:** Introduce a small `sql_activity` model helper for batch history and Console pending accumulation. Console gets `pending_transaction_sql` + `committed_sql_batches`; Relation reuses `transaction_review_sql` and adds the same ring buffer. `App` opens `Overlay::SqlActivity`, renders a stacked PENDING/COMMITTED modal in `src/ui/`, and routes Enter through `open_console_transaction_control` / Relation review open—never direct Commit.

**Tech Stack:** Rust, existing Overlay/keymap/clipboard patterns, `chrono::Local`, `VecDeque` ring buffer.

**Spec:** `docs/superpowers/specs/2026-09-11-sql-activity-overlay-design.md`

**Verification commands:** `cargo test --lib model::sql_activity`, `cargo test --lib sql_activity`, `cargo test --lib app`, `cargo fmt --all -- --check`, `cargo clippy --lib -- -D warnings`

---

## File Map

| File | Responsibility |
|------|----------------|
| `src/model/sql_activity.rs` | `CommittedSqlBatch`, ring-buffer helpers, `should_record_pending_sql`, `append_pending_sql`, `statement_count_in_pending`, empty-state copy constants |
| `src/model/mod.rs` | Export `sql_activity` |
| `src/model/tab.rs` | `ConsoleTab.pending_transaction_sql`, `committed_sql_batches`; constructors |
| `src/model/relation.rs` | `RelationTab.committed_sql_batches`; constructors |
| `src/model/workspace.rs` | `Overlay::SqlActivity(SqlActivityState)` + state types |
| `src/action.rs` | `OpenSqlActivity`, scroll/move/expand/yank/enter/dismiss actions |
| `src/app.rs` | Open/dismiss overlay; pending append/clear/commit archive hooks; Enter routing; yank |
| `src/ui/mod.rs` | Render SQL Activity overlay |
| `src/input/keymap.rs` | Overlay-local keys + global/leader binding match |
| `src/config.rs` | Register `sql-activity` in `SUPPORTED_COMMANDS` |
| `config/default.toml` | Default chord `Space a` |
| `docs/configuration.md` | Document binding |
| `src/help.rs` | Optional shortcut listing if other globals are listed there |

---

### Task 1: SQL Activity model helpers

**Files:**
- Create: `src/model/sql_activity.rs`
- Modify: `src/model/mod.rs`

- [ ] **Step 1: Write failing unit tests**

Create `src/model/sql_activity.rs` with a `#[cfg(test)] mod tests` that expects these APIs (they will not exist yet if you prefer split TDD—otherwise write tests first in the same file before implementations):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::SqlRisk;
    use std::collections::VecDeque;
    use std::time::Duration;

    #[test]
    fn should_record_skips_readonly_and_transaction_control_only() {
        assert!(!should_record_pending_sql(&[SqlRisk::ReadOnly]));
        assert!(!should_record_pending_sql(&[SqlRisk::TransactionControl]));
        assert!(!should_record_pending_sql(&[
            SqlRisk::ReadOnly,
            SqlRisk::TransactionControl
        ]));
        assert!(should_record_pending_sql(&[SqlRisk::Dml]));
        assert!(should_record_pending_sql(&[SqlRisk::ReadOnly, SqlRisk::Dml]));
        assert!(should_record_pending_sql(&[SqlRisk::Unknown]));
    }

    #[test]
    fn append_pending_uses_blank_line_separator() {
        let mut pending = String::new();
        append_pending_sql(&mut pending, "UPDATE t SET a=1;");
        append_pending_sql(&mut pending, "DELETE FROM t WHERE id=1;");
        assert_eq!(
            pending,
            "UPDATE t SET a=1;\n\nDELETE FROM t WHERE id=1;"
        );
        assert_eq!(statement_count_in_pending(&pending), 2);
    }

    #[test]
    fn push_committed_batch_caps_at_fifty_newest_first() {
        let mut batches = VecDeque::new();
        for i in 0..55 {
            push_committed_batch(
                &mut batches,
                CommittedSqlBatch {
                    timestamp: chrono::Local::now(),
                    sql: format!("stmt {i}"),
                    statement_count: 1,
                    elapsed: Some(Duration::from_millis(1)),
                },
            );
        }
        assert_eq!(batches.len(), 50);
        assert_eq!(batches.front().unwrap().sql, "stmt 54");
        assert_eq!(batches.back().unwrap().sql, "stmt 5");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test --lib model::sql_activity -- --nocapture
```

Expected: compile failure or missing items.

- [ ] **Step 3: Implement helpers**

```rust
// src/model/sql_activity.rs
use std::collections::VecDeque;
use std::time::Duration;

use crate::sql::SqlRisk;

pub const MAX_COMMITTED_BATCHES: usize = 50;
pub const EMPTY_NO_OPEN_TXN: &str = "No open transaction";
pub const EMPTY_OPEN_TXN_NO_STMTS: &str = "Open transaction — no recorded statements yet";
pub const EMPTY_NO_COMMITTED: &str = "No committed batches in this tab yet";

#[derive(Clone, Debug, PartialEq)]
pub struct CommittedSqlBatch {
    pub timestamp: chrono::DateTime<chrono::Local>,
    pub sql: String,
    pub statement_count: usize,
    pub elapsed: Option<Duration>,
}

pub fn should_record_pending_sql(risks: &[SqlRisk]) -> bool {
    risks.iter().any(|risk| {
        !matches!(risk, SqlRisk::ReadOnly | SqlRisk::TransactionControl)
    })
}

pub fn append_pending_sql(pending: &mut String, sql: &str) {
    let sql = sql.trim_end();
    if sql.is_empty() {
        return;
    }
    if !pending.is_empty() {
        pending.push_str("\n\n");
    }
    pending.push_str(sql);
}

pub fn statement_count_in_pending(pending: &str) -> usize {
    let count = pending
        .split("\n\n")
        .filter(|segment| !segment.trim().is_empty())
        .count();
    if pending.trim().is_empty() {
        0
    } else {
        count.max(1)
    }
}

pub fn push_committed_batch(batches: &mut VecDeque<CommittedSqlBatch>, batch: CommittedSqlBatch) {
    batches.push_front(batch);
    while batches.len() > MAX_COMMITTED_BATCHES {
        batches.pop_back();
    }
}

pub fn clear_pending_sql(pending: &mut String) {
    pending.clear();
}
```

Export from `src/model/mod.rs`:

```rust
pub mod sql_activity;
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test --lib model::sql_activity -- --nocapture
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/model/sql_activity.rs src/model/mod.rs
git commit -m "feat(sql-activity): add pending/committed batch model helpers"
```

---

### Task 2: Wire fields onto ConsoleTab and RelationTab

**Files:**
- Modify: `src/model/tab.rs`
- Modify: `src/model/relation.rs`

- [ ] **Step 1: Write a failing construction assertion** (in `src/model/sql_activity.rs` tests or tab/relation existing test modules)

```rust
#[test]
fn console_and_relation_tabs_start_with_empty_activity_state() {
    let console = crate::model::tab::ConsoleTab::new("SQL 1");
    assert!(console.pending_transaction_sql.is_empty());
    assert!(console.committed_sql_batches.is_empty());

    let relation = crate::model::relation::RelationTab::new("public.users");
    assert!(relation.committed_sql_batches.is_empty());
}
```

- [ ] **Step 2: Run test to verify it fails** (missing fields)

```bash
cargo test --lib console_and_relation_tabs_start_with_empty_activity_state -- --nocapture
```

- [ ] **Step 3: Add fields and initialize in constructors**

On `ConsoleTab`:

```rust
pub pending_transaction_sql: String,
pub committed_sql_batches: std::collections::VecDeque<crate::model::sql_activity::CommittedSqlBatch>,
```

In `ConsoleTab::new`:

```rust
pending_transaction_sql: String::new(),
committed_sql_batches: std::collections::VecDeque::new(),
```

On `RelationTab`:

```rust
pub committed_sql_batches: std::collections::VecDeque<crate::model::sql_activity::CommittedSqlBatch>,
```

Initialize to `VecDeque::new()` in `with_descriptor` / any other constructors that build a full struct.

Fix all struct-literal compile breaks in tests the same way.

- [ ] **Step 4: Run tests**

```bash
cargo test --lib console_and_relation_tabs_start_with_empty_activity_state -- --nocapture
cargo check --lib
```

Expected: PASS / clean check

- [ ] **Step 5: Commit**

```bash
git add src/model/tab.rs src/model/relation.rs src/model/sql_activity.rs
git commit -m "feat(sql-activity): add tab fields for pending and committed SQL"
```

---

### Task 3: Console pending accumulation + Idle clears + commit archive

**Files:**
- Modify: `src/app.rs` (`ManualQueryFinished`, `ManualCommitted`, `ManualRolledBack`, `ManualImplicitlyEnded`, ClearOutcome / abandon / idle transitions)

- [ ] **Step 1: Write failing app tests**

Add near other SQL logger / transaction tests in `src/app.rs` `mod tests`:

```rust
#[test]
fn manual_mutation_appends_to_pending_transaction_sql() {
    let (mut app, tab_id, generation) = connected_query_app("UPDATE users SET name = 'ada'");
    // force manual active txn matching ManualQueryFinished gates
    let connection = app.connection.active_identity().unwrap();
    let tab = app.active_console_mut();
    tab.transaction_mode = TransactionMode::Manual;
    tab.transaction_state = TransactionState::Active;
    let transaction_generation = tab.transaction_generation;

    app.update(Action::ManualQueryFinished {
        tab_id,
        query_generation: generation,
        transaction_generation,
        connection,
        outcome: empty_outcome(),
    });

    let pending = &app.active_console().pending_transaction_sql;
    assert!(pending.contains("UPDATE users SET name = 'ada'"));
}

#[test]
fn manual_readonly_does_not_append_pending_sql() {
    let (mut app, tab_id, generation) = connected_query_app("SELECT 1");
    let connection = app.connection.active_identity().unwrap();
    let tab = app.active_console_mut();
    tab.transaction_mode = TransactionMode::Manual;
    tab.transaction_state = TransactionState::Active;
    let transaction_generation = tab.transaction_generation;

    app.update(Action::ManualQueryFinished {
        tab_id,
        query_generation: generation,
        transaction_generation,
        connection,
        outcome: empty_outcome(),
    });

    assert!(app.active_console().pending_transaction_sql.is_empty());
}

#[tokio::test]
async fn manual_committed_archives_pending_into_committed_batches() {
    // Arrange: console with pending SQL + Committing state, then ManualCommitted
    // Assert: committed_sql_batches.front().sql == pending snapshot
    // Assert: pending_transaction_sql.is_empty()
}
```

Adapt the third test to the existing `manual_committed_records_in_sql_logger` setup pattern (manual Active → `CommitTransaction` → `ManualCommitted`).

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test --lib manual_mutation_appends_to_pending_transaction_sql -- --nocapture
```

- [ ] **Step 3: Implement hooks**

Helper on `App` (private):

```rust
fn record_console_pending_from_last_execution(&mut self, tab_id: Uuid) {
    let Some(tab) = self.tabs.iter_mut().find(|t| t.id() == tab_id).and_then(WorkspaceTab::as_console_mut) else {
        return;
    };
    if tab.transaction_state != TransactionState::Active {
        return;
    }
    let Some(last) = tab.last_execution.as_ref() else {
        return;
    };
    if !crate::model::sql_activity::should_record_pending_sql(&last.draft.risks) {
        return;
    }
    crate::model::sql_activity::append_pending_sql(
        &mut tab.pending_transaction_sql,
        &last.draft.sql,
    );
}

fn clear_console_pending_sql(&mut self, tab_id: Uuid) {
    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id() == tab_id).and_then(WorkspaceTab::as_console_mut) {
        crate::model::sql_activity::clear_pending_sql(&mut tab.pending_transaction_sql);
    }
}

fn archive_console_committed_batch(&mut self, tab_id: Uuid, elapsed: Option<Duration>) {
    let Some(tab) = self.tabs.iter_mut().find(|t| t.id() == tab_id).and_then(WorkspaceTab::as_console_mut) else {
        return;
    };
    if tab.pending_transaction_sql.trim().is_empty() {
        crate::model::sql_activity::clear_pending_sql(&mut tab.pending_transaction_sql);
        return;
    }
    let sql = std::mem::take(&mut tab.pending_transaction_sql);
    let statement_count = crate::model::sql_activity::statement_count_in_pending(&sql);
    crate::model::sql_activity::push_committed_batch(
        &mut tab.committed_sql_batches,
        crate::model::sql_activity::CommittedSqlBatch {
            timestamp: chrono::Local::now(),
            sql,
            statement_count,
            elapsed,
        },
    );
}
```

Call sites:

1. After successful `finish_query` inside `ManualQueryFinished` (and optionally `ManualQueryPageFinished` only if page executes recorded mutation drafts—default **skip page finishes** unless draft risks require record; page finishes are usually reads → skip).
2. `ManualCommitted` success: `archive_console_committed_batch` **before** txn snapshot clears idle (capture pending first).
3. `ManualRolledBack` success: `clear_console_pending_sql`.
4. Every path that sets console `transaction_state` to `Idle` without successful commit archive: clear pending (`ManualImplicitlyEnded`, ClearOutcome success, abandon in `resolve_transaction_exit`, leaving manual mode). Prefer a single `set_console_txn_idle(tab_id)` helper that clears pending then sets Idle when touching those sites.

Invariant to uphold in tests: Idle ⇒ `pending_transaction_sql` empty.

- [ ] **Step 4: Run tests**

```bash
cargo test --lib manual_mutation_appends_to_pending -- --nocapture
cargo test --lib manual_readonly_does_not_append -- --nocapture
cargo test --lib manual_committed_archives_pending -- --nocapture
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/app.rs
git commit -m "feat(sql-activity): accumulate and archive console pending SQL"
```

---

### Task 4: Relation commit archives review SQL

**Files:**
- Modify: `src/app.rs` (`relation_transaction_finished`)

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn relation_commit_archives_review_sql_batch() {
    let mut app = App::new(Vec::new());
    // attach a connected profile if connection_name needed; history does not need it
    let mut tab = RelationTab::new("public.users");
    tab.transaction_generation = 3;
    tab.transaction_state = TransactionState::Committing;
    tab.transaction_review_sql = Some("UPDATE t SET a=1 WHERE id=1;".into());
    tab.edit = Some(RelationEditSession::from_rows(vec![vec![CellValue::Integer(1)]]));
    let tab_id = tab.id;
    app.tabs.push(WorkspaceTab::Relation(tab));

    app.relation_transaction_finished(
        tab_id,
        3,
        ConnectionIdentity { profile_id: Uuid::nil(), generation: 1 },
        true,
        None,
    );

    let WorkspaceTab::Relation(tab) = app.tabs.iter().find(|t| t.id() == tab_id).unwrap() else {
        panic!();
    };
    assert_eq!(tab.committed_sql_batches.len(), 1);
    assert!(tab.committed_sql_batches[0].sql.contains("UPDATE t SET a=1"));
    assert!(tab.transaction_review_sql.is_none());
}
```

- [ ] **Step 2: Run test to verify fail**

```bash
cargo test --lib relation_commit_archives_review_sql_batch -- --nocapture
```

- [ ] **Step 3: Implement in `relation_transaction_finished`**

On successful commit branch, **before** clearing `transaction_review_sql`:

```rust
if was_committing {
    if let Some(sql) = review_sql.filter(|s| !s.trim().is_empty()) {
        let statement_count = crate::model::sql_activity::statement_count_in_pending(&sql);
        crate::model::sql_activity::push_committed_batch(
            &mut tab.committed_sql_batches,
            crate::model::sql_activity::CommittedSqlBatch {
                timestamp: chrono::Local::now(),
                sql,
                statement_count,
                elapsed: None, // Relation path has no elapsed today
            },
        );
    }
}
```

Use the already-cloned `review_sql` / `was_committing` from the SQL logger work; do not double-clear incorrectly.

Rollback success must **not** push a batch.

- [ ] **Step 4: Run tests**

```bash
cargo test --lib relation_commit_archives_review_sql_batch -- --nocapture
cargo test --lib relation_rollback_records_in_sql_logger -- --nocapture
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/app.rs
git commit -m "feat(sql-activity): archive relation review SQL on commit"
```

---

### Task 5: Overlay state, actions, open/dismiss, Enter, yank

**Files:**
- Modify: `src/model/workspace.rs`
- Modify: `src/action.rs`
- Modify: `src/app.rs`

- [ ] **Step 1: Write failing behavioral tests**

```rust
#[test]
fn open_sql_activity_on_console_with_pending() { /* ... */ }

#[test]
fn open_sql_activity_on_dashboard_notifies() { /* ... */ }

#[test]
fn sql_activity_enter_opens_transaction_exit_confirm_not_commit() {
    // Arrange: manual Active console + Overlay::SqlActivity focused on Pending
    // Act: Action::SqlActivityEnter
    // Assert: overlay is TransactionExitConfirm (or deferred path that creates it)
    // Assert: transaction_state is still Active (not Committing)
}

#[test]
fn sql_activity_yank_pending_writes_clipboard_command() { /* returns WriteClipboard */ }
```

- [ ] **Step 2: Run tests — expect fail**

- [ ] **Step 3: Add types**

In `workspace.rs`:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SqlActivitySection {
    Pending,
    Committed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SqlActivityState {
    pub tab_id: Uuid,
    pub section: SqlActivitySection,
    pub pending_scroll: u16,
    pub committed_cursor: usize,
    pub expanded: BTreeSet<usize>,
    pub status_hint: Option<String>,
}

// on Overlay:
SqlActivity(SqlActivityState),
```

In `action.rs` add:

```rust
OpenSqlActivity,
SqlActivityFocusSection(SqlActivitySection), // or ToggleSection
SqlActivityPendingScroll(i16),
SqlActivityCommittedMove(isize),
SqlActivityToggleExpand,
SqlActivityEnter,
SqlActivityYank,
SqlActivityDismiss,
```

Implement `App::open_sql_activity`:

- Active Console or Relation → set `Overlay::SqlActivity { tab_id, section: Pending, pending_scroll: 0, committed_cursor: 0, expanded: default, status_hint: None }`
- Else → `notify_warning("SQL Activity", "Open a SQL console or relation tab first")`
- If current overlay is blocking (ExecutionConfirm, busy CatalogDropConfirm, TransactionExitConfirm, RelationTransactionConfirm, ManualCancelConfirm, CatalogEditorDestructiveConfirm): notify `Close the current dialog first` and return
- Otherwise replace dismissible overlays (Help, NotificationHistory, Message, SqlActivity itself, etc.)

`SqlActivityEnter`:

- Verify overlay tab_id still matches an existing Console/Relation tab; else dismiss
- Console: call `open_console_transaction_control()`; if it returns without setting `TransactionExitConfirm` / deferred confirm, set `status_hint` and keep SqlActivity
- Relation: call `open_relation_transaction_control(tab_id, DeferredIntent::Stay)` (or the same path `OpenTransactionControl` uses for relation); keep SqlActivity on refusal

`SqlActivityYank`:

- Pending section: clipboard pending text (Console `pending_transaction_sql` / Relation `transaction_review_sql`)
- Committed section: clipboard `committed_sql_batches[committed_cursor].sql`
- Empty → `notify_warning("Clipboard", "Nothing to copy")`

Dismiss on active tab change if you already have a central tab-switch hook; otherwise check tab_id on each SqlActivity action and dismiss when missing.

- [ ] **Step 4: Run tests — expect pass**

- [ ] **Step 5: Commit**

```bash
git add src/model/workspace.rs src/action.rs src/app.rs
git commit -m "feat(sql-activity): add overlay actions open enter and yank"
```

---

### Task 6: Render the Overlay UI

**Files:**
- Modify: `src/ui/mod.rs` (and `HitTarget` if mouse support is trivial; otherwise keyboard-only v1 is OK)

- [ ] **Step 1: Add a render smoke test if the project has UI render tests; otherwise manual checklist in Step 4**

If no UI test harness is convenient, skip automated UI test and rely on app-state tests + manual verification.

- [ ] **Step 2: Implement `render_sql_activity_overlay`**

Follow `RelationTransactionConfirm` framing:

- Title: ` SQL ACTIVITY `
- Header line: tab title + connection/target if available
- Badges: `PENDING n` / `COMMITTED m batches` + txn state for pending
- Upper box: PENDING body or exact empty strings from `sql_activity` constants
- Lower box: COMMITTED list; collapsed shows timestamp + stmt count + elapsed; expanded shows SQL
- Footer: `Tab sections  j/k move  Enter review/expand  y yank  Esc`

Wire into the main `Overlay::` match in `src/ui/mod.rs`.

- [ ] **Step 3: `cargo check --lib`**

- [ ] **Step 4: Manual verification checklist**

1. Console manual txn with UPDATE → open overlay → PENDING shows SQL  
2. Enter → TransactionExitConfirm (not immediate commit)  
3. Commit → reopen → COMMITTED has batch; PENDING empty state  
4. Relation dirty rows → PENDING from review SQL → Enter → Relation TRANSACTION REVIEW  

- [ ] **Step 5: Commit**

```bash
git add src/ui/mod.rs
git commit -m "feat(sql-activity): render pending and committed overlay"
```

---

### Task 7: Keymap, config, docs

**Files:**
- Modify: `config/default.toml`
- Modify: `src/config.rs` (`SUPPORTED_COMMANDS`)
- Modify: `src/input/keymap.rs`
- Modify: `docs/configuration.md`
- Modify: `src/help.rs` only if globals are enumerated for Help listings

- [ ] **Step 1: Write failing keymap/config test**

```rust
#[test]
fn default_bindings_include_sql_activity() {
    let bindings = /* load defaults the same way other binding tests do */;
    assert!(bindings.contains_key("sql-activity") || bindings.matches(...));
}
```

Follow `default_bindings_match_the_declared_events` style in `src/config.rs`.

- [ ] **Step 2: Run — expect fail**

- [ ] **Step 3: Implement**

`config/default.toml` under `[keybindings.leader]`:

```toml
# Space-a opens SQL Activity for the current console/relation tab.
sql-activity = ["Space a"]
```

`src/config.rs`: add `"sql-activity"` to `SUPPORTED_COMMANDS`.

`keymap.rs`:

- Global/leader match when `app.overlay.is_none()` (or only when replaceable): `Action::OpenSqlActivity`
- When `Overlay::SqlActivity`:
  - `Esc` → dismiss
  - `Tab` → toggle section
  - `j`/`k`/arrows → pending scroll or committed move
  - `Enter`/`Space` → Enter on Pending, toggle expand on Committed
  - `y` → yank

- [ ] **Step 4: Run config/keymap tests + clippy/fmt**

```bash
cargo test --lib default_bindings -- --nocapture
cargo fmt --all -- --check
cargo clippy --lib -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add config/default.toml src/config.rs src/input/keymap.rs docs/configuration.md src/help.rs
git commit -m "feat(sql-activity): bind Space-a and document sql-activity"
```

---

## Spec coverage checklist

| Spec requirement | Task |
|------------------|------|
| PENDING/COMMITTED stacked Overlay | 5, 6 |
| Console `pending_transaction_sql` accumulation rules | 1, 3 |
| Idle ⇒ clear pending invariant | 3 |
| Relation archive on commit | 4 |
| Enter → `open_console_transaction_control` / Relation review | 5 |
| Never `CommitTransaction` from Enter | 5 |
| Yank whole pending / batch | 5 |
| Cap 50 batches | 1 |
| Empty-state copy constants | 1, 6 |
| Keybinding `sql-activity` | 7 |
| Exclude readonly / TransactionControl-only / reads from overlay | 1, 3 |
| Blocking overlay no-op | 5 |

## Placeholder scan

No TBD/TODO steps. Default chord fixed as `Space a`. UI mouse hit targets optional; keyboard is required.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-09-11-sql-activity-overlay.md`.

**Two execution options:**

1. **Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks  
2. **Inline Execution** — execute tasks in this session with executing-plans checkpoints  

Which approach?
