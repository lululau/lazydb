//! MySQL / MariaDB server version parsing and catalog capability flags.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MySqlFamily {
    OracleMySql,
    MariaDb,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MySqlServerInfo {
    pub family: MySqlFamily,
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub raw: String,
}

impl MySqlServerInfo {
    pub fn parse(version: &str) -> Option<Self> {
        let raw = version.to_owned();
        let lower = version.to_ascii_lowercase();
        let family = if lower.contains("mariadb") {
            MySqlFamily::MariaDb
        } else {
            MySqlFamily::OracleMySql
        };

        let numeric = if family == MySqlFamily::MariaDb {
            lower
                .strip_prefix("5.5.5-")
                .unwrap_or(lower.as_str())
                .split("-mariadb")
                .next()
                .unwrap_or(lower.as_str())
        } else {
            version
        };

        let (major, minor, patch) = parse_version_triplet(numeric)?;
        Some(Self {
            family,
            major,
            minor,
            patch,
            raw,
        })
    }

    pub fn supports_catalog(&self) -> bool {
        match self.family {
            MySqlFamily::OracleMySql => (self.major, self.minor, self.patch) >= (5, 6, 0),
            MySqlFamily::MariaDb => (self.major, self.minor, self.patch) >= (10, 1, 0),
        }
    }

    pub fn triplet(&self) -> (u32, u32, u32) {
        (self.major, self.minor, self.patch)
    }
}

pub fn parse_version_triplet(version: &str) -> Option<(u32, u32, u32)> {
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts
        .next()?
        .split(|character: char| !character.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    Some((major, minor, patch))
}

pub const CATALOG_BEGIN_SNAPSHOT_READ_ONLY: &str =
    "START TRANSACTION WITH CONSISTENT SNAPSHOT, READ ONLY";
pub const CATALOG_BEGIN_SNAPSHOT: &str = "START TRANSACTION WITH CONSISTENT SNAPSHOT";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MySqlCatalogCapabilities {
    pub statistics_expression: bool,
    pub generation_expression: bool,
    pub regexp_replace: bool,
    pub search_cte: bool,
    pub catalog_begin_sql: &'static str,
}

impl MySqlCatalogCapabilities {
    pub fn for_server(info: &MySqlServerInfo) -> Self {
        match info.family {
            MySqlFamily::OracleMySql => {
                let v = info.triplet();
                let is8 = v >= (8, 0, 0);
                Self {
                    statistics_expression: v >= (8, 0, 13),
                    generation_expression: v >= (5, 7, 6),
                    regexp_replace: v >= (8, 0, 4),
                    search_cte: is8,
                    catalog_begin_sql: if is8 {
                        CATALOG_BEGIN_SNAPSHOT_READ_ONLY
                    } else {
                        CATALOG_BEGIN_SNAPSHOT
                    },
                }
            }
            MySqlFamily::MariaDb => {
                let v = info.triplet();
                Self {
                    // Functional-index EXPRESSION column is an Oracle MySQL 8.0.13+ shape.
                    statistics_expression: false,
                    generation_expression: v >= (10, 2, 0),
                    regexp_replace: true,
                    search_cte: v >= (10, 2, 0),
                    catalog_begin_sql: CATALOG_BEGIN_SNAPSHOT,
                }
            }
        }
    }
}

/// Strip non-alphanumeric characters for separator-insensitive catalog search.
pub fn normalize_search_token(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_oracle_mysql_and_mariadb_prefix() {
        let mysql56 = MySqlServerInfo::parse("5.6.16-log").unwrap();
        assert_eq!(mysql56.family, MySqlFamily::OracleMySql);
        assert_eq!((mysql56.major, mysql56.minor, mysql56.patch), (5, 6, 16));
        assert!(mysql56.supports_catalog());

        let mysql57 = MySqlServerInfo::parse("5.7.44").unwrap();
        assert!(mysql57.supports_catalog());

        let mysql8012 = MySqlServerInfo::parse("8.0.12").unwrap();
        assert!(mysql8012.supports_catalog());

        let mysql8013 = MySqlServerInfo::parse("8.0.13").unwrap();
        assert!(mysql8013.supports_catalog());

        let commercial = MySqlServerInfo::parse("8.4.1-commercial").unwrap();
        assert_eq!((commercial.major, commercial.minor, commercial.patch), (8, 4, 1));

        let maria = MySqlServerInfo::parse("10.1.48-MariaDB").unwrap();
        assert_eq!(maria.family, MySqlFamily::MariaDb);
        assert!(maria.supports_catalog());

        let prefixed = MySqlServerInfo::parse("5.5.5-10.11.8-MariaDB").unwrap();
        assert_eq!(prefixed.family, MySqlFamily::MariaDb);
        assert_eq!((prefixed.major, prefixed.minor, prefixed.patch), (10, 11, 8));
        assert!(prefixed.supports_catalog());
    }

    #[test]
    fn rejects_below_floor_and_garbage() {
        assert!(!MySqlServerInfo::parse("5.5.62").unwrap().supports_catalog());
        assert!(!MySqlServerInfo::parse("10.0.38-MariaDB").unwrap().supports_catalog());
        assert!(MySqlServerInfo::parse("not-a-version").is_none());
    }

    #[test]
    fn capabilities_match_contract_matrix() {
        let c56 = MySqlCatalogCapabilities::for_server(&MySqlServerInfo::parse("5.6.16-log").unwrap());
        assert!(!c56.statistics_expression);
        assert!(!c56.generation_expression);
        assert!(!c56.regexp_replace);
        assert!(!c56.search_cte);
        assert_eq!(c56.catalog_begin_sql, CATALOG_BEGIN_SNAPSHOT);

        let c57 = MySqlCatalogCapabilities::for_server(&MySqlServerInfo::parse("5.7.44").unwrap());
        assert!(!c57.statistics_expression);
        assert!(c57.generation_expression);
        assert!(!c57.search_cte);

        let c8013 = MySqlCatalogCapabilities::for_server(&MySqlServerInfo::parse("8.0.13").unwrap());
        assert!(c8013.statistics_expression);
        assert!(c8013.generation_expression);
        assert!(c8013.regexp_replace);
        assert!(c8013.search_cte);
        assert_eq!(c8013.catalog_begin_sql, CATALOG_BEGIN_SNAPSHOT_READ_ONLY);

        let maria101 = MySqlCatalogCapabilities::for_server(
            &MySqlServerInfo::parse("10.1.48-MariaDB").unwrap(),
        );
        assert!(!maria101.statistics_expression);
        assert!(!maria101.search_cte);
        assert_eq!(maria101.catalog_begin_sql, CATALOG_BEGIN_SNAPSHOT);

        let maria102 = MySqlCatalogCapabilities::for_server(
            &MySqlServerInfo::parse("10.2.44-MariaDB").unwrap(),
        );
        assert!(maria102.search_cte);
        assert!(maria102.generation_expression);
        assert!(!maria102.statistics_expression);
    }

    #[test]
    fn normalize_search_token_strips_separators() {
        assert_eq!(normalize_search_token("Foo_Bar-1"), "foobar1");
    }
}
