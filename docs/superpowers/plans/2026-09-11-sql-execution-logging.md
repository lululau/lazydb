# SQL Execution Logging Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement session-based SQL execution logging for lazydb processes, logging all user-submitted and agent-executed SQL to `$LAZYDB_LOG_DIR/sql/lazydb_${TIME}_${HOSTNAME}_${PID}.log` with local timezone timestamps, execution metadata, raw SQL formatting, line-buffered writing, and non-blocking asynchronous I/O.

**Architecture:**
A dedicated `SqlLogger` module (`src/logger/mod.rs`) resolves the log directory (defaulting to `$HOME/logs/lazydb/sql`), generates the session filename, and spawns a background `tokio` writer worker connected by an unbounded mpsc channel. The writer formats records with local timezone timestamps and execution summaries, writes via `std::io::LineWriter<File>`, and explicitly flushes after each record. Execution points in `src/app.rs` (SQL editor queries, manual transactions, catalog mutations) and `src/agent/service.rs` (Agent/MCP queries) emit `SqlLogRecord` non-blockingly.

**Tech Stack:** Rust 2024 edition, `chrono` (local timezone & formatting), `tokio::sync::mpsc`, `std::io::LineWriter`, `directories`.

**Spec:** `docs/superpowers/specs/2026-09-11-sql-execution-logging-design.md`

**Verification commands:** `cargo test --lib logger`, `cargo test --test sql_logging`, `cargo fmt --all`, `cargo clippy`.

---

## File Map

| File | Responsibility |
|---|---|
| `src/logger/mod.rs` | Core `SqlLogger`, `SqlLogRecord`, `SqlLogOutcome`, path resolution, filename generation, background writer worker |
| `src/lib.rs` | Export `pub mod logger;` |
| `src/app.rs` | Hold `SqlLogger` in `App`, log in `finish_query`, manual query finish/fail, and catalog mutations |
| `src/agent/service.rs` | Hold `SqlLogger` in `AgentService`, log in `query` and `execute` |
| `tests/sql_logging.rs` | End-to-end integration tests verifying file creation, line buffering, and log contents |
| `docs/configuration.md` | Document `LAZYDB_LOG_DIR` environment variable and SQL log format |

---

### Task 1: Create `src/logger/mod.rs` with Path Resolution, Filename Generation, and Formatting

**Files:**
- Create: `src/logger/mod.rs`
- Modify: `src/lib.rs`

- [x] **Step 1: Write the failing tests in `src/logger/mod.rs`**
- [x] **Step 2: Run test to verify failure**
- [x] **Step 3: Implement `src/logger/mod.rs`**
- [x] **Step 4: Run tests to verify they pass**
- [x] **Step 5: Commit**

```bash
git add src/logger/mod.rs src/lib.rs
git commit -m "feat(logger): add core SqlLogger with formatting and path resolution"
```

---

### Task 2: Integrate `SqlLogger` into `App`

**Files:**
- Modify: `src/app.rs`

- [x] **Step 1: Write a test verifying `App` initializes `SqlLogger` and routes executed query events**
- [x] **Step 2: Run test to verify it fails**
- [x] **Step 3: Implement logger integration in `src/app.rs`**
- [x] **Step 4: Run tests to verify they pass**
- [x] **Step 5: Commit**

```bash
git add src/app.rs
git commit -m "feat(app): hook SqlLogger into query execution and transaction completion"
```

---

### Task 3: Integrate `SqlLogger` into `AgentService`

**Files:**
- Modify: `src/agent/service.rs`
- Modify: `src/agent/cli.rs`
- Modify: `src/agent/mcp.rs`

- [ ] **Step 1: Write a test verifying `AgentService` logs executed queries**

Add unit test in `src/agent/service.rs` verifying query execution emits to `SqlLogger`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib agent::service::tests`
Expected: FAIL

- [ ] **Step 3: Implement logger in `AgentService`**

1. Add `sql_logger: crate::logger::SqlLogger` to `AgentService`.
2. In `AgentService::query(...)`, after successful or failed query execution, emit `SqlLogRecord`.
3. In `AgentService::execute(...)`, after mutation execution, emit `SqlLogRecord`.
4. Wire `SqlLogger` initialization when constructing `AgentService` in `src/agent/cli.rs` and `src/agent/mcp.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib agent::service::tests`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/agent/service.rs src/agent/cli.rs src/agent/mcp.rs
git commit -m "feat(agent): record executed SQL in Agent and MCP services"
```

---

### Task 4: End-to-End Integration Test

**Files:**
- Create: `tests/sql_logging.rs`

- [ ] **Step 1: Write integration tests**

Create `tests/sql_logging.rs`:
- Test 1: Set `LAZYDB_LOG_DIR` to a `tempdir`, execute query through `SqlLogger`, verify directory structure `$LAZYDB_LOG_DIR/sql/` and filename format `lazydb_...log`.
- Test 2: Verify line buffering: write entry, verify file content is immediately present and ends with newline.
- Test 3: Multiple entries in sequential order with multi-line SQL formatting preserved.

- [ ] **Step 2: Run integration tests to verify they pass**

Run: `cargo test --test sql_logging`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add tests/sql_logging.rs
git commit -m "test: add integration tests for SQL execution logging"
```

---

### Task 5: Documentation & Formatting

**Files:**
- Modify: `docs/configuration.md`

- [ ] **Step 1: Update documentation**

Document `LAZYDB_LOG_DIR` environment variable, default path `$HOME/logs/lazydb/sql/`, file naming rules, and session logging behavior in `docs/configuration.md`.

- [ ] **Step 2: Format and run full linter/test check**

Run: `cargo fmt --all && cargo clippy --all-targets && cargo test`
Expected: PASS with 0 warnings or errors.

- [ ] **Step 3: Commit**

```bash
git add docs/configuration.md
git commit -m "docs: document LAZYDB_LOG_DIR and SQL execution logging"
```

---

## Execution Handoff

Two execution options:

1. **Subagent-Driven (this session)** - I dispatch fresh subagents per task, review between tasks, fast iteration.
2. **Parallel Session (separate)** - Open new session with executing-plans, batch execution with checkpoints.
