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
    risks
        .iter()
        .any(|risk| !matches!(risk, SqlRisk::ReadOnly | SqlRisk::TransactionControl))
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
