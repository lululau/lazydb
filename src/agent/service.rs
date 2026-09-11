use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    agent::{
        context::{AgentProfileScope, AgentProjectContext, VisibleAgentProfile},
        selection::{AgentError, SelectedAgentProfile, select_profile},
        types::{AgentConnection, AgentContext, AgentQueryResult, AgentTarget, QueryOutcomeJson},
    },
    db::{DatabaseConnection, query::QueryBudget},
    logger::{SqlLogOutcome, SqlLogRecord, SqlLogger},
    persistence::{
        credentials::CredentialResolver, local_credentials::LocalCredentialStore, paths::AppPaths,
        profiles::ProfileStore, secrets::NativeSecretStore,
    },
    profile::ConnectionProfile,
};

pub const DEFAULT_MAX_ROWS: usize = 500;
pub const DEFAULT_MAX_RESULT_BYTES: usize = 1024 * 1024;

pub struct AgentService {
    project: AgentProjectContext,
    profiles: Vec<ConnectionProfile>,
    credential_resolver: CredentialResolver,
    sql_logger: SqlLogger,
    max_rows: usize,
    max_result_bytes: usize,
}

impl AgentService {
    pub fn load(project: Option<&Path>, config: Option<PathBuf>) -> Result<Self, AgentError> {
        let project = AgentProjectContext::resolve(project).map_err(|error| AgentError {
            code: super::selection::AgentErrorCode::NoVisibleConnections,
            message: error.to_string(),
        })?;
        let paths = AppPaths::discover().map_err(|error| AgentError {
            code: super::selection::AgentErrorCode::NoVisibleConnections,
            message: error.to_string(),
        })?;
        let profile_path = config.unwrap_or_else(|| paths.profiles_file());
        let profiles = ProfileStore::new(profile_path)
            .load()
            .map_err(|error| AgentError {
                code: super::selection::AgentErrorCode::NoVisibleConnections,
                message: error.to_string(),
            })?;
        let profiles = profiles.profiles;
        let credentials = CredentialResolver::new(
            Arc::new(NativeSecretStore),
            LocalCredentialStore::from_paths(&paths, "lazydb"),
        );
        let (sql_logger, _) = SqlLogger::init(None)
            .unwrap_or_else(|_| (SqlLogger::noop(), std::path::PathBuf::new()));
        Ok(Self::new(project, profiles, credentials).with_sql_logger(sql_logger))
    }

    pub fn new(
        project: AgentProjectContext,
        profiles: Vec<ConnectionProfile>,
        credential_resolver: CredentialResolver,
    ) -> Self {
        Self {
            project,
            profiles,
            credential_resolver,
            sql_logger: SqlLogger::noop(),
            max_rows: DEFAULT_MAX_ROWS,
            max_result_bytes: DEFAULT_MAX_RESULT_BYTES,
        }
    }

    pub fn with_sql_logger(mut self, logger: SqlLogger) -> Self {
        self.sql_logger = logger;
        self
    }

    pub fn with_limits(mut self, max_rows: usize, max_result_bytes: usize) -> Self {
        self.max_rows = max_rows;
        self.max_result_bytes = max_result_bytes;
        self
    }

    pub fn visible_profiles(&self) -> Vec<VisibleAgentProfile<'_>> {
        self.project.visible_profiles(&self.profiles)
    }

    pub fn project_root(&self) -> &Path {
        self.project.root()
    }

    pub fn select(&self, selector: Option<&str>) -> Result<SelectedAgentProfile<'_>, AgentError> {
        let visible = self.visible_profiles();
        select_profile(&visible, selector)
    }

    pub fn connections(&self) -> Vec<AgentConnection> {
        self.visible_profiles()
            .into_iter()
            .map(|entry| project_connection(entry.profile, entry.scope))
            .collect()
    }

    pub fn context(&self, selector: Option<&str>) -> Result<AgentContext, AgentError> {
        let selected = self.select(selector)?;
        Ok(AgentContext {
            project_root: self.project.root().display().to_string(),
            project_name: self.project.project.display_name.clone(),
            connections: self.connections(),
            selected_connection: selected.profile.id.to_string(),
        })
    }

    fn log_sql(
        &self,
        profile: &ConnectionProfile,
        elapsed: std::time::Duration,
        outcome: SqlLogOutcome,
        sql: &str,
    ) {
        let target = match (&profile.database, &profile.default_schema) {
            (Some(db), Some(schema)) if !schema.is_empty() => format!("{db}.{schema}"),
            (Some(db), _) => db.clone(),
            (None, Some(schema)) => schema.clone(),
            (None, None) => String::new(),
        };
        self.sql_logger.log(SqlLogRecord {
            timestamp: chrono::Local::now(),
            connection: profile.name.clone(),
            target,
            elapsed,
            outcome,
            sql: sql.to_owned(),
        });
    }

    pub async fn query(
        &self,
        selector: Option<&str>,
        sql: &str,
    ) -> Result<AgentQueryResult, AgentError> {
        let selected = self.select(selector)?;
        super::policy::authorize_query(selected.profile, sql).map_err(|error| AgentError {
            code: super::selection::AgentErrorCode::PolicyDenied,
            message: format!("query rejected: {error}"),
        })?;
        let password = self
            .credential_resolver
            .resolve_headless(selected.profile)
            .await
            .map_err(|error| AgentError {
                code: super::selection::AgentErrorCode::CredentialFailure,
                message: error.to_string(),
            })?;
        let start = std::time::Instant::now();
        let connection = DatabaseConnection::connect(selected.profile, password.as_ref())
            .await
            .map_err(|error| {
                self.log_sql(
                    selected.profile,
                    start.elapsed(),
                    SqlLogOutcome::Failure {
                        message: error.to_string(),
                    },
                    sql,
                );
                AgentError {
                    code: super::selection::AgentErrorCode::DatabaseFailure,
                    message: error.to_string(),
                }
            })?;
        let outcome = connection
            .execute_with_budget(
                sql,
                QueryBudget {
                    max_rows: self.max_rows,
                    max_bytes: self.max_result_bytes,
                },
            )
            .await;
        connection.close().await;
        let outcome = outcome.map_err(|error| {
            self.log_sql(
                selected.profile,
                start.elapsed(),
                SqlLogOutcome::Failure {
                    message: error.to_string(),
                },
                sql,
            );
            AgentError {
                code: super::selection::AgentErrorCode::DatabaseFailure,
                message: error.to_string(),
            }
        })?;
        self.log_sql(
            selected.profile,
            outcome.stats.total(),
            SqlLogOutcome::QuerySuccess {
                rows: outcome.stats.row_count,
            },
            sql,
        );
        let json = QueryOutcomeJson::from(outcome);
        Ok(AgentQueryResult {
            target: AgentTarget {
                connection: project_connection(selected.profile, selected.scope),
                schema: selected.profile.default_schema.clone(),
            },
            outcome: json,
        })
    }

    pub async fn search_schema(
        &self,
        selector: Option<&str>,
        query: String,
        limit: usize,
    ) -> Result<super::catalog::SchemaSearchResult, AgentError> {
        let selected = self.select(selector)?;
        let password = self
            .credential_resolver
            .resolve_headless(selected.profile)
            .await
            .map_err(|error| AgentError {
                code: super::selection::AgentErrorCode::CredentialFailure,
                message: error.to_string(),
            })?;
        let connection = DatabaseConnection::connect(selected.profile, password.as_ref())
            .await
            .map_err(|error| AgentError {
                code: super::selection::AgentErrorCode::DatabaseFailure,
                message: error.to_string(),
            })?;
        let target = AgentTarget {
            connection: project_connection(selected.profile, selected.scope),
            schema: selected.profile.default_schema.clone(),
        };
        let result =
            super::catalog::search_schema(&connection, selected.profile, target, query, limit)
                .await;
        connection.close().await;
        result
    }

    pub async fn execute(
        &self,
        selector: Option<&str>,
        sql: &str,
        policy: super::policy::WritePolicy,
    ) -> Result<AgentQueryResult, AgentError> {
        let selected = self.select(selector)?;
        super::policy::authorize_write(selected.profile, policy, sql).map_err(|error| {
            AgentError {
                code: super::selection::AgentErrorCode::PolicyDenied,
                message: format!("write rejected: {error}"),
            }
        })?;
        let password = self
            .credential_resolver
            .resolve_headless(selected.profile)
            .await
            .map_err(|error| AgentError {
                code: super::selection::AgentErrorCode::CredentialFailure,
                message: error.to_string(),
            })?;
        let start = std::time::Instant::now();
        let connection = DatabaseConnection::connect(selected.profile, password.as_ref())
            .await
            .map_err(|error| {
                self.log_sql(
                    selected.profile,
                    start.elapsed(),
                    SqlLogOutcome::Failure {
                        message: error.to_string(),
                    },
                    sql,
                );
                AgentError {
                    code: super::selection::AgentErrorCode::DatabaseFailure,
                    message: error.to_string(),
                }
            })?;
        let result = connection
            .execute_with_budget(
                sql,
                QueryBudget {
                    max_rows: self.max_rows,
                    max_bytes: self.max_result_bytes,
                },
            )
            .await;
        connection.close().await;
        let outcome = result.map_err(|error| {
            self.log_sql(
                selected.profile,
                start.elapsed(),
                SqlLogOutcome::Failure {
                    message: error.to_string(),
                },
                sql,
            );
            AgentError {
                code: super::selection::AgentErrorCode::DatabaseFailure,
                message: error.to_string(),
            }
        })?;
        let affected_rows = outcome
            .result_sets
            .iter()
            .map(|r| r.affected_rows)
            .sum::<u64>();
        self.log_sql(
            selected.profile,
            outcome.stats.total(),
            SqlLogOutcome::MutationSuccess { affected_rows },
            sql,
        );
        Ok(AgentQueryResult {
            target: AgentTarget {
                connection: project_connection(selected.profile, selected.scope),
                schema: selected.profile.default_schema.clone(),
            },
            outcome: QueryOutcomeJson::from(outcome),
        })
    }
}

fn project_connection(profile: &ConnectionProfile, scope: AgentProfileScope) -> AgentConnection {
    AgentConnection {
        id: profile.id.to_string(),
        name: profile.name.clone(),
        scope: match scope {
            AgentProfileScope::CurrentProject => "current_project",
            AgentProfileScope::Global => "global",
        }
        .into(),
        kind: profile.kind,
        environment: profile.environment,
        host: profile.host.clone(),
        port: profile.port,
        database: profile.database.clone(),
        default_schema: profile.default_schema.clone(),
        user: profile.user.clone(),
        read_only: profile.read_only,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        agent::policy::WritePolicy,
        persistence::{
            local_credentials::LocalCredentialStore,
            secrets::{SecretStore, SecretStoreError},
        },
        profile::import_connection_url,
    };
    use secrecy::SecretString;
    use tempfile::TempDir;
    use uuid::Uuid;

    #[derive(Default)]
    struct EmptySecrets;

    #[async_trait::async_trait]
    impl SecretStore for EmptySecrets {
        async fn available(&self) -> Result<(), SecretStoreError> {
            Ok(())
        }
        async fn get(&self, _id: Uuid) -> Result<Option<SecretString>, SecretStoreError> {
            Ok(None)
        }
        async fn set(&self, _id: Uuid, _value: &SecretString) -> Result<(), SecretStoreError> {
            Ok(())
        }
        async fn delete(&self, _id: Uuid) -> Result<(), SecretStoreError> {
            Ok(())
        }
    }

    fn test_service(temp: &TempDir, profiles: Vec<ConnectionProfile>) -> AgentService {
        let context = AgentProjectContext::resolve(Some(temp.path())).unwrap();
        let resolver = CredentialResolver::new(
            Arc::new(EmptySecrets),
            LocalCredentialStore::new(temp.path().join("credential.key"), "test"),
        );
        AgentService::new(context, profiles, resolver)
    }

    fn sqlite_profile(path: &Path, name: &str) -> ConnectionProfile {
        import_connection_url(&format!("sqlite://{}", path.display()), Some(name))
            .unwrap()
            .profile
    }

    async fn wait_for_log_content(path: &Path, expected_substring: &str) -> String {
        let start = tokio::time::Instant::now();
        let timeout = std::time::Duration::from_millis(1500);
        while start.elapsed() < timeout {
            if let Ok(content) = std::fs::read_to_string(path)
                && content.contains(expected_substring)
            {
                return content;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        std::fs::read_to_string(path).unwrap_or_default()
    }

    #[tokio::test]
    async fn test_query_emits_log_record_on_success() {
        let temp = TempDir::new().unwrap();
        std::fs::create_dir(temp.path().join(".git")).unwrap();
        let db_path = temp.path().join("test.db");
        let profile = sqlite_profile(&db_path, "dev-db");

        let setup = DatabaseConnection::connect(&profile, None).await.unwrap();
        setup
            .execute("CREATE TABLE users (id INTEGER, name TEXT); INSERT INTO users VALUES (1, 'Alice'), (2, 'Bob');")
            .await
            .unwrap();
        setup.close().await;

        let (logger, log_path) = SqlLogger::init(Some(temp.path().to_path_buf())).unwrap();
        let service = test_service(&temp, vec![profile]).with_sql_logger(logger);

        let sql = "SELECT id, name FROM users ORDER BY id";
        let result = service.query(None, sql).await.unwrap();
        assert_eq!(result.outcome.row_count, 2);

        let log_content = wait_for_log_content(&log_path, "OK").await;
        assert!(log_content.contains("dev-db"), "content: {log_content}");
        assert!(log_content.contains("OK"), "content: {log_content}");
        assert!(log_content.contains("2 rows"), "content: {log_content}");
        assert!(log_content.contains(sql), "content: {log_content}");
    }

    #[tokio::test]
    async fn test_query_emits_log_record_on_failure() {
        let temp = TempDir::new().unwrap();
        std::fs::create_dir(temp.path().join(".git")).unwrap();
        let db_path = temp.path().join("test.db");
        let profile = sqlite_profile(&db_path, "dev-db");

        let (logger, log_path) = SqlLogger::init(Some(temp.path().to_path_buf())).unwrap();
        let service = test_service(&temp, vec![profile]).with_sql_logger(logger);

        let sql = "SELECT * FROM missing_table";
        let err = service.query(None, sql).await.unwrap_err();
        assert_eq!(
            err.code,
            super::super::selection::AgentErrorCode::DatabaseFailure
        );

        let log_content = wait_for_log_content(&log_path, "ERROR").await;
        assert!(log_content.contains("dev-db"), "content: {log_content}");
        assert!(log_content.contains("ERROR"), "content: {log_content}");
        assert!(log_content.contains(sql), "content: {log_content}");
    }

    #[tokio::test]
    async fn test_query_does_not_log_on_policy_denied() {
        let temp = TempDir::new().unwrap();
        std::fs::create_dir(temp.path().join(".git")).unwrap();
        let db_path = temp.path().join("test.db");
        let profile = sqlite_profile(&db_path, "dev-db");

        let (logger, log_path) = SqlLogger::init(Some(temp.path().to_path_buf())).unwrap();
        let service = test_service(&temp, vec![profile]).with_sql_logger(logger);

        let sql = "UPDATE users SET name = 'eve'";
        let err = service.query(None, sql).await.unwrap_err();
        assert_eq!(
            err.code,
            super::super::selection::AgentErrorCode::PolicyDenied
        );

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let content = std::fs::read_to_string(&log_path).unwrap_or_default();
        assert!(!content.contains("UPDATE users SET name = 'eve'"));
    }

    #[tokio::test]
    async fn test_execute_emits_log_record_on_success() {
        let temp = TempDir::new().unwrap();
        std::fs::create_dir(temp.path().join(".git")).unwrap();
        let db_path = temp.path().join("test.db");
        let profile = sqlite_profile(&db_path, "dev-db");

        let setup = DatabaseConnection::connect(&profile, None).await.unwrap();
        setup
            .execute("CREATE TABLE items (id INTEGER, val TEXT);")
            .await
            .unwrap();
        setup.close().await;

        let (logger, log_path) = SqlLogger::init(Some(temp.path().to_path_buf())).unwrap();
        let service = test_service(&temp, vec![profile]).with_sql_logger(logger);

        let sql = "INSERT INTO items VALUES (1, 'foo'), (2, 'bar');";
        let result = service.execute(None, sql, WritePolicy::All).await.unwrap();
        let affected: u64 = result
            .outcome
            .result_sets
            .iter()
            .map(|r| r.affected_rows)
            .sum();
        assert_eq!(affected, 2);

        let log_content = wait_for_log_content(&log_path, "row(s) affected").await;
        assert!(log_content.contains("dev-db"), "content: {log_content}");
        assert!(log_content.contains("OK"), "content: {log_content}");
        assert!(
            log_content.contains("2 row(s) affected"),
            "content: {log_content}"
        );
        assert!(log_content.contains(sql), "content: {log_content}");
    }

    #[tokio::test]
    async fn test_execute_emits_log_record_on_failure() {
        let temp = TempDir::new().unwrap();
        std::fs::create_dir(temp.path().join(".git")).unwrap();
        let db_path = temp.path().join("test.db");
        let profile = sqlite_profile(&db_path, "dev-db");

        let (logger, log_path) = SqlLogger::init(Some(temp.path().to_path_buf())).unwrap();
        let service = test_service(&temp, vec![profile]).with_sql_logger(logger);

        let sql = "INSERT INTO missing_table VALUES (1);";
        let err = service
            .execute(None, sql, WritePolicy::All)
            .await
            .unwrap_err();
        assert_eq!(
            err.code,
            super::super::selection::AgentErrorCode::DatabaseFailure
        );

        let log_content = wait_for_log_content(&log_path, "ERROR").await;
        assert!(log_content.contains("dev-db"), "content: {log_content}");
        assert!(log_content.contains("ERROR"), "content: {log_content}");
        assert!(log_content.contains(sql), "content: {log_content}");
    }

    #[test]
    fn test_with_sql_logger_builder() {
        let temp = TempDir::new().unwrap();
        let service = test_service(&temp, vec![]);
        let service = service.with_sql_logger(SqlLogger::noop());
        service.sql_logger.log(SqlLogRecord {
            timestamp: chrono::Local::now(),
            connection: "test".to_string(),
            target: "db".to_string(),
            elapsed: std::time::Duration::from_millis(1),
            outcome: SqlLogOutcome::QuerySuccess { rows: 0 },
            sql: "SELECT 1".to_string(),
        });
    }
}
