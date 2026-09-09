# MySQL 5.6+ and MariaDB 10.1+ Catalog Support

**Date:** 2026-09-09  
**Status:** Approved for planning  
**Repo:** local clone of `yelog/lazydb`  
**Goal:** Extend the MySQL catalog contract so Explorer, relation Data/DDL, catalog search, and completion work on Oracle MySQL ≥ 5.6 and MariaDB ≥ 10.1, with silent degradation for metadata that only exists on newer servers. Target an upstream-contributable change.

## Problem

LazyDB already connects and executes SQL against MySQL 5.6/5.7. Catalog pages are hard-gated to Oracle MySQL ≥ 8.0.13 and reject MariaDB. The gate exists because catalog SQL assumes MySQL 8-shaped metadata (for example `information_schema.statistics.expression`, `REGEXP_REPLACE`, combined `START TRANSACTION … READ ONLY`), not because 5.x lacks databases/tables/columns.

Users with MySQL 5.6/5.7 (and MariaDB) get a clear catalog error and lose Explorer, relation tabs driven by catalog identity, and catalog-backed completion, even though the underlying metadata is queryable.

## Success criteria

- Catalog works on Oracle MySQL ≥ 5.6.0 and MariaDB ≥ 10.1.0 (including `5.5.5-10.x.y-MariaDB` version strings).
- Feature surface matches the current MySQL catalog groups: databases, tables, views, functions, procedures, triggers, indexes/constraints children, relation Data/DDL, catalog search, completion cache population.
- MySQL 8.0.13+ behavior is unchanged (regression baseline).
- Missing newer-only fields degrade silently (empty / omitted), not as user-facing Unsupported badges or connection failures.
- Change is structured for an upstream PR: capability layer, tests, docs.

## Non-goals

- SSH tunneling.
- A separate MariaDB driver, UI brand, or dialect enum beyond catalog/SQL variants.
- User-facing “compatibility mode” banners or Unsupported metadata badges.
- Moving the version gate onto connect / `agent query` / MCP query (SQL execution stays ungated as today).
- Supporting Oracle MySQL < 5.6 or MariaDB < 10.1.

## Approach

**Version capability layer + SQL variants** (chosen over dual adapters or probe-and-retry).

Catalog code asks capabilities, not scattered `if version >= …` checks. Oracle MySQL 8.0.13+ keeps the current SQL path.

## Architecture

### Components

1. **`MySqlServerInfo` (parse)**  
   Input: `SELECT VERSION()` string.  
   Output: `family` (`OracleMySql` | `MariaDb`), normalized `(major, minor, patch)`, and whether the server meets the minimum catalog floor.  
   Must strip MariaDB’s legacy `5.5.5-` prefix so `5.5.5-10.11.8-MariaDB` parses as MariaDB 10.11.8.

2. **`MySqlCatalogCapabilities` (derive)**  
   Booleans / small enums derived from `MySqlServerInfo`, for example:
   - `statistics_expression`
   - `generation_expression`
   - `regexp_replace`
   - `search_cte`
   - `catalog_begin_sql` variant  
   Catalog helpers branch only on these flags.

3. **`MySqlAdapter` (execute)**  
   Keep the existing public adapter API. After connect/probe, parse version once and cache capabilities on the adapter. `load_catalog_page`, search, relation children, and column metadata select SQL or post-process by capability. Prefer extracting parse/capability helpers into a focused module (for example `src/db/mysql_version.rs`) so `mysql.rs` does not grow another unstructured block.

4. **Docs contract**  
   Update README, `docs/database-capabilities.md`, and `docs/architecture.md` to the new minimum versions and silent-degradation rule.

### Data flow

```text
connect / probe
  → VERSION()
  → MySqlServerInfo
  → MySqlCatalogCapabilities (cached on adapter)
  → catalog / search / column / index helpers pick SQL variant
  → existing Explorer / completion / relation UI unchanged
```

## Compatibility matrix

| Capability | Typically true when | When false |
| --- | --- | --- |
| `statistics_expression` | Oracle MySQL ≥ 8.0.13 | Omit `expression` from index SQL; use `column_name` only |
| `generation_expression` | Oracle MySQL ≥ 5.7.6; MariaDB with generated columns | Omit column from SELECT; still detect generated via `EXTRA` when possible; expression text empty |
| `regexp_replace` | Oracle MySQL ≥ 8.0; MariaDB usually | Normalize search needles/names in Rust; SQL uses `LOWER` + `LOCATE` only |
| `search_cte` | Oracle MySQL ≥ 8.0; MariaDB ≥ 10.2 | Non-CTE search SQL (or UNION + app-side filter) with same result semantics |
| `catalog_begin_readonly_combo` | Oracle MySQL 8.0+ (current string) | `START TRANSACTION WITH CONSISTENT SNAPSHOT` (read intent via no writes / existing session policy) |
| Database / table / view / routine / trigger listing | MySQL 5.6+ / MariaDB 10.1+ | Keep current `information_schema` queries |
| `SHOW CREATE` DDL | MySQL 5.6+ / MariaDB 10.1+ | Keep; tolerate MariaDB textual differences without hard 8.0-only parsing |

### Version gate replacement

- Remove “any MariaDB ⇒ unsupported”.
- Replace the 8.0.13 catalog floor with: Oracle MySQL ≥ 5.6.0 **or** MariaDB ≥ 10.1.0.
- 8.0.13 remains a capability threshold for functional-index `expression`, not a catalog entry ban.
- Unparseable `VERSION()` ⇒ catalog unsupported (fail closed).

## Error handling

- Below minimum: `Unsupported` with an updated message naming MySQL 5.6+ / MariaDB 10.1+.
- Missing capabilities: silent degradation only.
- Connect, execute, agent/MCP query: unchanged; no new version gate on those paths.

## Testing

### Unit (required)

- Version parsing for `5.6.16-log`, `5.7.44`, `8.0.12`, `8.0.13`, `8.4.1-commercial`, `10.1.48-MariaDB`, `5.5.5-10.11.8-MariaDB`, and invalid strings.
- Capability matrix assertions per family/version.
- SQL variant selection (index `expression`, CTE/`REGEXP_REPLACE`, begin SQL).
- Update `tests/mysql_adapter.rs` assertions that currently require 5.7/MariaDB to fail `supports_catalog_version`.

### Integration

- Existing MySQL 8 fixture suite must stay green.
- If 5.7 or MariaDB is available locally/CI: smoke databases/tables expand, one catalog search, column metadata load.
- Otherwise document manual verification against known 5.6/5.7 hosts in the PR.

### Docs

- Keep README / capability / architecture text aligned with code; update any docs tests that pin the old 8.0.13-only wording.

## PR scope

**In**

- Capability layer + catalog SQL variants + silent degradation
- Gate/message/docs updates
- Unit tests and MySQL 8 regression

**Out**

- SSH, MariaDB-as-separate-product UI, Unsupported badges, agent policy changes

## Implementation notes for planning

- Follow existing `OptionalMetadata` patterns where metadata is already optional.
- Do not change Explorer/completion protocols; only the adapter’s ability to feed catalog pages on older servers.
- Keep MariaDB on the same `DatabaseKind::MySql` profile path; family is an internal catalog concern unless a later change needs user-visible labeling.
