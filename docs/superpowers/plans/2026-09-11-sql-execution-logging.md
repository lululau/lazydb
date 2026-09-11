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

- [ ] **Step 1: Write the failing tests in `src/logger/mod.rs`**

Create `src/logger/mod.rs` with test declarations:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::time::Duration;
    use chrono::{TimeZone, Local};

    #[test]
    fn resolve_log_dir_prefers_env_var() {
        let env = Some(OsString::from("/custom/log/dir"));
        let dir = resolve_log_dir_with(env.as_deref());
        assert_eq!(dir, PathBuf::from("/custom/log/dir"));
    }

    #[test]
    fn resolve_log_dir_defaults_to_home_logs_lazydb() {
        let dir = resolve_log_dir_with(None);
        if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
            assert_eq!(dir, PathBuf::from(home).join("logs/lazydb"));
        }
    }

    #[test]
    fn generate_log_filename_matches_pattern() {
        let fixed_time = Local.with_ymd_and_hms(2026, 9, 11, 9, 45, 0).unwrap();
        let name = generate_log_filename_with(fixed_time, "myhost", 12345);
        assert_eq!(name, "lazydb_20260911_094500_myhost_12345.log");
    }

    #[test]
    fn format_record_query_success_preserves_multiline_sql() {
        let fixed_time = Local.with_ymd_and_hms(2026, 9, 11, 9, 45, 0).unwrap();
        let record = SqlLogRecord {
            timestamp: fixed_time,
            connection: "local-pg".into(),
            target: "orders.public".into(),
            elapsed: Duration::from_millis(15),
            outcome: SqlLogOutcome::QuerySuccess { rows: 120 },
            sql: "SELECT id, name\nFROM orders\nWHERE active = true;".into(),
        };

        let formatted = format_log_entry(&record);
        let tz_offset = fixed_time.format("%:z").to_string();
        let expected_header = format!("[2026-09-11 09:45:00.000 {tz_offset}] [local-pg/orders.public] [OK 15ms 120 rows]");
        assert!(formatted.starts_with(&expected_header));
        assert!(formatted.contains("\nSELECT id, name\nFROM orders\nWHERE active = true;\n\n"));
    }

    #[test]
    fn format_record_mutation_success() {
        let fixed_time = Local.with_ymd_and_hms(2026, 9, 11, 9, 45, 0).unwrap();
        let record = SqlLogRecord {
            timestamp: fixed_time,
            connection: "local-pg".into(),
            target: "orders.public".into(),
            elapsed: Duration::from_millis(8),
            outcome: SqlLogOutcome::MutationSuccess { affected_rows: 1 },
            sql: "UPDATE orders SET status = 'done' WHERE id = 1;".into(),
        };

        let formatted = format_log_entry(&record);
        assert!(formatted.contains("[OK 8ms 1 row(s) affected]"));
    }

    #[test]
    fn format_record_failure() {
        let fixed_time = Local.with_ymd_and_hms(2026, 9, 11, 9, 45, 0).unwrap();
        let record = SqlLogRecord {
            timestamp: fixed_time,
            connection: "local-pg".into(),
            target: "orders.public".into(),
            elapsed: Duration::from_millis(3),
            outcome: SqlLogOutcome::Failure { message: "relation \"tbl\" does not exist".into() },
            sql: "SELECT * FROM tbl;".into(),
        };

        let formatted = format_log_entry(&record);
        assert!(formatted.contains("[ERROR 3ms: relation \"tbl\" does not exist]"));
    }
}
```

Register `pub mod logger;` in `src/lib.rs`.

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test --lib logger::tests`
Expected: Compile error (missing structs and functions in `src/logger/mod.rs`).

- [ ] **Step 3: Implement `src/logger/mod.rs`**

Implement in `src/logger/mod.rs`:
- Types:
  - `SqlLogOutcome`: `QuerySuccess { rows: usize }`, `MutationSuccess { affected_rows: u64 }`, `Failure { message: String }`
  - `SqlLogRecord`: `timestamp`, `connection`, `target`, `elapsed`, `outcome`, `sql`
  - `SqlLogger`: holds `Option<tokio::sync::mpsc::UnboundedSender<SqlLogRecord>>`
- Functions:
  - `resolve_log_dir_with(env_var: Option<&OsStr>) -> PathBuf`
  - `resolve_log_dir() -> PathBuf` (delegates using `std::env::var_os("LAZYDB_LOG_DIR")`)
  - `get_sanitized_hostname() -> String`
  - `generate_log_filename_with(start_time: DateTime<Local>, hostname: &str, pid: u32) -> String`
  - `generate_log_filename(start_time: DateTime<Local>, pid: u32) -> String`
  - `format_log_entry(record: &SqlLogRecord) -> String`
  - `SqlLogger::noop() -> Self`
  - `SqlLogger::init(log_dir: Option<PathBuf>) -> Result<(Self, PathBuf), std::io::Error>`: creates dir, opens file with `LineWriter`, spawns `tokio::spawn` loop reading from unbounded channel, writes formatted entry and flushes.
  - `SqlLogger::log(&self, record: SqlLogRecord)`

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib logger::tests`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/logger/mod.rs src/lib.rs
git commit -m "feat(logger): add core SqlLogger with formatting and path resolution"
```

---

### Task 2: Integrate `SqlLogger` into `App`

**Files:**
- Modify: `src/app.rs`

- [ ] **Step 1: Write a test verifying `App` initializes `SqlLogger` and routes executed query events**

Add unit test in `src/app.rs` or `tests/app_flow.rs` testing `finish_query` logging.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib app::tests` (or targeted test)
Expected: Test fails.

- [ ] **Step 3: Implement logger integration in `src/app.rs`**

1. Add `pub sql_logger: crate::logger::SqlLogger` field to `App`.
2. In `App::new`, initialize `SqlLogger`:
   ```rust
   let (sql_logger, log_path) = match crate::logger::SqlLogger::init(None) {
       Ok((logger, path)) => (logger, Some(path)),
       Err(error) => {
           // Post warning notification to notifications
           (crate::logger::SqlLogger::noop(), None)
       }
   };
   ```
3. In `App::finish_query(...)`:
   Construct `SqlLogRecord` from `last.draft` (or connection/target), duration, outcome, and raw SQL, then call `self.sql_logger.log(...)`.
4. In `Action::ManualQueryFinished` & `Action::ManualQueryFailed`:
   Call `self.sql_logger.log(...)` with success / error outcome.
5. In `Action::ExecuteCatalogMutation` & `Action::ExecuteCatalogDrop`:
   When mutation SQL statements execute, log the executed DDL statements.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib app`
Expected: PASS

- [ ] **Step 5: Commit**

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
