use crate::db::value::CellValue;
use crate::model::relation::{RelationLoad, RelationTab};
use crate::model::relation_edit::{EditableRowState, RelationEditSession};
use crate::model::transaction::TransactionState;

/// Pending SQL for SQL Activity: stored review SQL if present, otherwise a live
/// preview of local dirty edits (before Ctrl-s / TRANSACTION REVIEW).
pub fn activity_pending_sql(tab: &RelationTab) -> String {
    if let Some(sql) = tab
        .transaction_review_sql
        .as_deref()
        .filter(|sql| !sql.trim().is_empty())
    {
        return sql.to_owned();
    }
    let dirty = tab.edit.as_ref().is_some_and(|edit| {
        edit.rows
            .iter()
            .any(|row| !matches!(row.state, EditableRowState::Clean))
    });
    if !dirty && tab.transaction_state == TransactionState::Idle {
        return String::new();
    }
    let Some(edit) = tab.edit.as_ref() else {
        return String::new();
    };
    let snapshot = match &tab.data {
        RelationLoad::Ready(snapshot) => Some(snapshot),
        RelationLoad::Loading { previous, .. }
        | RelationLoad::Failed { previous, .. }
        | RelationLoad::Cancelled { previous } => previous.as_ref(),
        RelationLoad::Empty => None,
    };
    snapshot
        .and_then(|snapshot| snapshot.value.result.result_sets.last())
        .map(|result| {
            let columns = result
                .columns
                .iter()
                .map(|column| column.name.clone())
                .collect::<Vec<_>>();
            let primary_key_columns = match &tab.ddl {
                RelationLoad::Ready(ddl) => {
                    crate::db::mutation::metadata_fingerprint(&ddl.value).primary_key
                }
                _ => Vec::new(),
            };
            preview_sql(edit, tab.title(), &columns, &primary_key_columns)
        })
        .unwrap_or_default()
}

/// Builds a safe, read-only review representation from the local edit session.
/// Execution still goes through the typed mutation requests in `App::relation_save`.
pub fn preview_sql(
    session: &RelationEditSession,
    relation: &str,
    columns: &[String],
    primary_key_columns: &[String],
) -> String {
    let table = quote_qualified_identifier(relation);
    let mut statements = Vec::new();
    for row in &session.rows {
        match &row.state {
            EditableRowState::Updated { changed_columns } => {
                for column in changed_columns {
                    let Some(name) = columns.get(*column) else {
                        continue;
                    };
                    let Some(value) = row.current.get(*column) else {
                        continue;
                    };
                    let predicate = predicate(&row.original, columns, primary_key_columns);
                    statements.push(format!(
                        "UPDATE {table} SET {} = {} WHERE {predicate};",
                        quote_identifier(name),
                        literal(value),
                    ));
                }
            }
            EditableRowState::InsertDraft => {
                let supplied = row
                    .supplied_columns
                    .iter()
                    .filter_map(|column| columns.get(*column).map(|name| (name, *column)))
                    .collect::<Vec<_>>();
                let names = supplied
                    .iter()
                    .map(|(name, _)| quote_identifier(name))
                    .collect::<Vec<_>>();
                let values = supplied
                    .iter()
                    .filter_map(|(_, column)| row.current.get(*column).map(literal))
                    .collect::<Vec<_>>();
                if names.is_empty() {
                    statements.push(format!("INSERT INTO {table} DEFAULT VALUES;"));
                } else {
                    statements.push(format!(
                        "INSERT INTO {table} ({}) VALUES ({});",
                        names.join(", "),
                        values.join(", ")
                    ));
                }
            }
            EditableRowState::Deleted => {
                let predicate = predicate(&row.original, columns, primary_key_columns);
                statements.push(format!("DELETE FROM {table} WHERE {predicate};"));
            }
            _ => {}
        }
    }
    statements.join("\n")
}

pub fn summary(session: &RelationEditSession) -> (usize, usize, usize, usize) {
    let mut updated = 0;
    let mut inserted = 0;
    let mut deleted = 0;
    let mut statements = 0;
    for row in &session.rows {
        match &row.state {
            EditableRowState::Updated { changed_columns } => {
                updated += 1;
                statements += changed_columns.len();
            }
            EditableRowState::InsertDraft => {
                inserted += 1;
                statements += 1;
            }
            EditableRowState::Deleted => {
                deleted += 1;
                statements += 1;
            }
            _ => {}
        }
    }
    (updated, inserted, deleted, statements)
}

fn predicate(values: &[CellValue], columns: &[String], primary_key_columns: &[String]) -> String {
    let indices = if primary_key_columns.is_empty() {
        (0..values.len()).collect::<Vec<_>>()
    } else {
        primary_key_columns
            .iter()
            .filter_map(|name| columns.iter().position(|column| column == name))
            .collect::<Vec<_>>()
    };
    if indices.is_empty() {
        return "/* row locator validated at execution */ 1 = 1".into();
    }
    indices
        .into_iter()
        .filter_map(|index| {
            let column = quote_identifier(columns.get(index)?);
            let value = values.get(index)?;
            Some(match value {
                CellValue::Null => format!("{column} IS NULL"),
                _ => format!("{column} = {}", literal(value)),
            })
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn quote_qualified_identifier(value: &str) -> String {
    value
        .split('.')
        .map(quote_identifier)
        .collect::<Vec<_>>()
        .join(".")
}

fn literal(value: &CellValue) -> String {
    match value {
        CellValue::Null => "NULL".into(),
        CellValue::Boolean(value) => value.to_string().to_ascii_uppercase(),
        CellValue::Integer(value) => value.to_string(),
        CellValue::Unsigned(value) => value.to_string(),
        CellValue::Float(value) => value.to_string(),
        CellValue::Text(value) => format!("'{}'", value.replace('\'', "''")),
        _ => format!("'{}'", value.clipboard_text().replace('\'', "''")),
    }
}

#[cfg(test)]
mod tests {
    use super::preview_sql;
    use crate::db::value::CellValue;
    use crate::model::relation_edit::RelationEditSession;

    #[test]
    fn preview_sql_contains_highlightable_dml_for_local_changes() {
        let mut session = RelationEditSession::from_rows(vec![vec![
            CellValue::Integer(1),
            CellValue::Text("old".into()),
        ]]);
        session.update_cell(0, 1, CellValue::Text("new".into()));
        let sql = preview_sql(
            &session,
            "items",
            &["id".into(), "name".into()],
            &["id".into()],
        );
        assert_eq!(
            sql,
            "UPDATE \"items\" SET \"name\" = 'new' WHERE \"id\" = 1;"
        );
    }

    #[test]
    fn preview_sql_preserves_null_and_default_insert_semantics() {
        let mut session = RelationEditSession::default();
        session.insert_row(0, vec![CellValue::Null, CellValue::Text("x".into())]);
        let sql = preview_sql(&session, "items", &["id".into(), "name".into()], &[]);
        assert!(sql.contains("DEFAULT VALUES") || sql.contains("NULL"));
    }

    #[test]
    fn predicates_use_equality_or_is_null_for_original_values() {
        let columns = vec!["id".into(), "name".into()];
        let values = vec![CellValue::Integer(12), CellValue::Null];
        assert_eq!(
            super::predicate(&values, &columns, &["id".into()]),
            "\"id\" = 12"
        );
        assert_eq!(
            super::predicate(&values, &columns, &[]),
            "\"id\" = 12 AND \"name\" IS NULL"
        );
        assert_eq!(
            super::predicate(&[CellValue::Null], &["name".into()], &[]),
            "\"name\" IS NULL"
        );
    }

    #[test]
    fn predicates_preserve_composite_key_order_and_escaping() {
        let columns = vec!["id".into(), "tenant\"name".into(), "note".into()];
        let values = vec![
            CellValue::Integer(12),
            CellValue::Text("O'Reilly".into()),
            CellValue::Null,
        ];
        assert_eq!(
            super::predicate(&values, &columns, &["tenant\"name".into(), "id".into()],),
            "\"tenant\"\"name\" = 'O''Reilly' AND \"id\" = 12"
        );
    }

    #[test]
    fn preview_sql_simplifies_deletes_and_locates_updates_by_old_key() {
        use crate::model::relation_edit::EditableRowState;

        let mut session = RelationEditSession::from_rows(vec![
            vec![CellValue::Integer(8)],
            vec![CellValue::Integer(1218)],
            vec![CellValue::Integer(12)],
        ]);
        session.rows[0].state = EditableRowState::Deleted;
        session.rows[1].state = EditableRowState::Deleted;
        session.update_cell(2, 0, CellValue::Integer(13));
        assert_eq!(
            preview_sql(&session, "items", &["id".into()], &["id".into()]),
            concat!(
                "DELETE FROM \"items\" WHERE \"id\" = 8;\n",
                "DELETE FROM \"items\" WHERE \"id\" = 1218;\n",
                "UPDATE \"items\" SET \"id\" = 13 WHERE \"id\" = 12;",
            )
        );
    }

    #[test]
    fn summary_counts_rows_and_update_statements_separately() {
        let mut session = RelationEditSession::from_rows(vec![vec![
            CellValue::Integer(1),
            CellValue::Text("old".into()),
        ]]);
        session.update_cell(0, 0, CellValue::Integer(2));
        session.update_cell(0, 1, CellValue::Text("new".into()));
        session.insert_row(1, vec![CellValue::Null, CellValue::Null]);
        assert_eq!(super::summary(&session), (1, 1, 0, 3));
    }
}
