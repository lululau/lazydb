# MySQL 5.6+ / MariaDB 10.1+ Catalog Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend LazyDB's MySQL catalog so Explorer, relation Data/DDL, search, and completion work on Oracle MySQL ≥ 5.6 and MariaDB ≥ 10.1, with silent degradation for newer-only metadata, without changing MySQL 8.0.13+ behavior.

**Architecture:** Add a small `mysql_version` module that parses `VERSION()` into family + semver and derives `MySqlCatalogCapabilities`. Catalog entry points use those capabilities to choose SQL variants (index `expression`, generation columns, CTE/`REGEXP_REPLACE` search, transaction begin). Keep a single `MySqlAdapter`; do not add a second adapter.

**Tech Stack:** Rust, sqlx MySQL, existing `tests/mysql_adapter.rs` + unit tests in `src/db/mysql_version.rs`.

**Spec:** `docs/superpowers/specs/2026-09-09-mysql-5x-mariadb-catalog-design.md`

---

## File map

| File | Responsibility |
| --- | --- |
| Create `src/db/mysql_version.rs` | Parse `VERSION()`, minimum gate, capability derivation, begin-SQL constants, search normalize helper |
| Modify `src/db/mod.rs` | `pub mod mysql_version;` (or `pub(crate)`) |
| Modify `src/db/mysql.rs` | Call capabilities at catalog/search entry; branch index/columns/search/begin SQL; keep 8.0 path default |
| Modify `tests/mysql_adapter.rs` | Replace 8.0.13-only / MariaDB-reject assertions; add variant SQL string tests |
| Modify `README.md`, `docs/database-capabilities.md`, `docs/architecture.md` | New catalog contract wording |

---

### Task 1: Version parse + catalog floor

**Files:**
- Create: `src/db/mysql_version.rs`
- Modify: `src/db/mod.rs`
- Test: unit tests inside `src/db/mysql_version.rs`

- [ ] **Step 1: Write failing tests for parse + floor**

Create `src/db/mysql_version.rs` with tests first (types can be stubbed so the file compiles, or write tests that call APIs you are about to add):

```rust
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
}
```

Add to `src/db/mod.rs` next to `pub mod mysql;`:

```rust
pub mod mysql_version;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p lazydb mysql_version::tests -- --nocapture`

Expected: compile failure or missing `MySqlServerInfo` / failing asserts.

- [ ] **Step 3: Implement parse + floor**

```rust
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
                .unwrap_or(version)
                .split("-mariadb")
                .next()
                .unwrap_or(version)
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
```

Keep MariaDB prefix handling robust: if `contains("mariadb")` and string starts with `5.5.5-`, strip that prefix before parsing the triplet from the remainder (stop at first non-version suffix).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p lazydb mysql_version:: -- --nocapture`

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/db/mysql_version.rs src/db/mod.rs
git commit -m "feat(mysql): parse Oracle MySQL and MariaDB VERSION() strings"
```

---

### Task 2: Catalog capabilities derivation

**Files:**
- Modify: `src/db/mysql_version.rs`
- Test: same module tests

- [ ] **Step 1: Write failing capability tests**

```rust
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
```

- [ ] **Step 2: Run tests — expect FAIL**

Run: `cargo test -p lazydb mysql_version::tests::capabilities_match_contract_matrix -- --nocapture`

- [ ] **Step 3: Implement capabilities**

```rust
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
```

- [ ] **Step 4: Run tests — expect PASS**

Run: `cargo test -p lazydb mysql_version:: -- --nocapture`

- [ ] **Step 5: Commit**

```bash
git add src/db/mysql_version.rs
git commit -m "feat(mysql): derive catalog SQL capabilities from server version"
```

---

### Task 3: Replace catalog version gate in `mysql.rs`

**Files:**
- Modify: `src/db/mysql.rs` (gate helpers ~3105–3136 and call sites ~584–590, ~669–675)
- Modify: `tests/mysql_adapter.rs` (`mysql_catalog_requires_oracle_mysql_8_0_13`)

- [ ] **Step 1: Update the failing/obsolete integration-style unit test**

Replace `mysql_catalog_requires_oracle_mysql_8_0_13` with:

```rust
#[test]
fn mysql_catalog_accepts_mysql_5_6_and_mariadb_10_1() {
    assert!(mysql::supports_catalog_version("5.6.16-log"));
    assert!(mysql::supports_catalog_version("5.7.44"));
    assert!(mysql::supports_catalog_version("8.0.12"));
    assert!(mysql::supports_catalog_version("8.0.13"));
    assert!(mysql::supports_catalog_version("8.4.1-commercial"));
    assert!(mysql::supports_catalog_version("10.1.48-MariaDB"));
    assert!(mysql::supports_catalog_version("10.11.8-MariaDB"));
    assert!(mysql::supports_catalog_version("5.5.5-10.11.8-MariaDB"));
    assert!(!mysql::supports_catalog_version("5.5.62"));
    assert!(!mysql::supports_catalog_version("10.0.38-MariaDB"));
    assert!(!mysql::supports_catalog_version("nope"));
}
```

- [ ] **Step 2: Run test — expect FAIL** (old gate still rejects 5.7/MariaDB)

Run: `cargo test -p lazydb mysql_catalog_accepts_mysql_5_6_and_mariadb_10_1 -- --nocapture`

- [ ] **Step 3: Rewire gate helpers**

In `src/db/mysql.rs`:

1. Import: `use crate::db::mysql_version::{MySqlCatalogCapabilities, MySqlServerInfo};`
2. Replace `supports_catalog_version` / `unsupported_catalog_version` / local `parse_version_triplet` with:

```rust
pub fn supports_catalog_version(version: &str) -> bool {
    MySqlServerInfo::parse(version).is_some_and(|info| info.supports_catalog())
}

fn unsupported_catalog_version(version: &str) -> DatabaseError {
    DatabaseError {
        category: ErrorCategory::Unsupported,
        code: Some("mysql_catalog_version_unsupported".to_owned()),
        message: sanitize_terminal_text(&format!(
            "MySQL catalog requires Oracle MySQL 5.6+ or MariaDB 10.1+; server reported {version}"
        )),
        diagnostic: None,
    }
}

fn catalog_capabilities(version: &str) -> Result<MySqlCatalogCapabilities, DatabaseError> {
    let info = MySqlServerInfo::parse(version).ok_or_else(|| unsupported_catalog_version(version))?;
    if !info.supports_catalog() {
        return Err(unsupported_catalog_version(version));
    }
    Ok(MySqlCatalogCapabilities::for_server(&info))
}
```

3. At both catalog entry sites, replace the gate + hard-coded begin SQL:

```rust
let version: String = sqlx::query_scalar("SELECT VERSION()")
    .fetch_one(&mut *connection)
    .await
    .map_err(sql_error)?;
let capabilities = catalog_capabilities(&version)?;
let mut transaction = connection
    .begin_with(capabilities.catalog_begin_sql)
    .await
    .map_err(sql_error)?;
```

Pass `capabilities` into snapshot helpers that need it (see later tasks). For this task, if helpers do not take capabilities yet, store them in a local and only use `catalog_begin_sql` so the gate + begin path compiles; thread the struct in Tasks 4–6.

Also update the third `begin_with(CATALOG_PAGE_BEGIN_SQL)` around relation DDL (~1712) the same way if that path also selects `VERSION()` / assumes 8.0. If it currently has **no** version gate, still switch begin SQL via a local `VERSION()` + `catalog_capabilities` so 5.6 does not hit the combined READ ONLY form.

Keep `CATALOG_PAGE_BEGIN_SQL` as an alias to the 8.0 constant for tests that assert the modern string, or update those tests to import `mysql_version::CATALOG_BEGIN_SNAPSHOT_READ_ONLY`.

- [ ] **Step 4: Run tests**

Run:

```bash
cargo test -p lazydb mysql_catalog_accepts_mysql_5_6_and_mariadb_10_1 -- --nocapture
cargo test -p lazydb --test mysql_adapter -- --nocapture
```

Expected: new gate test PASS; fix any begin-SQL string assertions that assumed a single constant.

- [ ] **Step 5: Commit**

```bash
git add src/db/mysql.rs tests/mysql_adapter.rs
git commit -m "feat(mysql): accept MySQL 5.6+ and MariaDB 10.1+ for catalog"
```

---

### Task 4: Index SQL without `expression`

**Files:**
- Modify: `src/db/mysql.rs` (`CATALOG_PAGE_INDEXES_SQL`, `load_index_metadata`)
- Modify: `tests/mysql_adapter.rs` (string contains `expression` assertions)

- [ ] **Step 1: Add failing test for legacy index SQL**

```rust
#[test]
fn mysql_index_sql_omits_expression_without_capability() {
    let legacy = mysql::catalog_page_indexes_sql(false);
    assert!(legacy.contains("column_name"));
    assert!(!legacy.to_ascii_lowercase().contains("expression"));

    let modern = mysql::catalog_page_indexes_sql(true);
    assert!(modern.to_ascii_lowercase().contains("expression"));
}
```

- [ ] **Step 2: Run — expect FAIL** (function missing)

- [ ] **Step 3: Implement SQL selector + decode**

Replace the single constant usage with:

```rust
pub fn catalog_page_indexes_sql(statistics_expression: bool) -> &'static str {
    if statistics_expression {
        r#"SELECT index_name, non_unique, seq_in_index, column_name, expression
FROM information_schema.statistics
WHERE BINARY table_schema=BINARY ? AND BINARY table_name=BINARY ?
ORDER BY BINARY index_name, seq_in_index"#
    } else {
        r#"SELECT index_name, non_unique, seq_in_index, column_name
FROM information_schema.statistics
WHERE BINARY table_schema=BINARY ? AND BINARY table_name=BINARY ?
ORDER BY BINARY index_name, seq_in_index"#
    }
}
```

Keep `pub const CATALOG_PAGE_INDEXES_SQL` as the modern variant for backward-compatible tests, or point tests at `catalog_page_indexes_sql(true)`.

Update `load_index_metadata` to take `&MySqlCatalogCapabilities` (or `statistics_expression: bool`) and map rows:

```rust
expression: if capabilities.statistics_expression {
    row.try_get(4).map_err(decode_error)?
} else {
    None
},
```

Thread capabilities from `load_catalog_page` / `load_relation_children` into `load_index_metadata`. Prefer resolving capabilities once per catalog request and passing `&MySqlCatalogCapabilities` down; avoid a second `VERSION()` query inside index loading.

- [ ] **Step 4: Run tests**

```bash
cargo test -p lazydb mysql_index_sql_omits_expression_without_capability -- --nocapture
cargo test -p lazydb --test mysql_adapter -- --nocapture
```

- [ ] **Step 5: Commit**

```bash
git add src/db/mysql.rs tests/mysql_adapter.rs
git commit -m "feat(mysql): omit statistics.expression on pre-8.0.13 catalog indexes"
```

---

### Task 5: Column metadata without `generation_expression`

**Files:**
- Modify: `src/db/mysql.rs` (column SELECT ~1395–1436)
- Test: add unit test for SQL selector if extracted; otherwise cover via capability + decode helper test

- [ ] **Step 1: Write failing test for column SQL variants**

```rust
#[test]
fn mysql_column_sql_omits_generation_expression_when_unavailable() {
    let legacy = mysql::relation_columns_sql(false);
    assert!(legacy.contains("column_default"));
    assert!(legacy.contains("extra"));
    assert!(!legacy.contains("generation_expression"));

    let modern = mysql::relation_columns_sql(true);
    assert!(modern.contains("generation_expression"));
}
```

- [ ] **Step 2: Run — expect FAIL**

- [ ] **Step 3: Implement**

```rust
pub fn relation_columns_sql(generation_expression: bool) -> &'static str {
    if generation_expression {
        "SELECT ordinal_position, column_name, column_type, data_type, is_nullable, \
         column_default, extra, generation_expression, numeric_precision, numeric_scale, \
         character_maximum_length, collation_name, character_set_name, column_comment \
         FROM information_schema.columns WHERE BINARY table_schema=BINARY ? AND BINARY table_name=BINARY ? \
         ORDER BY ordinal_position"
    } else {
        "SELECT ordinal_position, column_name, column_type, data_type, is_nullable, \
         column_default, extra, numeric_precision, numeric_scale, \
         character_maximum_length, collation_name, character_set_name, column_comment \
         FROM information_schema.columns WHERE BINARY table_schema=BINARY ? AND BINARY table_name=BINARY ? \
         ORDER BY ordinal_position"
    }
}
```

Adjust ordinal indexes in the decode loop when `generation_expression` is absent:

- Still read `extra`.
- Set `generation_expression` local to `String::new()`.
- Keep existing `EXTRA` contains `VIRTUAL GENERATED` / `STORED GENERATED` detection.
- Shift indexes for precision/scale/length/collation/charset/comment by −1 when the column is omitted (or use named `try_get("column_name")` style if the query aliases allow — prefer stable named gets if already used elsewhere; this file currently uses ordinals, so document the mapping in a small comment).

Thread `capabilities.generation_expression` from the relation-children / hydrate path.

- [ ] **Step 4: Run tests**

```bash
cargo test -p lazydb mysql_column_sql_omits_generation_expression_when_unavailable -- --nocapture
cargo test -p lazydb --test mysql_adapter -- --nocapture
```

- [ ] **Step 5: Commit**

```bash
git add src/db/mysql.rs tests/mysql_adapter.rs
git commit -m "feat(mysql): load column metadata without generation_expression on 5.6"
```

---

### Task 6: Catalog search without CTE / `REGEXP_REPLACE`

**Files:**
- Modify: `src/db/mysql.rs` (`CATALOG_SEARCH_CANDIDATES_SQL`, `search_catalog_snapshot`)
- Modify: `tests/mysql_adapter.rs` (search SQL assertions)

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn mysql_search_sql_has_modern_and_legacy_shapes() {
    let modern = mysql::catalog_search_candidates_sql(true, true, true);
    assert!(modern.contains("WITH candidates AS"));
    assert!(modern.contains("REGEXP_REPLACE"));

    let legacy = mysql::catalog_search_candidates_sql(false, false, false);
    assert!(!legacy.contains("WITH candidates AS"));
    assert!(!legacy.contains("REGEXP_REPLACE"));
    assert!(legacy.contains("UNION ALL"));
    assert!(legacy.contains("{scope_predicate}"));
}
```

- [ ] **Step 2: Run — expect FAIL**

- [ ] **Step 3: Implement legacy search SQL + optional Rust normalize**

Provide `catalog_search_candidates_sql(search_cte: bool, regexp_replace: bool, ignore_separators: bool) -> &'static str` (modern vs legacy-locate vs legacy-scope templates):

**Modern:** keep current `CATALOG_SEARCH_CANDIDATES_SQL` (CTE + `REGEXP_REPLACE`).

**Legacy (no CTE, no `REGEXP_REPLACE`):** wrap the same `UNION ALL` arms in a derived table.

When `ignore_separators` is true, legacy search must **scope-filter in SQL and match in Rust** — do **not** `LOCATE` a normalized needle against raw names (that drops matches like needle `foobar` vs name `foo_bar`). Use scope-only SQL (no text `LOCATE`), fetch in-scope candidates (higher `LIMIT`, e.g. 5000), then filter+rank in Rust with `catalog::normalize_search_text` / haystacks. Cap at 101 after filter.

When `ignore_separators` is false (empty normalized query edge case), `LOCATE` on lowercase raw needle against `LOWER(name/path)` is OK:

```sql
SELECT kind, database_name, object_name, relation_name, relation_type, native_identity, comment
FROM (
    /* same UNION ALL arms as modern candidates, without normalized/REGEXP_REPLACE CTEs */
) AS candidates
WHERE {scope_predicate}
  AND database_name NOT IN ('information_schema','mysql','performance_schema','sys')
  AND (
        LOCATE(?, LOWER(object_name)) > 0
     OR LOCATE(?, LOWER(IFNULL(qualified_path, ''))) > 0
      )
ORDER BY LOWER(IFNULL(qualified_path, object_name)), kind, BINARY native_identity
LIMIT 101
```

In `search_catalog_snapshot`:

1. Build SQL from capabilities (`search_cte` / `regexp_replace`) and `ignore_separators`.
2. If modern: keep existing bind pattern (`ignore_separators` twice + scope + five search binds).
3. If legacy:
   - Bind scope databases as today.
   - If `!ignore_separators`: bind `search_query.to_ascii_lowercase()` twice for the two `LOCATE` predicates.
   - If `ignore_separators`: no text binds; after fetch filter/rank in Rust (exact normalized → prefix → contains). Cap at 101 after filter.
4. Recompute search haystacks in Rust from fields already on `MySqlSearchCandidate`. Include `relation_name` when present for relation children (`db.relation.name`), matching modern `qualified_path` shape:

```rust
fn candidate_search_haystacks(candidate: &MySqlSearchCandidate) -> [String; 2] {
    let name = candidate.name.to_ascii_lowercase();
    let path = match candidate.kind {
        CatalogKind::Database | CatalogKind::Schema => candidate.database.to_ascii_lowercase(),
        _ if candidate.kind.is_relation_child() => match &candidate.relation_name {
            Some(relation) => format!("{}.{}.{}", candidate.database, relation, candidate.name)
                .to_ascii_lowercase(),
            None => format!("{}.{}", candidate.database, candidate.name).to_ascii_lowercase(),
        },
        _ => format!("{}.{}", candidate.database, candidate.name).to_ascii_lowercase(),
    };
    [name, path]
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p lazydb mysql_search_sql_has_modern_and_legacy_shapes -- --nocapture
cargo test -p lazydb --test mysql_adapter -- --nocapture
cargo test -p lazydb mysql_version:: -- --nocapture
```

- [ ] **Step 5: Commit**

```bash
git add src/db/mysql.rs src/db/mysql_version.rs tests/mysql_adapter.rs
git commit -m "feat(mysql): legacy catalog search without CTE or REGEXP_REPLACE"
```

---

### Task 7: Docs + message consistency

**Files:**
- Modify: `README.md` (Supported Databases table)
- Modify: `docs/database-capabilities.md`
- Modify: `docs/architecture.md`
- Modify: any docs tests that pin `8.0.13` / `MariaDB is rejected` (search with `rg`)

- [ ] **Step 1: Find stale wording**

```bash
rg -n '8\.0\.13|MariaDB is not|rejects MariaDB|Oracle MySQL 8' README.md docs tests
```

- [ ] **Step 2: Update docs to the new contract**

README / capabilities table row should read approximately:

> Oracle MySQL 5.6+ or MariaDB 10.1+ — Databases, tables, views, functions, procedures, triggers. Newer-only metadata (for example functional index expressions) is omitted on older servers.

Architecture paragraph: replace “requires 8.0.13 or newer and rejects MariaDB” with the capability-layer description.

- [ ] **Step 3: Run docs-related tests if present**

```bash
cargo test -p lazydb --test docs -- --nocapture
```

If no `docs` test, skip.

- [ ] **Step 4: Commit**

```bash
git add README.md docs/database-capabilities.md docs/architecture.md
git commit -m "docs: expand MySQL catalog contract to 5.6+ and MariaDB 10.1+"
```

---

### Task 8: Regression + manual smoke

**Files:** none required unless fixes fall out

- [ ] **Step 1: Full MySQL adapter + version unit tests**

```bash
cargo test -p lazydb mysql_version:: -- --nocapture
cargo test -p lazydb --test mysql_adapter -- --nocapture
```

Expected: PASS (MySQL 8 live fixtures still green when env available).

- [ ] **Step 2: Manual smoke against local profiles (when VPN/network allows)**

```bash
lazydb agent query --project /Users/liuxiang/global-claude --connection 'UEOS MySQL Dev' --sql 'SELECT VERSION()'
lazydb agent query --project /Users/liuxiang/global-claude --connection 'Jicai Dev' --sql 'SELECT VERSION()'
```

Then open TUI (`cargo run` or installed binary with `--config` pointing at profiles) and confirm Explorer expands on 5.7 / 5.6 without the old catalog error.

- [ ] **Step 3: Final commit only if fixes were needed**; otherwise note smoke results in the PR body.

---

## Spec coverage check

| Spec requirement | Task |
| --- | --- |
| Parse family + MariaDB `5.5.5-` prefix | Task 1 |
| Minimum MySQL 5.6 / MariaDB 10.1 gate | Tasks 1, 3 |
| Capability flags matrix | Task 2 |
| Begin SQL variant | Tasks 2, 3 |
| Index `expression` silent omit | Task 4 |
| `generation_expression` silent omit | Task 5 |
| Search without CTE/`REGEXP_REPLACE` | Task 6 |
| Docs / upstream-facing contract | Task 7 |
| Keep MySQL 8 behavior / regression | Tasks 3–8 |
| No SSH / no Unsupported badges / no connect gate | Honored (not in tasks) |

## Placeholder / consistency review

- Types named consistently: `MySqlServerInfo`, `MySqlFamily`, `MySqlCatalogCapabilities`.
- Public test hooks: `supports_catalog_version`, `catalog_page_indexes_sql`, `relation_columns_sql`, `catalog_search_candidates_sql`.
- No TBD steps; legacy search ranking defined via Rust normalize helper.
