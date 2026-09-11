# SQL Execution Logging Design

**Date:** 2026-09-11
**Status:** Approved for planning
**Repo:** local clone of `yelog/lazydb`
**Goal:** Implement session-based SQL execution logging for lazydb processes, logging all user-submitted and agent-executed SQL to `$LAZYDB_LOG_DIR/sql/lazydb_${TIME}_${HOSTNAME}_${PID}.log` with local timezone timestamps, execution metadata, raw SQL formatting, line-buffered writing, and non-blocking asynchronous I/O.

---

## 1. Problem & Context

Currently, lazydb records executed SQL only in an ephemeral, in-memory `OUTPUT` view per console tab. This in-memory log is not persisted to disk and is lost when a tab is closed or when lazydb exits.

Users require a persistent, audit-friendly execution log for:
- Tracking what queries and mutations were run in each session.
- Post-mortem analysis and debugging after application exit or abnormal termination.
- Compliance and operational audit trails across terminal sessions and agent/MCP invocations.

---

## 2. Requirements & Scope

### 2.1 Functional Requirements

1. **Session-Level File Isolation**:
   - Each lazydb process constitutes an independent session.
   - The session log file is stored at:
     ```text
     $LAZYDB_LOG_DIR/sql/lazydb_${TIME}_${HOSTNAME}_${PID}.log
     ```
   - If `$LAZYDB_LOG_DIR` is unset or empty, it defaults to:
     ```text
     $HOME/logs/lazydb
     ```
     So the target path defaults to `$HOME/logs/lazydb/sql/lazydb_${TIME}_${HOSTNAME}_${PID}.log`.
   - The directory `$LAZYDB_LOG_DIR/sql/` is created automatically if it does not exist.

2. **Filename Placeholders**:
   - `${TIME}`: Local timestamp when the session starts, in compact format `YYYYMMDD_HHMMSS` (e.g. `20260911_094500`).
   - `${HOSTNAME}`: The machine hostname (e.g. via `gethostname` API or fallback to `HOSTNAME`/`HOST` environment variable, or `"unknown-host"`). Sanitized for filesystem safety.
   - `${PID}`: The operating system process ID (`std::process::id()`).

3. **Execution Scope**:
   - **Included**:
     - SQL executed from the interactive SQL Editor consoles (statements, full buffers, manual transaction queries/commits/rollbacks).
     - Catalog DDL mutations triggered by the user (drop table, table alter, etc.).
     - SQL queries and executions executed via Agent CLI (`lazydb agent query / execute`) or MCP tool calls (`query` / `execute` / `execute_file`).
   - **Excluded**:
     - Internal background queries for catalog discovery, schema introspection, connection health probes, dashboard process list polling, or table pagination/metadata auto-fetch.

4. **Log Format & Timestamps**:
   - Timestamps use the local machine timezone with explicit offset: `YYYY-MM-DD HH:MM:SS.mmm ±HH:MM` (e.g. `2026-09-11 09:45:00.123 +08:00`).
   - Each entry consists of an informative Header line followed by the exact SQL body:
     ```text
     [<Timestamp>] [<Connection>/<Target>] [<Status> <Elapsed> <Stats>]
     <Raw SQL Body>
     <Blank Line>
     ```
   - Status line variations:
     - Read query: `[OK 15ms 120 rows]`
     - Mutation/DML: `[OK 8ms 1 row affected]`
     - Error: `[ERROR 3ms: relation "table" does not exist]`
   - The SQL body retains the original indentation, whitespace, and line breaks.

5. **Buffering & Disk Flushing**:
   - Line/block buffered writing using `LineWriter<File>`.
   - Explicit `flush()` immediately following each completed entry write, ensuring that queries are persisted to disk without delay and survive crashes.

6. **Non-blocking Asynchronous Writing**:
   - File I/O must never block the TUI rendering loop, input event loop, or database query workers.
   - Handled via an asynchronous `tokio::sync::mpsc` queue with a dedicated background writer task.

7. **Fail-Safe Operation**:
   - If log directory creation or file opening fails (e.g. permission denied, disk full), lazydb must:
     - Push a warning notification to the `NotificationCenter` (TUI mode) or write a warning to stderr (CLI/MCP mode).
     - Degrade gracefully to a no-op logger so core database and IDE operations proceed without interruption.

---

## 3. Architecture & Component Design

```
+-------------------------------------------------------------+
|                     Execution Sources                       |
|  - TUI Editor: App::finish_query / ManualQueryFinished      |
|  - DDL Mutations: ExecuteCatalogDrop / Mutation             |
|  - Agent / MCP: AgentService::query / execute               |
+-------------------------------------------------------------+
                              |
                              | SqlLogRecord
                              v
                +----------------------------+
                |    SqlLogger (Sender)      |
                +----------------------------+
                              |
                              | mpsc channel (unbounded)
                              v
                +----------------------------+
                |   Background Writer Task   |
                +----------------------------+
                              |
                              | LineWriter<File>::write_all()
                              | LineWriter<File>::flush()
                              v
                +----------------------------+
                |   Session Log File         |
                |   $LAZYDB_LOG_DIR/sql/     |
                |   lazydb_..._..._....log   |
                +----------------------------+
```

### 3.1 Module Placement
- New module: `src/logger/mod.rs` (or `src/persistence/sql_log.rs`, re-exported via `src/lib.rs`).
- Core types:
  - `SqlLogger`: Handle held by `App` and `AgentService`. Contains `Option<mpsc::UnboundedSender<SqlLogRecord>>`. When `None`, functions as a no-op.
  - `SqlLogRecord`: Represents an execution event.
  - `SqlLogOutcome`: Success (rows / affected) or Failure (error message).

### 3.2 Data Models

```rust
pub enum SqlLogOutcome {
    QuerySuccess { rows: usize },
    MutationSuccess { affected_rows: u64 },
    Failure { message: String },
}

pub struct SqlLogRecord {
    pub timestamp: chrono::DateTime<chrono::Local>,
    pub connection: String,
    pub target: String,
    pub elapsed: std::time::Duration,
    pub outcome: SqlLogOutcome,
    pub sql: String,
}
```

### 3.3 Path & Filename Resolution

```rust
pub fn resolve_log_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("LAZYDB_LOG_DIR").filter(|d| !d.is_empty()) {
        PathBuf::from(dir)
    } else if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        PathBuf::from(home).join("logs/lazydb")
    } else {
        // Fallback for systems without HOME
        std::env::temp_dir().join("lazydb/logs")
    }
}

pub fn generate_log_filename(start_time: chrono::DateTime<chrono::Local>, pid: u32) -> String {
    let time_str = start_time.format("%Y%m%d_%H%M%S");
    let hostname = get_sanitized_hostname();
    format!("lazydb_{time_str}_{hostname}_{pid}.log")
}
```

### 3.4 Hook Points in Codebase

1. **`src/app.rs`**:
   - In `App::finish_query(...)`: query completed in SQL editor.
   - In `Action::ManualQueryFinished` & `Action::ManualQueryFailed`: manual transaction queries.
   - In `Action::ExecuteCatalogMutation` & `Action::ExecuteCatalogDrop`: catalog DDL modifications.
2. **`src/agent/service.rs`**:
   - In `AgentService::query(...)` & `AgentService::execute(...)`: agent/MCP command executions.

---

## 4. Error Handling & Resilience

| Scenario | System Response |
|---|---|
| `$LAZYDB_LOG_DIR` not writable / permission denied | Warning notification posted to Notification Center (TUI) or stderr (CLI); logger disables itself (no-op). |
| Disk space exhausted during session | Writer catches write/flush error, drops remaining writes to prevent panics, session continues uninterrupted. |
| Hostname resolution fails | Fallback to `"unknown-host"` safely. |
| Unclean exit / SIGINT / Panic hook | Previous queries already flushed due to immediate `flush()`; `terminal::install_panic_hook` remains unhindered. |

---

## 5. Testing Strategy

1. **Unit Tests (`src/logger/tests.rs` or `src/persistence/sql_log.rs`)**:
   - Path resolution with `$LAZYDB_LOG_DIR` set vs unset.
   - Filename generation with regex validation (`^lazydb_\d{8}_\d{6}_.+_\d+\.log$`).
   - Record serialization: verify header formatting, timestamp with `%Y-%m-%d %H:%M:%S%.3f %:z`, multi-line SQL preservation, and terminal newline.
   - No-op logger behavior on initialization failure.
2. **Integration Tests (`tests/sql_logging.rs`)**:
   - Run a simulated query session targeting an isolated `tempfile::tempdir()`.
   - Verify that the log file is created in `<tempdir>/sql/`.
   - Verify that multiple queries append in sequence and are readable immediately from disk.

---

## 6. Documentation & Configuration Updates

- Update [`docs/configuration.md`](file:///Users/liuxiang/cascode/github.com/lazydb/docs/configuration.md):
  - Add documentation for `LAZYDB_LOG_DIR` environment variable and `$LAZYDB_LOG_DIR/sql/` session log files.
