# SQL Activity Overlay Design

**Date:** 2026-09-11  
**Status:** Approved for planning  
**Repo:** local clone of `yelog/lazydb`  
**Goal:** Add a one-key TUI overlay that shows, for the **current tab only**, (1) SQL still pending in an open transaction and (2) batches successfully committed earlier in this tab’s process lifetime—aligned with TRANSACTION REVIEW, without replacing disk SQL execution logs.

---

## 1. Problem & Context

lazydb already has:

- **TRANSACTION REVIEW** overlays for Console manual transactions and Relation edit transactions (preview pending mutation SQL, then Commit / Rollback).
- **Console OUTPUT** (in-memory execution transcript for that console).
- **Disk SQL execution logging** (`~/logs/lazydb/sql/…`) covering executed statements across console, relation preview/pagination, derived queries, commits, agent/MCP, etc.

What is missing is a **single, tab-scoped activity surface** that answers:

> In *this* tab, what mutation SQL is still uncommitted, and what have I already successfully committed this session?

Users want that view reachable in one keystroke, consistent with TRANSACTION REVIEW semantics—not a global audit browser and not a duplicate of the on-disk log viewer.

---

## 2. Requirements & Scope

### 2.1 Functional Requirements

1. **Entry point**
   - A dedicated keymap action (e.g. binding id `sql-activity`) opens an overlay for the **active tab**.
   - Supported tab kinds: **SQL Console** and **Relation**.
   - If the active tab is neither (e.g. Dashboard), show a short notification and do not open the overlay.

2. **Semantics (database transaction, not disk log)**
   - **PENDING**: mutation SQL associated with the **currently open** transaction on this tab (same source of truth as TRANSACTION REVIEW).
   - **COMMITTED**: successful **Commit** batches recorded for this tab during the current lazydb process lifetime.
   - Rollback clears PENDING only; it does not create a COMMITTED batch.
   - Read-only queries, relation preview/pagination, derived filter/sort pages, and catalog introspection **do not** appear in this overlay (they remain in OUTPUT / disk SQL logs).

3. **Layout**
   - Centered **Overlay** (same family as Help / NotificationHistory / TRANSACTION REVIEW).
   - **Stacked sections**:
     - Upper: **PENDING**
     - Lower: **COMMITTED** (newest batch first)
   - Header shows tab identity (console name or relation title), connection/target label when available, and counts (`PENDING n`, `COMMITTED m batches`).

4. **PENDING section**
   - When a transaction is open: show the review SQL text (statement list) and transaction state (e.g. Active / Committing / RollingBack).
   - When no transaction is open: empty state `No open transaction`.
   - **Enter** on PENDING (when a transaction is open): close this overlay and open the **existing** TRANSACTION REVIEW flow for that tab. Do **not** commit or roll back directly from SQL Activity.
   - **y**: yank/copy the pending SQL (full review text) to the clipboard.

5. **COMMITTED section**
   - Each entry is a **batch** produced by one successful Commit:
     - Local timestamp
     - Statement count
     - Optional elapsed duration when available
     - Collapsed by default; expand to show full SQL body
   - Empty state: `No committed batches in this tab yet`.
   - **y**: yank the focused batch SQL (or the single focused statement if statement-level focus is implemented; default is whole batch).
   - History is **in-memory only**, capped (default **50 batches** per tab). Oldest batches drop when over capacity.
   - Closing the tab or exiting the process discards history. No persistence to disk.

6. **Keyboard / focus**
   - `Esc` / existing overlay-dismiss bindings close the overlay.
   - `Tab`: move focus between PENDING and COMMITTED sections.
   - `j`/`k` or arrows: move within the focused section.
   - `Enter` / `Space` on a COMMITTED batch: toggle expand/collapse.
   - Optional `/`: filter committed batch SQL text (nice-to-have; not required for v1 if it expands scope).

7. **Parity**
   - Console and Relation share one overlay chrome and key model.
   - Data adapters differ:
     - Console: pending from console transaction review SQL / last manual txn statements; committed batches appended on `ManualCommitted` (and equivalent success paths).
     - Relation: pending from `transaction_review_sql`; committed batches appended on successful relation commit in `relation_transaction_finished`.

### 2.2 Non-Goals (YAGNI)

- Cross-tab or cross-connection global history.
- Surfacing SELECT / preview / pagination / derived-query SQL in this overlay.
- Persisting activity history to disk (disk audit remains `SqlLogger`).
- Committing or rolling back **inside** SQL Activity without going through TRANSACTION REVIEW.
- Replacing Console OUTPUT or TRANSACTION REVIEW.

### 2.3 Success Criteria

- From a Console or Relation tab with an open transaction, one keystroke shows pending SQL that matches TRANSACTION REVIEW content.
- After Commit, that SQL appears under COMMITTED; PENDING becomes empty (or shows no open transaction).
- Enter from PENDING opens the existing Review overlay; Commit/Rollback behavior unchanged.
- Closing the tab drops that tab’s committed history.

---

## 3. Architecture & Component Design

```
Active Tab (Console | Relation)
        │
        ├─ pending_sql() ──────────────► PENDING pane
        │     (review SQL / txn state)
        │
        ├─ committed_batches[] ────────► COMMITTED pane
        │     (ring buffer in tab model)
        │
        └─ Enter on PENDING ───────────► existing TRANSACTION REVIEW overlay
```

### 3.1 Model

Add tab-scoped committed history (names illustrative):

```rust
struct CommittedSqlBatch {
    timestamp: DateTime<Local>,
    sql: String,          // full batch body (statements joined as shown in review)
    statement_count: usize,
    elapsed: Option<Duration>,
}

// On ConsoleTab and RelationTab (or shared helper owned by both):
committed_sql_batches: VecDeque<CommittedSqlBatch>, // newest at front, max 50
```

Pending SQL is **not** duplicated long-term: read from existing fields (`transaction_review_sql`, console review/pending txn representation) when opening/rendering the overlay.

### 3.2 Overlay state

```rust
enum Overlay {
    // ...
    SqlActivity(SqlActivityState),
}

struct SqlActivityState {
    tab_id: Uuid,
    section: SqlActivitySection, // Pending | Committed
    pending_cursor: usize,
    committed_cursor: usize,
    expanded: BTreeSet<usize>,   // committed batch indices
    // optional: search draft for v2
}
```

Opening the overlay snapshots `tab_id` from the active tab. If the tab disappears or is no longer Console/Relation, dismiss the overlay.

### 3.3 Append points (committed batches)

| Path | When to append |
|------|----------------|
| Console manual commit success | `Action::ManualCommitted` success path (same place SQL logger records `commit;`) |
| Relation commit success | `relation_transaction_finished` when committing succeeds and review SQL was non-empty |

Capture review SQL **before** clearing `transaction_review_sql` / transaction snapshot. If review SQL is empty, skip appending (nothing meaningful to show).

Do **not** append on:

- Rollback success/failure
- Commit failure
- Auto-mode single statements that never entered a manual/relation review transaction (those stay in OUTPUT / disk log only)

### 3.4 UI module

- Render in `src/ui/` following `RelationTransactionConfirm` / `NotificationHistory` patterns (bordered modal, scrollable body, hit targets if mouse is supported for other overlays).
- Reuse existing clipboard yank helpers where possible.

### 3.5 Keymap

- New binding id, e.g. `sql-activity`, default chord TBD in implementation plan (must not collide with existing globals).
- Overlay-local map active only while `Overlay::SqlActivity` is focused.

---

## 4. Edge Cases & Error Handling

| Case | Behavior |
|------|----------|
| No open transaction | PENDING empty state; Enter does nothing (optional status hint) |
| Open transaction but empty review SQL | PENDING shows empty/placeholder; Enter may still open Review if Review itself allows it |
| Commit with empty review SQL | No COMMITTED batch appended |
| Tab switch while overlay open | Dismiss overlay (tab_id mismatch) or rebind only if product later opts in; **v1: dismiss** |
| History over 50 batches | Drop oldest |
| Yank with empty focus | No-op or brief notification |
| Dashboard / unsupported tab | Notification; no overlay |
| Committing/RollingBack state | PENDING still visible; Enter opens Review only if that flow is valid for current state—otherwise no-op with hint |

---

## 5. Testing

1. **Unit / app tests**
   - Opening overlay on Console vs Relation vs Dashboard.
   - PENDING mirrors review SQL while transaction active.
   - Successful Console/Relation commit appends one batch and clears pending source.
   - Rollback does not append; pending cleared per existing txn semantics.
   - Cap at 50 batches.
   - Enter from PENDING transitions to existing Review overlay action/command path.
   - Esc dismisses without side effects.

2. **Manual**
   - Console manual txn: edit → open activity → Enter → Review → Commit → reopen activity → batch listed.
   - Relation grid edit → same loop.
   - Yank pending and committed SQL.
   - Confirm read-only queries do not appear.

---

## 6. Relationship to Existing Features

| Feature | Relationship |
|---------|--------------|
| TRANSACTION REVIEW | Source of PENDING text; destination for Commit/Rollback via Enter |
| Console OUTPUT | Unchanged; broader execution transcript including reads/errors |
| Disk `SqlLogger` | Unchanged; process-wide audit including reads and commits |
| SQL Activity Overlay | Tab-scoped mutation pending + committed-batch browser only |

---

## 7. Open Defaults (accepted)

- History cap: **50** batches per tab.
- Search (`/`): **deferred** unless implementation is trivial.
- Default keybinding: chosen during planning; must be configurable via keymap.
- Auto-commit statements outside manual/relation review transactions: **excluded** from COMMITTED.
