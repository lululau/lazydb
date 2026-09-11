# SQL Activity Overlay Design

**Date:** 2026-09-11  
**Status:** Approved for planning  
**Repo:** local clone of `yelog/lazydb`  
**Goal:** Add a one-key TUI overlay that shows, for the **current tab only**, (1) SQL still pending in an open transaction and (2) batches successfully committed earlier in this tab’s process lifetime—aligned with each tab’s existing commit/rollback confirmation flow, without replacing disk SQL execution logs.

---

## 1. Problem & Context

lazydb already has:

- **Relation TRANSACTION REVIEW** (`Overlay::RelationTransactionConfirm`): previews pending mutation SQL, then Commit / Rollback.
- **Console transaction exit** (`Overlay::TransactionExitConfirm`): Commit / Rollback confirmation **without** a pending-SQL statement list today.
- **Console OUTPUT** (in-memory execution transcript for that console).
- **Disk SQL execution logging** (`~/logs/lazydb/sql/…`) covering executed statements across console, relation preview/pagination, derived queries, commits, agent/MCP, etc.

What is missing is a **single, tab-scoped activity surface** that answers:

> In *this* tab, what mutation SQL is still uncommitted, and what have I already successfully committed this session?

Users want that view reachable in one keystroke. Relation already has a review SQL string; Console needs a small **pending-SQL accumulator** so both tabs can share the same overlay chrome.

---

## 2. Requirements & Scope

### 2.1 Functional Requirements

1. **Entry point**
   - A dedicated keymap action (binding id `sql-activity`) opens an overlay for the **active tab**.
   - Supported tab kinds: **SQL Console** and **Relation**.
   - If the active tab is neither (e.g. Dashboard), show a short notification and do not open the overlay.
   - If another overlay is already open: **replace it** with SQL Activity only when the existing overlay is dismissible without losing in-flight destructive confirm state; if the current overlay is a busy/destructive confirm (`CatalogDropConfirm` busy, `ExecutionConfirm`, active `RelationTransactionConfirm` / `TransactionExitConfirm`, etc.), **no-op** and notify `Close the current dialog first`. (Implementation may reuse whatever “overlay replace vs block” pattern Help already uses.)

2. **Semantics (database transaction, not disk log)**
   - **PENDING**: mutation SQL associated with the **currently open** transaction on this tab.
   - **COMMITTED**: successful **Commit** batches recorded for this tab during the current lazydb process lifetime.
   - Rollback clears PENDING only; it does not create a COMMITTED batch.
   - Read-only queries, relation preview/pagination, derived filter/sort pages, and catalog introspection **do not** appear in this overlay (they remain in OUTPUT / disk SQL logs).

3. **Layout**
   - Centered **Overlay** (same family as Help / NotificationHistory / Relation TRANSACTION REVIEW).
   - **Stacked sections**:
     - Upper: **PENDING**
     - Lower: **COMMITTED** (newest batch first)
   - Header shows tab identity (console name or relation title), connection/target label when available, and counts (`PENDING n stmts`, `COMMITTED m batches`).

4. **PENDING section**
   - When a transaction is open and pending SQL is non-empty: show the full pending SQL text and transaction state (e.g. Active / Committing / RollingBack / Aborted).
   - When no transaction is open: empty state copy exactly `No open transaction`.
   - When a transaction is open but pending SQL is empty: empty state copy exactly `Open transaction — no recorded statements yet`.
   - **Enter** on PENDING:
     - Console + open txn: close SQL Activity and open **`Overlay::TransactionExitConfirm`** for this console (same path as the existing commit/rollback confirmation entry, e.g. via the tab’s deferred/transaction-control helper used by `CommitTransaction` / exit flows).
     - Relation + open txn: close SQL Activity and open **`Overlay::RelationTransactionConfirm`** for this relation tab (existing review overlay, including its SQL preview).
     - No open transaction, or open txn with empty pending where Review itself is not offered: **no-op** (optional muted status line; no overlay change).
   - Do **not** commit or roll back directly from SQL Activity.
   - **y**: yank/copy the full pending SQL text to the clipboard; if empty, no-op or brief notification.

5. **COMMITTED section**
   - Each entry is a **batch** produced by one successful Commit:
     - Local timestamp
     - Statement count
     - Optional elapsed duration when available
     - Collapsed by default; expand to show full SQL body
   - Empty state copy exactly `No committed batches in this tab yet`.
   - **y**: yank the **whole focused batch** SQL (v1 has no statement-level focus).
   - History is **in-memory only**, capped at **50 batches** per tab. Oldest batches drop when over capacity.
   - Closing the tab or exiting the process discards history. No persistence to disk.

6. **Keyboard / focus (v1)**
   - `Esc` / existing overlay-dismiss bindings close the overlay.
   - `Tab`: move focus between PENDING and COMMITTED sections.
   - **PENDING navigation**: vertical **scroll only** (`j`/`k` or arrows adjust `pending_scroll`). No per-statement cursor in v1; the pending body is one text block.
   - **COMMITTED navigation**: `j`/`k` or arrows move `committed_cursor` across batches; `Enter` / `Space` toggles expand/collapse for the focused batch.
   - Search (`/`): **out of scope for v1**.

7. **Parity**
   - Console and Relation share one overlay chrome and key model.
   - Data adapters:
     - **Relation**: PENDING from existing `RelationTab.transaction_review_sql`; COMMITTED appended in `relation_transaction_finished` on successful commit when captured review SQL is non-empty.
     - **Console**: PENDING from new `ConsoleTab.pending_transaction_sql` (see §3.1); COMMITTED appended on `Action::ManualCommitted` success when pending SQL captured before clear is non-empty.

### 2.2 Non-Goals (YAGNI)

- Cross-tab or cross-connection global history.
- Surfacing SELECT / preview / pagination / derived-query SQL in this overlay.
- Persisting activity history to disk (disk audit remains `SqlLogger`).
- Committing or rolling back **inside** SQL Activity without going through the existing confirm overlays.
- Replacing Console OUTPUT, `TransactionExitConfirm`, or `RelationTransactionConfirm`.
- Per-statement focus inside PENDING, or in-overlay search, in v1.
- Changing Console `TransactionExitConfirm` itself to show a SQL preview list (SQL Activity is the statement browser; exit confirm stays as today).

### 2.3 Success Criteria

- From a Console tab in an open manual transaction that has executed at least one recorded statement, one keystroke shows that pending SQL in PENDING.
- From a Relation tab with non-empty `transaction_review_sql`, one keystroke shows the same text as Relation TRANSACTION REVIEW.
- After a successful Commit, that SQL appears under COMMITTED; PENDING shows `No open transaction` (and Console pending buffer / Relation review SQL are cleared per existing txn teardown).
- Enter from PENDING opens `TransactionExitConfirm` (Console) or `RelationTransactionConfirm` (Relation); Commit/Rollback behavior of those overlays is unchanged.
- Closing the tab drops that tab’s committed history.

---

## 3. Architecture & Component Design

```
Active Tab (Console | Relation)
        │
        ├─ pending_sql_text() ─────────► PENDING pane (+ scroll)
        │     Console: pending_transaction_sql
        │     Relation: transaction_review_sql
        │
        ├─ committed_sql_batches[] ────► COMMITTED pane
        │
        └─ Enter on PENDING
              Console  → Overlay::TransactionExitConfirm
              Relation → Overlay::RelationTransactionConfirm
```

### 3.1 Model

Shared committed-batch type (names illustrative):

```rust
struct CommittedSqlBatch {
    timestamp: DateTime<Local>,
    sql: String,          // full batch body
    statement_count: usize,
    elapsed: Option<Duration>,
}
```

**RelationTab** (existing + history):

- `transaction_review_sql: Option<String>` — unchanged; PENDING reads this.
- `committed_sql_batches: VecDeque<CommittedSqlBatch>` — newest at front, max 50.

**ConsoleTab** (new pending accumulator + history):

- `pending_transaction_sql: String` — newline-joined statements recorded while `transaction_state` is in an open manual transaction.
- `committed_sql_batches: VecDeque<CommittedSqlBatch>` — newest at front, max 50.

#### Console pending accumulation rules

| Event | Behavior |
|-------|----------|
| Manual transaction becomes Active (begin success / enter manual active) | Ensure buffer starts empty for the new txn |
| `ManualQueryFinished` (or equivalent success path for `ManualExecute`) while txn Active | If the executed draft is **not** classified as read-only-only (i.e. has any non-`ReadOnly` risk, or statement_count mutations), **append** the executed SQL text to `pending_transaction_sql` (separate statements with a blank line). Pure read-only successful queries **do not** append. |
| Failed manual execute | Do not append |
| `ManualCommitted` success | Capture `pending_transaction_sql`; if non-empty, push `CommittedSqlBatch`; then clear pending buffer as part of txn idle transition |
| `ManualRolledBack` success | Clear pending buffer; do not push committed batch |
| Commit/rollback failure | Leave pending buffer unchanged (txn returns to Active / OutcomeUnknown per existing rules) |
| Leaving manual mode / tab close | Clear pending buffer |

Statement counting for a batch: number of non-empty SQL segments in the captured pending text (split on blank-line boundaries), or `max(1, segment_count)` when non-empty.

### 3.2 Overlay state

```rust
enum Overlay {
    // ...
    SqlActivity(SqlActivityState),
}

enum SqlActivitySection {
    Pending,
    Committed,
}

struct SqlActivityState {
    tab_id: Uuid,
    section: SqlActivitySection,
    pending_scroll: u16,       // line offset into pending text block
    committed_cursor: usize,   // index into committed_sql_batches
    expanded: BTreeSet<usize>, // committed batch indices
}
```

Opening the overlay snapshots `tab_id` from the active tab. If the active tab changes or the tab is closed, **dismiss** the overlay.

### 3.3 Append points (committed batches)

| Path | When to append |
|------|----------------|
| Console | `Action::ManualCommitted` success branch — capture `pending_transaction_sql` **before** clear; append if non-empty; then clear |
| Relation | `relation_transaction_finished` on successful **commit** — capture `transaction_review_sql` **before** clear; append if non-empty; then clear as today |

Do **not** append on rollback success/failure, commit failure, or auto-mode statements outside a manual/relation review transaction.

### 3.4 UI module

- Render in `src/ui/` following `RelationTransactionConfirm` / `NotificationHistory` patterns.
- PENDING body: monospace paragraph with `pending_scroll`.
- COMMITTED body: list of batch headers; expanded batch shows SQL under the header.
- Reuse existing clipboard yank helpers.

### 3.5 Keymap

- Binding id `sql-activity`; default chord chosen in the implementation plan (must not collide with existing globals; must be user-configurable).
- Overlay-local map active only while `Overlay::SqlActivity` is focused.

### 3.6 Enter → existing confirm (exact targets)

| Tab | Open condition | Action on Enter |
|-----|----------------|-----------------|
| Console | `transaction_state` indicates an open/controllable txn (Active / Aborted / OutcomeUnknown as allowed by existing commit entry) | Dismiss SQL Activity; invoke the same helper that opens `Overlay::TransactionExitConfirm` for this console (do not invent a new confirm UI) |
| Relation | Relation has an open edit transaction / review-capable state as today | Dismiss SQL Activity; open `Overlay::RelationTransactionConfirm` with current `transaction_review_sql` (same construction as today’s save/review entry) |

---

## 4. Edge Cases & Error Handling

| Case | Behavior |
|------|----------|
| No open transaction | PENDING shows `No open transaction`; Enter no-op |
| Open transaction, empty pending SQL | PENDING shows `Open transaction — no recorded statements yet`; Enter still opens the tab’s existing confirm overlay if that overlay can be opened today without SQL; otherwise no-op |
| Commit with empty pending/review SQL | No COMMITTED batch appended |
| Tab switch / tab close while overlay open | Dismiss overlay |
| History over 50 batches | Drop oldest |
| Yank with empty focus/text | No-op or brief notification |
| Dashboard / unsupported tab | Notification; no overlay |
| Another blocking overlay already open | No-op + `Close the current dialog first` |
| Committing / RollingBack | PENDING still visible (scrollable); Enter no-op until state returns to a controllable confirmable state, matching existing Review gating |

---

## 5. Testing

1. **Unit / app tests**
   - Opening overlay on Console vs Relation vs Dashboard.
   - Console: after manual begin + successful non-readonly `ManualQueryFinished`, PENDING shows appended SQL; readonly execute does not append.
   - Relation: PENDING equals `transaction_review_sql`.
   - Empty states use the exact copy above.
   - `ManualCommitted` / relation commit success append one batch and clear pending source.
   - Rollback clears pending and does not append.
   - Cap at 50 batches.
   - Enter from PENDING opens `TransactionExitConfirm` (Console) or `RelationTransactionConfirm` (Relation).
   - Esc dismisses without side effects.
   - PENDING `j`/`k` only changes scroll; COMMITTED `j`/`k` changes batch cursor.

2. **Manual**
   - Console manual txn: run mutations → open activity → Enter → exit confirm → Commit → reopen activity → batch listed.
   - Relation grid edit → same loop via Relation TRANSACTION REVIEW.
   - Yank pending and committed SQL.
   - Confirm read-only queries do not appear in PENDING/COMMITTED.

---

## 6. Relationship to Existing Features

| Feature | Relationship |
|---------|--------------|
| `RelationTransactionConfirm` | Relation PENDING source + Enter destination |
| `TransactionExitConfirm` | Console Enter destination (unchanged UI; still no SQL list there) |
| `ConsoleTab.pending_transaction_sql` | **New** Console PENDING source |
| Console OUTPUT | Unchanged; broader transcript including reads/errors |
| Disk `SqlLogger` | Unchanged; process-wide audit |
| SQL Activity Overlay | Tab-scoped mutation pending + committed-batch browser |

---

## 7. Open Defaults (accepted)

- History cap: **50** batches per tab.
- Search (`/`): deferred past v1.
- Default key chord: chosen during planning; binding id fixed as `sql-activity`.
- Auto-commit statements outside manual/relation review transactions: **excluded** from PENDING and COMMITTED.
- Console `TransactionExitConfirm` is **not** redesigned to embed SQL preview in v1.
