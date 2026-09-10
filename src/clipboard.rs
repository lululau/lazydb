use crate::db::catalog::QualifiedName;
use crate::db::{query::ColumnMeta, value::CellValue};
use crate::sql::relation_filter::cell_sql_literal;
use crate::sql::{SqlDialect, quote_identifier};
use base64::{Engine as _, engine::general_purpose::STANDARD};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Osc52Error {
    PayloadTooLarge {
        encoded_bytes: usize,
        max_bytes: usize,
    },
}

impl std::fmt::Display for Osc52Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PayloadTooLarge {
                encoded_bytes,
                max_bytes,
            } => write!(
                f,
                "clipboard payload is too large after encoding ({encoded_bytes} bytes; limit {max_bytes})"
            ),
        }
    }
}

impl std::error::Error for Osc52Error {}

pub fn osc52_sequence(text: &str, max_bytes: usize) -> Result<String, Osc52Error> {
    let encoded = STANDARD.encode(text.as_bytes());
    if encoded.len() > max_bytes {
        return Err(Osc52Error::PayloadTooLarge {
            encoded_bytes: encoded.len(),
            max_bytes,
        });
    }
    Ok(format!("\x1b]52;c;{encoded}\x07"))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClipboardPayload {
    pub text: String,
    pub description: String,
    pub sensitive: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CopyTarget {
    EditorYank,
    EditorStatement,
    EditorBuffer,
    GridCell,
    GridRow { include_headers: bool },
}

pub fn copy_cell(label: &str, value: &CellValue) -> ClipboardPayload {
    let description = if matches!(value, CellValue::Null) {
        format!("cell {label} (NULL as empty value)")
    } else {
        format!("cell {label}")
    };
    ClipboardPayload {
        text: value.clipboard_text(),
        description,
        sensitive: false,
    }
}

pub fn copy_row_tsv(
    columns: &[ColumnMeta],
    row: &[CellValue],
    include_headers: bool,
) -> Option<ClipboardPayload> {
    if columns.is_empty() {
        return None;
    }

    let mut lines = Vec::with_capacity(2);
    if include_headers {
        lines.push(
            columns
                .iter()
                .map(|column| escape_tsv(column.name.clone()))
                .collect::<Vec<_>>()
                .join("\t"),
        );
    }
    lines.push(
        columns
            .iter()
            .enumerate()
            .map(|(index, _)| {
                row.get(index)
                    .map_or_else(String::new, CellValue::clipboard_text)
                    .pipe(escape_tsv)
            })
            .collect::<Vec<_>>()
            .join("\t"),
    );
    Some(ClipboardPayload {
        text: lines.join("\n"),
        description: if include_headers {
            format!("row: {} columns as TSV with headers", columns.len())
        } else {
            format!("row: {} columns as TSV", columns.len())
        },
        sensitive: false,
    })
}

pub fn copy_row_json(
    columns: &[ColumnMeta],
    row: &[CellValue],
) -> Option<ClipboardPayload> {
    if columns.is_empty() {
        return None;
    }
    // Serialize in column order. serde_json::Map sorts keys unless the
    // preserve_order feature is enabled, which this crate does not use.
    let mut parts = Vec::with_capacity(columns.len());
    for (index, column) in columns.iter().enumerate() {
        let value = row.get(index).unwrap_or(&CellValue::Null);
        parts.push(format!(
            "{}:{}",
            serde_json::Value::String(column.name.clone()),
            cell_json_value(value)
        ));
    }
    Some(ClipboardPayload {
        text: format!("{{{}}}", parts.join(",")),
        description: format!("row: {} columns as JSON", columns.len()),
        sensitive: false,
    })
}

pub fn copy_row_insert_sql(
    dialect: SqlDialect,
    qualified_name: &QualifiedName,
    columns: &[ColumnMeta],
    row: &[CellValue],
) -> Option<ClipboardPayload> {
    if columns.is_empty() {
        return None;
    }
    let table = quote_qualified_name(qualified_name, dialect);
    let cols = columns
        .iter()
        .map(|column| quote_identifier(&column.name, dialect))
        .collect::<Vec<_>>()
        .join(", ");
    let values = columns
        .iter()
        .enumerate()
        .map(|(index, _)| {
            cell_sql_literal(row.get(index).unwrap_or(&CellValue::Null), dialect)
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(ClipboardPayload {
        text: format!("INSERT INTO {table} ({cols}) VALUES ({values});"),
        description: format!("row: INSERT INTO {table} ({} columns)", columns.len()),
        sensitive: false,
    })
}

fn quote_qualified_name(name: &QualifiedName, dialect: SqlDialect) -> String {
    let mut parts = Vec::new();
    if let Some(database) = &name.database {
        parts.push(quote_identifier(database, dialect));
    }
    if let Some(schema) = &name.schema {
        parts.push(quote_identifier(schema, dialect));
    }
    parts.push(quote_identifier(&name.object, dialect));
    parts.join(".")
}

fn cell_json_value(value: &CellValue) -> serde_json::Value {
    match value {
        CellValue::Null => serde_json::Value::Null,
        CellValue::Boolean(inner) => serde_json::Value::Bool(*inner),
        CellValue::Integer(inner) => serde_json::json!(*inner),
        CellValue::Unsigned(inner) => serde_json::json!(*inner),
        CellValue::Float(inner) if inner.is_finite() => serde_json::json!(*inner),
        other => serde_json::Value::String(other.clipboard_text()),
    }
}

fn escape_tsv(value: String) -> String {
    if value
        .chars()
        .any(|character| matches!(character, '\t' | '\r' | '\n' | '"'))
    {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value
    }
}

trait Pipe: Sized {
    fn pipe<T>(self, function: impl FnOnce(Self) -> T) -> T {
        function(self)
    }
}

impl<T> Pipe for T {}

#[cfg(test)]
mod tests {
    use super::{
        ClipboardPayload, Osc52Error, copy_cell, copy_row_insert_sql, copy_row_json, copy_row_tsv,
        osc52_sequence,
    };
    use crate::db::{query::ColumnMeta, value::CellValue};

    #[test]
    fn cell_copy_uses_complete_values_instead_of_previews() {
        let value = CellValue::Text("alpha-beta".into());
        assert_eq!(
            copy_cell("users.name", &value),
            ClipboardPayload {
                text: "alpha-beta".into(),
                description: "cell users.name".into(),
                sensitive: false,
            }
        );
    }

    #[test]
    fn tsv_quotes_fields_that_would_break_the_grid_shape() {
        let columns = vec![
            ColumnMeta {
                name: "id".into(),
                type_name: "INT".into(),
            },
            ColumnMeta {
                name: "note".into(),
                type_name: "TEXT".into(),
            },
        ];
        let row = vec![CellValue::Integer(7), CellValue::Text("a\tb\n\"c\"".into())];
        assert_eq!(
            copy_row_tsv(&columns, &row, false).unwrap().text,
            "7\t\"a\tb\n\"\"c\"\"\""
        );
        assert_eq!(
            copy_row_tsv(&columns, &row, true).unwrap().text,
            "id\tnote\n7\t\"a\tb\n\"\"c\"\"\""
        );
    }

    #[test]
    fn null_and_empty_text_are_valid_but_described_differently() {
        let null = copy_cell("users.note", &CellValue::Null);
        let empty = copy_cell("users.note", &CellValue::Text(String::new()));
        assert_eq!(null.text, "");
        assert_eq!(empty.text, "");
        assert!(null.description.contains("NULL as empty value"));
        assert!(!empty.description.contains("NULL"));
    }

    #[test]
    fn bytes_are_complete_uppercase_hex() {
        assert_eq!(
            copy_cell("payload", &CellValue::Bytes(vec![0, 1, 2, 255])).text,
            "0x000102FF"
        );
    }

    #[test]
    fn osc52_encodes_utf8_and_empty_payload() {
        assert_eq!(
            osc52_sequence("你好", 100).unwrap(),
            "\x1b]52;c;5L2g5aW9\x07"
        );
        assert_eq!(osc52_sequence("", 0).unwrap(), "\x1b]52;c;\x07");
    }

    #[test]
    fn osc52_rejects_encoded_payload_over_limit_without_truncating() {
        assert_eq!(
            osc52_sequence("hello", 4),
            Err(Osc52Error::PayloadTooLarge {
                encoded_bytes: 8,
                max_bytes: 4
            })
        );
    }

    #[test]
    fn json_copies_one_object_with_typed_values() {
        let columns = vec![
            ColumnMeta {
                name: "id".into(),
                type_name: "INT".into(),
            },
            ColumnMeta {
                name: "name".into(),
                type_name: "TEXT".into(),
            },
            ColumnMeta {
                name: "note".into(),
                type_name: "TEXT".into(),
            },
            ColumnMeta {
                name: "flag".into(),
                type_name: "BOOL".into(),
            },
        ];
        let row = vec![
            CellValue::Integer(1),
            CellValue::Text("Ada".into()),
            CellValue::Null,
            CellValue::Boolean(true),
        ];
        let payload = copy_row_json(&columns, &row).unwrap();
        assert_eq!(payload.text, r#"{"id":1,"name":"Ada","note":null,"flag":true}"#);
        assert_eq!(payload.description, "row: 4 columns as JSON");
        assert!(!payload.sensitive);
    }

    #[test]
    fn json_returns_none_for_empty_columns() {
        assert!(copy_row_json(&[], &[]).is_none());
    }

    #[test]
    fn json_stringifies_non_finite_floats_and_bytes() {
        let columns = vec![
            ColumnMeta {
                name: "n".into(),
                type_name: "FLOAT".into(),
            },
            ColumnMeta {
                name: "blob".into(),
                type_name: "BYTEA".into(),
            },
        ];
        let row = vec![
            CellValue::Float(f64::INFINITY),
            CellValue::Bytes(vec![0x01, 0xFF]),
        ];
        let payload = copy_row_json(&columns, &row).unwrap();
        let value: serde_json::Value = serde_json::from_str(&payload.text).unwrap();
        assert_eq!(
            value["n"],
            serde_json::Value::String(CellValue::Float(f64::INFINITY).clipboard_text())
        );
        assert_eq!(value["blob"], serde_json::Value::String("0x01FF".into()));
    }

    #[test]
    fn insert_sql_quotes_identifiers_and_literals_per_dialect() {
        use crate::db::catalog::QualifiedName;
        use crate::sql::SqlDialect;

        let columns = vec![
            ColumnMeta {
                name: "id".into(),
                type_name: "INT".into(),
            },
            ColumnMeta {
                name: "name".into(),
                type_name: "TEXT".into(),
            },
            ColumnMeta {
                name: "note".into(),
                type_name: "TEXT".into(),
            },
        ];
        let row = vec![
            CellValue::Integer(1),
            CellValue::Text("O'Hara".into()),
            CellValue::Null,
        ];
        let name = QualifiedName {
            database: None,
            schema: Some("public".into()),
            object: "users".into(),
        };
        let payload =
            copy_row_insert_sql(SqlDialect::Postgres, &name, &columns, &row).unwrap();
        assert_eq!(
            payload.text,
            r#"INSERT INTO "public"."users" ("id", "name", "note") VALUES (1, 'O''Hara', NULL);"#
        );
        assert!(payload.description.contains("INSERT INTO"));
        assert!(payload.description.contains("3 columns"));

        let mysql = copy_row_insert_sql(
            SqlDialect::MySql,
            &QualifiedName {
                database: Some("app".into()),
                schema: None,
                object: "users".into(),
            },
            &columns,
            &row,
        )
        .unwrap();
        assert_eq!(
            mysql.text,
            "INSERT INTO `app`.`users` (`id`, `name`, `note`) VALUES (1, 'O''Hara', NULL);"
        );
    }

    #[test]
    fn insert_sql_returns_none_for_empty_columns() {
        use crate::db::catalog::QualifiedName;
        use crate::sql::SqlDialect;
        assert!(copy_row_insert_sql(
            SqlDialect::Sqlite,
            &QualifiedName {
                database: None,
                schema: None,
                object: "t".into(),
            },
            &[],
            &[]
        )
        .is_none());
    }
}
