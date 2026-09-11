use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io::{LineWriter, Write};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

#[derive(Clone, Debug, PartialEq)]
pub enum SqlLogOutcome {
    QuerySuccess { rows: usize },
    MutationSuccess { affected_rows: u64 },
    Failure { message: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct SqlLogRecord {
    pub timestamp: chrono::DateTime<chrono::Local>,
    pub connection: String,
    pub target: String,
    pub elapsed: Duration,
    pub outcome: SqlLogOutcome,
    pub sql: String,
}

#[derive(Clone, Debug)]
pub struct SqlLogger {
    sender: Option<UnboundedSender<SqlLogRecord>>,
}

pub fn resolve_log_dir_with(env_var: Option<&OsStr>) -> PathBuf {
    if let Some(val) = env_var
        && !val.is_empty()
    {
        return PathBuf::from(val);
    }
    if let Some(home) = std::env::var_os("HOME")
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|s| !s.is_empty()))
    {
        return PathBuf::from(home).join("logs/lazydb");
    }
    std::env::temp_dir().join("lazydb/logs")
}

pub fn resolve_log_dir() -> PathBuf {
    resolve_log_dir_with(std::env::var_os("LAZYDB_LOG_DIR").as_deref())
}

pub fn get_sanitized_hostname() -> String {
    let raw = std::env::var("HOSTNAME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var("HOST").ok().filter(|s| !s.trim().is_empty()))
        .or_else(|| {
            std::env::var("COMPUTERNAME")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .filter(|s| !s.trim().is_empty())
        })
        .unwrap_or_else(|| "unknown-host".to_string());

    let sanitized: String = raw
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();

    if sanitized.is_empty() {
        "unknown-host".to_string()
    } else {
        sanitized
    }
}

pub fn generate_log_filename_with(
    start_time: chrono::DateTime<chrono::Local>,
    hostname: &str,
    pid: u32,
) -> String {
    format!(
        "lazydb_{}_{}_{}.log",
        start_time.format("%Y%m%d_%H%M%S"),
        hostname,
        pid
    )
}

pub fn generate_log_filename(start_time: chrono::DateTime<chrono::Local>, pid: u32) -> String {
    generate_log_filename_with(start_time, &get_sanitized_hostname(), pid)
}

pub fn format_log_entry(record: &SqlLogRecord) -> String {
    let conn_target = if record.target.is_empty() {
        record.connection.clone()
    } else {
        format!("{}/{}", record.connection, record.target)
    };

    let status_str = match &record.outcome {
        SqlLogOutcome::QuerySuccess { rows } => {
            format!("OK {}ms {} rows", record.elapsed.as_millis(), rows)
        }
        SqlLogOutcome::MutationSuccess { affected_rows } => {
            format!(
                "OK {}ms {} row(s) affected",
                record.elapsed.as_millis(),
                affected_rows
            )
        }
        SqlLogOutcome::Failure { message } => {
            let sanitized_message = message.replace('\n', " ");
            format!(
                "ERROR {}ms: {}",
                record.elapsed.as_millis(),
                sanitized_message
            )
        }
    };

    let header = format!(
        "[{}] [{}] [{}]",
        record.timestamp.format("%Y-%m-%d %H:%M:%S%.3f %:z"),
        conn_target,
        status_str
    );

    format!("{header}\n{}\n\n", record.sql.trim_end())
}

impl SqlLogger {
    pub fn noop() -> Self {
        Self { sender: None }
    }

    pub fn init(base_log_dir: Option<PathBuf>) -> Result<(Self, PathBuf), std::io::Error> {
        let handle = tokio::runtime::Handle::try_current().map_err(std::io::Error::other)?;

        let base_dir = base_log_dir.unwrap_or_else(resolve_log_dir);
        let sql_dir = base_dir.join("sql");
        std::fs::create_dir_all(&sql_dir)?;

        let filename = generate_log_filename(chrono::Local::now(), std::process::id());
        let file_path = sql_dir.join(filename);

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)?;
        let mut writer = LineWriter::new(file);

        let (sender, mut receiver) = unbounded_channel::<SqlLogRecord>();

        handle.spawn(async move {
            while let Some(record) = receiver.recv().await {
                let entry = format_log_entry(&record);
                if writer.write_all(entry.as_bytes()).is_err() {
                    break;
                }
                if writer.flush().is_err() {
                    break;
                }
            }
        });

        Ok((
            Self {
                sender: Some(sender),
            },
            file_path,
        ))
    }

    pub fn log(&self, record: SqlLogRecord) {
        if let Some(sender) = &self.sender {
            let _ = sender.send(record);
        }
    }
}

impl Default for SqlLogger {
    fn default() -> Self {
        Self::noop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_log_dir_prefers_env_var() {
        let custom_path = OsStr::new("/tmp/custom_log_dir");
        let resolved = resolve_log_dir_with(Some(custom_path));
        assert_eq!(resolved, PathBuf::from("/tmp/custom_log_dir"));
    }

    #[test]
    fn resolve_log_dir_defaults_to_home_logs_lazydb() {
        let resolved = resolve_log_dir_with(None);
        if let Some(home) = std::env::var_os("HOME")
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var_os("USERPROFILE").filter(|s| !s.is_empty()))
        {
            assert_eq!(resolved, PathBuf::from(home).join("logs/lazydb"));
            return;
        }
        assert_eq!(resolved, std::env::temp_dir().join("lazydb/logs"));
    }

    #[test]
    fn generate_log_filename_matches_pattern() {
        let now = chrono::Local::now();
        let filename = generate_log_filename_with(now, "my-host", 4242);
        let expected_time = now.format("%Y%m%d_%H%M%S").to_string();
        assert_eq!(filename, format!("lazydb_{expected_time}_my-host_4242.log"));

        let gen_filename = generate_log_filename(now, 100);
        assert!(gen_filename.starts_with(&format!("lazydb_{expected_time}_")));
        assert!(gen_filename.ends_with("_100.log"));
    }

    #[test]
    fn format_record_query_success_preserves_multiline_sql() {
        let timestamp = chrono::Local::now();
        let record = SqlLogRecord {
            timestamp,
            connection: "prod_db".to_string(),
            target: "users".to_string(),
            elapsed: Duration::from_millis(15),
            outcome: SqlLogOutcome::QuerySuccess { rows: 25 },
            sql: "SELECT id, name\n  FROM users\n WHERE active = true;".to_string(),
        };

        let formatted = format_log_entry(&record);
        let ts_str = timestamp.format("%Y-%m-%d %H:%M:%S%.3f %:z").to_string();
        let expected_header = format!("[{ts_str}] [prod_db/users] [OK 15ms 25 rows]\n");
        assert!(formatted.starts_with(&expected_header));
        assert!(formatted.contains("SELECT id, name\n  FROM users\n WHERE active = true;"));
        assert!(formatted.ends_with("WHERE active = true;\n\n"));
    }

    #[test]
    fn format_record_mutation_success() {
        let timestamp = chrono::Local::now();
        let record = SqlLogRecord {
            timestamp,
            connection: "dev_db".to_string(),
            target: "".to_string(),
            elapsed: Duration::from_millis(7),
            outcome: SqlLogOutcome::MutationSuccess { affected_rows: 3 },
            sql: "UPDATE users SET active = false;".to_string(),
        };

        let formatted = format_log_entry(&record);
        let ts_str = timestamp.format("%Y-%m-%d %H:%M:%S%.3f %:z").to_string();
        let expected = format!(
            "[{ts_str}] [dev_db] [OK 7ms 3 row(s) affected]\nUPDATE users SET active = false;\n\n"
        );
        assert_eq!(formatted, expected);
    }

    #[test]
    fn format_record_failure() {
        let timestamp = chrono::Local::now();
        let record = SqlLogRecord {
            timestamp,
            connection: "dev_db".to_string(),
            target: "public".to_string(),
            elapsed: Duration::from_millis(42),
            outcome: SqlLogOutcome::Failure {
                message: "syntax error at or near \"SELCT\"".to_string(),
            },
            sql: "SELCT 1;".to_string(),
        };

        let formatted = format_log_entry(&record);
        let ts_str = timestamp.format("%Y-%m-%d %H:%M:%S%.3f %:z").to_string();
        let expected = format!(
            "[{ts_str}] [dev_db/public] [ERROR 42ms: syntax error at or near \"SELCT\"]\nSELCT 1;\n\n"
        );
        assert_eq!(formatted, expected);
    }

    #[test]
    fn format_record_failure_multiline_sanitized_and_sql_trimmed() {
        let timestamp = chrono::Local::now();
        let record = SqlLogRecord {
            timestamp,
            connection: "dev_db".to_string(),
            target: "".to_string(),
            elapsed: Duration::from_millis(5),
            outcome: SqlLogOutcome::Failure {
                message: "line1\nline2\nline3".to_string(),
            },
            sql: "SELECT 1;\n\n".to_string(),
        };

        let formatted = format_log_entry(&record);
        let ts_str = timestamp.format("%Y-%m-%d %H:%M:%S%.3f %:z").to_string();
        let expected = format!("[{ts_str}] [dev_db] [ERROR 5ms: line1 line2 line3]\nSELECT 1;\n\n");
        assert_eq!(formatted, expected);
    }

    #[tokio::test]
    async fn sql_logger_writes_and_flushes_to_file() {
        let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
        let (logger, log_path) =
            SqlLogger::init(Some(temp_dir.path().to_path_buf())).expect("failed to init logger");

        assert!(log_path.starts_with(temp_dir.path().join("sql")));
        assert!(log_path.exists());

        let record = SqlLogRecord {
            timestamp: chrono::Local::now(),
            connection: "test_conn".to_string(),
            target: "test_db".to_string(),
            elapsed: Duration::from_millis(10),
            outcome: SqlLogOutcome::QuerySuccess { rows: 5 },
            sql: "SELECT * FROM test;".to_string(),
        };

        logger.log(record.clone());

        let expected_entry = format_log_entry(&record);
        let start = tokio::time::Instant::now();
        let timeout = Duration::from_millis(500);
        let mut content = String::new();
        while start.elapsed() < timeout {
            if let Ok(c) = std::fs::read_to_string(&log_path) {
                if c == expected_entry {
                    content = c;
                    break;
                }
                content = c;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert_eq!(content, expected_entry);
    }

    #[test]
    fn sql_logger_init_outside_tokio_returns_error() {
        let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
        let result = SqlLogger::init(Some(temp_dir.path().to_path_buf()));
        assert!(result.is_err());
    }

    #[test]
    fn sql_logger_noop_does_not_panic() {
        let logger = SqlLogger::noop();
        let record = SqlLogRecord {
            timestamp: chrono::Local::now(),
            connection: "test_conn".to_string(),
            target: "test_db".to_string(),
            elapsed: Duration::from_millis(10),
            outcome: SqlLogOutcome::QuerySuccess { rows: 5 },
            sql: "SELECT 1;".to_string(),
        };
        logger.log(record);
    }
}
