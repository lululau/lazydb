use std::{path::Path, sync::Mutex, time::Duration};

use lazydb::{
    action::{Action, Command},
    app::App,
    db::{
        query::{ColumnMeta, QueryOutcome, QueryStats, ResultSet},
        value::CellValue,
    },
    logger::{SqlLogOutcome, SqlLogRecord, SqlLogger},
    model::workspace::ConnectionStatus,
    profile::import_connection_url,
};
use regex::Regex;
use tempfile::tempdir;

static ENV_MUTEX: Mutex<()> = Mutex::new(());

async fn wait_for_log_content<F>(log_path: &Path, predicate: F) -> String
where
    F: Fn(&str) -> bool,
{
    let start = tokio::time::Instant::now();
    let timeout = Duration::from_millis(1000);
    let mut last_content = String::new();
    while start.elapsed() < timeout {
        if let Ok(content) = std::fs::read_to_string(log_path) {
            if predicate(&content) {
                return content;
            }
            last_content = content;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    last_content
}

// 1. Integration Test 1: File creation & naming convention
#[tokio::test]
async fn file_creation_and_naming_convention() {
    let temp_dir = tempdir().expect("failed to create temp dir");
    let (_logger, log_path) =
        SqlLogger::init(Some(temp_dir.path().to_path_buf())).expect("failed to init SqlLogger");

    let expected_sql_dir = temp_dir.path().join("sql");
    assert_eq!(
        log_path.parent(),
        Some(expected_sql_dir.as_path()),
        "log file path {:?} should be inside {:?}",
        log_path,
        expected_sql_dir
    );
    assert!(log_path.exists(), "log file {:?} should exist", log_path);

    let filename = log_path
        .file_name()
        .and_then(|name| name.to_str())
        .expect("valid filename utf-8 string");
    let re = Regex::new(r"^lazydb_\d{8}_\d{6}_.+_\d+\.log$").expect("valid regex pattern");
    assert!(
        re.is_match(filename),
        "filename {filename} does not match expected regex"
    );
}

// 2. Integration Test 2: Sequential query logging & line-buffered immediate flushing
#[tokio::test]
async fn sequential_query_logging_and_line_buffered_immediate_flushing() {
    let temp_dir = tempdir().expect("failed to create temp dir");
    let (logger, log_path) =
        SqlLogger::init(Some(temp_dir.path().to_path_buf())).expect("failed to init SqlLogger");

    let record1 = SqlLogRecord {
        timestamp: chrono::Local::now(),
        connection: "prod_db".to_string(),
        target: "public".to_string(),
        elapsed: Duration::from_millis(15),
        outcome: SqlLogOutcome::QuerySuccess { rows: 42 },
        sql: "SELECT id, name, email\n  FROM users\n WHERE active = true;".to_string(),
    };

    let record2 = SqlLogRecord {
        timestamp: chrono::Local::now(),
        connection: "prod_db".to_string(),
        target: "public".to_string(),
        elapsed: Duration::from_millis(8),
        outcome: SqlLogOutcome::MutationSuccess { affected_rows: 5 },
        sql: "UPDATE users\n   SET active = false\n WHERE last_login < '2026-01-01';".to_string(),
    };

    let record3 = SqlLogRecord {
        timestamp: chrono::Local::now(),
        connection: "prod_db".to_string(),
        target: "".to_string(),
        elapsed: Duration::from_millis(3),
        outcome: SqlLogOutcome::Failure {
            message: "syntax error at or near \"FROM\"".into(),
        },
        sql: "SELECT FROM users;".to_string(),
    };

    let record4 = SqlLogRecord {
        timestamp: chrono::Local::now(),
        connection: "prod_db".to_string(),
        target: "".to_string(),
        elapsed: Duration::from_millis(1),
        outcome: SqlLogOutcome::MutationSuccess { affected_rows: 0 },
        sql: "commit;".to_string(),
    };

    // Test immediate flushing for record 1
    logger.log(record1);
    let content1 = wait_for_log_content(&log_path, |c| c.contains("SELECT id, name, email")).await;
    assert!(
        content1.ends_with("\n\n"),
        "entry 1 should be immediately line-buffered and end with double newline"
    );

    // Send the remaining records sequentially
    logger.log(record2);
    logger.log(record3);
    logger.log(record4);

    let final_content = wait_for_log_content(&log_path, |c| c.contains("commit;")).await;

    // Verify double newline separation between entries
    let entries: Vec<&str> = final_content.split("\n\n").collect();
    assert_eq!(
        entries.len(),
        5,
        "expected 4 entries ending with a trailing double newline delimiter, got: {entries:?}"
    );
    assert_eq!(
        entries[4], "",
        "trailing component after final delimiter should be empty"
    );

    let header_re = Regex::new(
        r"^\[(?P<ts>\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{3} [+-]\d{2}:\d{2})\] \[(?P<target>[^\]]+)\] \[(?P<status>[^\]]+)\]$",
    )
    .expect("valid header regex");

    let mut parsed_timestamps = Vec::new();

    // Check each entry format: header, status, exact multiline sql
    for (idx, entry) in entries[..4].iter().enumerate() {
        let (header, sql_body) = entry
            .split_once('\n')
            .unwrap_or_else(|| panic!("entry {idx} should contain a header newline"));

        let caps = header_re
            .captures(header)
            .unwrap_or_else(|| panic!("header {header:?} does not match expected format"));

        let ts_str = caps.name("ts").unwrap().as_str();
        let parsed_ts = chrono::DateTime::parse_from_str(ts_str, "%Y-%m-%d %H:%M:%S%.3f %:z")
            .unwrap_or_else(|err| {
                panic!("timestamp {ts_str} does not match expected format: {err}")
            });
        parsed_timestamps.push(parsed_ts);

        let conn_target = caps.name("target").unwrap().as_str();
        let status = caps.name("status").unwrap().as_str();

        match idx {
            0 => {
                assert_eq!(conn_target, "prod_db/public");
                assert_eq!(status, "OK 15ms 42 rows");
                assert_eq!(
                    sql_body,
                    "SELECT id, name, email\n  FROM users\n WHERE active = true;"
                );
            }
            1 => {
                assert_eq!(conn_target, "prod_db/public");
                assert_eq!(status, "OK 8ms 5 row(s) affected");
                assert_eq!(
                    sql_body,
                    "UPDATE users\n   SET active = false\n WHERE last_login < '2026-01-01';"
                );
            }
            2 => {
                assert_eq!(conn_target, "prod_db");
                assert_eq!(status, "ERROR 3ms: syntax error at or near \"FROM\"");
                assert_eq!(sql_body, "SELECT FROM users;");
            }
            3 => {
                assert_eq!(conn_target, "prod_db");
                assert_eq!(status, "OK 1ms 0 row(s) affected");
                assert_eq!(sql_body, "commit;");
            }
            _ => unreachable!(),
        }
    }

    // Verify all 4 entries appear in correct order
    let select_pos = final_content
        .find("SELECT id, name, email")
        .expect("select pos");
    let update_pos = final_content.find("UPDATE users").expect("update pos");
    let fail_pos = final_content.find("SELECT FROM users;").expect("fail pos");
    let commit_pos = final_content.find("commit;").expect("commit pos");

    assert!(
        select_pos < update_pos && update_pos < fail_pos && fail_pos < commit_pos,
        "entries did not appear in sequential order"
    );
}

// 3. Integration Test 3: Full End-to-End with App
#[tokio::test]
async fn full_end_to_end_with_app() {
    let temp_dir = tempdir().expect("failed to create temp dir");
    let (logger, log_path) =
        SqlLogger::init(Some(temp_dir.path().to_path_buf())).expect("failed to init SqlLogger");

    let profile = import_connection_url("postgres://localhost/kms", Some("kms"))
        .unwrap()
        .profile;
    let profile_id = profile.id;
    let mut app = App::new(vec![profile]).with_sql_logger(logger);
    app.connection.profile_id = Some(profile_id);
    app.connection.generation = 1;
    app.connection.status = ConnectionStatus::Connected;
    app.update(Action::NewConsole);
    app.connection.target = app.active_console().execution_target.clone();

    // 1. Submit and finish a query execution
    let query_sql = "SELECT id, title\n  FROM books\n WHERE published_year > 2020;";
    app.update(Action::ReplaceEditor(query_sql.into()));
    let commands = app.update(Action::RunActiveSql);
    let (tab_id, generation) = match &commands[0] {
        Command::RunQuery {
            tab_id, generation, ..
        }
        | Command::RunQueryPage {
            tab_id, generation, ..
        } => (*tab_id, *generation),
        other => panic!("expected RunQuery/RunQueryPage command, got: {other:?}"),
    };
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
                        type_name: "int4".into(),
                    },
                    ColumnMeta {
                        name: "title".into(),
                        type_name: "text".into(),
                    },
                ],
                rows: vec![vec![
                    CellValue::Integer(1),
                    CellValue::Text("Rust in Action".into()),
                ]],
                affected_rows: 0,
            }],
            stats: QueryStats::new(Duration::from_millis(15), Duration::from_millis(5), 1),
        },
    });

    let content = wait_for_log_content(&log_path, |c| c.contains("SELECT id, title")).await;
    assert!(content.contains("[kms/kms]"));
    assert!(content.contains("[OK 20ms 1 rows]"));
    assert!(content.contains("SELECT id, title\n  FROM books\n WHERE published_year > 2020;"));

    // 2. Submit and fail a query execution
    let fail_sql = "SELECT * FROM non_existent_table;";
    app.update(Action::ReplaceEditor(fail_sql.into()));
    let commands = app.update(Action::RunActiveSql);
    let (tab_id, generation) = match &commands[0] {
        Command::RunQuery {
            tab_id, generation, ..
        }
        | Command::RunQueryPage {
            tab_id, generation, ..
        } => (*tab_id, *generation),
        other => panic!("expected RunQuery/RunQueryPage command, got: {other:?}"),
    };

    app.update(Action::QueryFailed {
        tab_id,
        generation,
        connection,
        message: "relation \"non_existent_table\" does not exist".into(),
    });

    let content_after = wait_for_log_content(&log_path, |c| c.contains("non_existent_table")).await;
    assert!(content_after.contains("[kms/kms]"));
    assert!(content_after.contains("[ERROR 0ms: relation \"non_existent_table\" does not exist]"));
    assert!(content_after.contains("SELECT * FROM non_existent_table;"));
}

// 4. Integration Test 4: LAZYDB_LOG_DIR environment variable resolution
#[tokio::test]
async fn lazydb_log_dir_environment_variable_resolution() {
    let _lock = ENV_MUTEX.lock().unwrap();
    let temp_dir = tempdir().expect("failed to create temp dir");

    struct EnvGuard(&'static str);
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe { std::env::remove_var(self.0) };
        }
    }

    unsafe { std::env::set_var("LAZYDB_LOG_DIR", temp_dir.path()) };
    let _guard = EnvGuard("LAZYDB_LOG_DIR");

    let (_logger, log_path) = SqlLogger::init(None).expect("failed to init SqlLogger with None");

    let expected_sql_dir = temp_dir.path().join("sql");
    assert_eq!(
        log_path.parent(),
        Some(expected_sql_dir.as_path()),
        "log file path {:?} should be inside {:?}",
        log_path,
        expected_sql_dir
    );
    assert!(log_path.exists(), "log file {:?} should exist", log_path);

    let filename = log_path
        .file_name()
        .and_then(|name| name.to_str())
        .expect("valid filename utf-8 string");
    let re = Regex::new(r"^lazydb_\d{8}_\d{6}_.+_\d+\.log$").expect("valid regex pattern");
    assert!(
        re.is_match(filename),
        "filename {filename} does not match expected regex"
    );
}
