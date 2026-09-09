# Database Capabilities

This page describes the implemented adapter contract. It is a capability
reference, not a claim that a live server is available on the development
machine.

## Driver Matrix

## Monitoring Dashboard

The connection dashboard is available for PostgreSQL and MySQL. It
collects read-only status counters, connection gauges, and a bounded process
list. SQLite reports monitoring as unsupported because it has no server-wide
activity catalog. Samples remain in memory for the active workspace tab and
are not persisted. Counter rates use the elapsed time between valid samples;
the first sample, missing fields, resets, restarts, and failed polls create a
history gap rather than a fabricated rate.

The process list is bounded to 2,000 rows and may be marked restricted when
the database role cannot observe every session. Dashboard V1 does not cancel
queries or terminate sessions.

| Driver | Server/version gate | Namespace model | Catalog groups | Metadata support |
| --- | --- | --- | --- | --- |
| PostgreSQL | PostgreSQL 12 or newer | Database + schema | Tables, views, materialized views, sequences, functions, procedures, types | Type family, defaults, identity, generated expressions, character length, collation, comments; numeric precision/scale is not advertised |
| MySQL | Oracle MySQL 5.6+ or MariaDB 10.1+; newer-only metadata (for example functional index expressions) is omitted on older servers | Database is schema | Tables, views, functions, procedures, triggers | Type family, defaults, auto-increment, generated expressions, numeric precision/scale, character length, collation, character set, comments |
| SQL Server | SQL Server 2012 or newer | Database + schema | Tables, views, functions, procedures, sequences, triggers; relation children include columns, indexes, keys, and foreign keys | Type family, defaults, identity, computed/generated expressions, numeric precision/scale, character length, collation, comments, and rowversion metadata |
| SQLite | SQLite metadata support through native schema tables; no server-version gate | Database + attached schema aliases | Tables, views, triggers | Default expressions and hidden-column metadata; unsupported fields are represented as unsupported |

PostgreSQL, MySQL, and SQLite advertise lazy children. SQLite opens a pool with
exactly one physical connection; SQL Server loads its supported catalog groups
without lazy child requests.

PostgreSQL relation catalog identities include the database, schema, object name,
and `pg_class.oid`. The OID is used only to reconcile a stale relation after an
external rename; the adapter still checks the active database, catalog scope,
relation kind, and permissions before returning the replacement entry. OIDs are
not permanent identifiers: after a relation is dropped, an OID may be reused,
so an unavailable or out-of-scope result is treated as a failed reconciliation
rather than a rename.

SQL Server catalog reads are scoped to databases and schemas and use native
`sys.*` catalog views. SQL Server does not currently use lazy child requests.
The adapter gates connections at SQL Server 2012, supports SQL username/password
authentication only, and requires an explicit TCP host and port. Windows
Integrated Authentication, Kerberos, Entra, Named Instance/SQL Browser discovery,
Dashboard metrics, process metrics, and additional specialized types are
deferred.

SQL Server query support includes multiple result sets, empty-result column
metadata, bounded relation previews, adapter-owned relation DDL, and relation
data editing with bound values. Standalone `GO` batch separator lines are split
before execution; `GO` and `GO 1` are supported, while other repeat counts are
rejected. The value layer decodes NULL, booleans, signed and unsigned integers,
floating point, text, bytes, dates, times, date-times, timestamps,
`uniqueidentifier` as text, and XML as text. Types that do not have a safe
normalized representation, including `money`/`smallmoney` and `sql_variant`, are
shown as unsupported values rather than being silently coerced.

## Profile Discovery

`Test Connection` probes the draft connection, then performs read-only database
and schema discovery for the hierarchical scope picker. It does not save
metadata, change the active connection, or create a persisted profile. Results
are fingerprinted against connection fields and credential revision; a result
for a draft that has changed is ignored.

The picker presents `All` or `Selected` databases, with schemas nested under
each discovered database. PostgreSQL and SQLite can select `All schemas` or
individual schemas. MySQL mirrors each selected database as its schema; those
rows are read-only and cannot be toggled separately. If discovery is stale or
unavailable, saved selections remain visible with a warning.

## Catalog Paging

### PostgreSQL Schema Owners

The New Schema form uses PostgreSQL's effective `current_user` as the initial
owner. Its owner picker discovers both `LOGIN` users and `NOLOGIN` roles from
`pg_roles`. Roles that are visible but cannot be assumed by the active user are
shown as disabled rather than hidden. PostgreSQL 16 and newer use `SET ROLE`
privilege checks, as required by `CREATE SCHEMA ... AUTHORIZATION`; this same
check is used on older supported PostgreSQL versions. If role discovery is unavailable, the Owner field remains a
manual text input and PostgreSQL remains authoritative when the mutation runs.

Explorer pages are lazy, bounded to a maximum page size of 500, and use
versioned keyset cursors rather than offsets. A request includes connection
identity, catalog epoch, request id, target, cursor, and scope. Pages are
validated against that complete key before updating the tree or completion
cache. Refresh advances the catalog epoch; late, duplicate, malformed, or
cross-connection pages are ignored. Existing data is retained as stale when a
refresh fails. Empty targets render an empty state and partial targets render
`Load more...`.

SQLite loads each catalog page inside one transaction and rolls it back
afterward. The single physical connection gives the page a stable snapshot and
prevents races with another pooled SQLite connection; the catalog operations do
not write database state.

## Relations

### Refresh And Catalog Synchronization

Explorer `r` refreshes the selected catalog target and advances the catalog
epoch. Requests from an older epoch, duplicate pages, malformed pages, and
responses for another connection are ignored. Existing rows remain visible as
stale until the replacement page succeeds; a failed refresh does not erase the
last known snapshot.

After SQL changes that can alter catalog structure, LazyDB conservatively
refreshes the active profile's database catalog. The refresh is deduplicated,
completion data is rebuilt from accepted pages, and the UI reports either
`Catalog synchronized` or `SQL succeeded, but catalog synchronization failed;
refresh to retry`. Ambiguous or multi-statement SQL is not used to infer a
smaller target. Catalog synchronization does not claim that unrelated external
changes have been detected; use Explorer `r` when another client changes the
database.

For a selected PostgreSQL relation or relation child, `r` first resolves the
stored OID. A successful resolution replaces the stale catalog entry, rebinds
an open relation tab in place, invalidates its pending requests, and reloads
the relation children. This path preserves the tab's view and local state, but
a dirty relation remains write-blocked until its new identity is verified.

Opening a table, view, materialized view, or supported relation child creates or
activates a relation workspace tab. It has independent `Data` and `DDL` pages.
Data is an adapter-owned read-only preview with a hard `LIMIT 500`; callers do
not append an arbitrary limit. The adapter quotes the relation and applies the
optional WHERE/ORDER BY preview clauses before applying that limit. Statement
metadata is collected before rows, so zero-row relations still expose their
columns.

The `DDL` page is also adapter-owned end to end. The adapter validates the
catalog identity, reads the relation and its children, assembles complete
display SQL, and returns the SQL plus its provenance. The UI only renders and
scrolls that result; it never reconstructs DDL from generic catalog rows.

Driver-specific DDL behavior is:

- PostgreSQL uses a read-only `REPEATABLE READ` transaction and native catalog
  functions such as `pg_get_viewdef`, `pg_get_expr`, `pg_get_constraintdef`,
  `pg_get_indexdef`, and `pg_get_triggerdef`. It assembles tables, views, and
  materialized views with columns, identity/default/generated clauses,
  constraints, comments, indexes, and non-internal triggers where available.
- PostgreSQL relation rename reconciliation requires access to `pg_class` and
  `pg_namespace` for the stored OID and the active profile's selected schema.
  It does not require database or role creation privileges. A dropped object,
  an OID reused for another relation kind, or an object hidden by permissions
  is not treated as a rename.
- Oracle MySQL and MariaDB read the main table/view and each trigger through
  `SHOW CREATE`, discover triggers through `information_schema`, and assemble
  the native object statement with sorted trigger statements. Catalog SQL
  follows parsed server capabilities; fields that exist only on newer servers
  are omitted rather than failing the read.
- SQLite reads the main table/view and related indexes/triggers from each
  schema's `sqlite_schema` table. The complete read runs on the single SQLite
  connection inside a transaction that is rolled back afterward. A relation
  with only its native statement has `NativeCatalog` provenance; adding related
  statements makes the assembled result `AdapterGenerated`.

`NativeCatalog` means the returned DDL is one native server/catalog statement.
`AdapterGenerated` means the adapter combined native statements into a stable,
sectioned result (and, for PostgreSQL, reconstructed the main statement from
catalog metadata). Both are read-only display results and remain owned by the
adapter.

Every relation request carries tab UUID, tab generation, request id, connection
identity, relation key, request kind, and catalog scope. Cancellation and stale
responses cannot overwrite a newer request. A loading, failed, or cancelled
request may continue displaying its previous owned Data or DDL snapshot. Each
owned snapshot records connection identity, profile UUID, and catalog scope.
The UI attributes snapshots as `LIVE`, `OFFLINE SNAPSHOT`, `PROFILE DELETED
SNAPSHOT`, or `OUT OF SCOPE SNAPSHOT`; these labels describe the snapshot's
relationship to the current connection/profile/scope, not whether its SQL was
native or adapter-generated.

## Completion Cache

Completion is served from an in-memory catalog cache while typing. Lazy pages
append stable, deduplicated entries filtered to the active scope; connection or
catalog resets clear it. Scheduled completion carries console UUID, document
revision, connection identity, and catalog generation. If any component is
stale, the result is discarded. Explicit completion performs no database I/O,
returns ranked context-aware candidates, and is capped at ten results.
PostgreSQL and MySQL routine entries can contribute function and procedure
completion; SQLite does not advertise routine completion.

Cancelling SQL Server execution closes the active session because the TDS driver
does not expose a cancellation primitive. SQL Server rolls back an open
transaction when that session closes; LazyDB does not claim commit or rollback
acknowledgement after the forced close.
## Coding Agent Boundary

LazyDB's agent interface uses the same native adapters as the TUI, but each
operation is headless and target-attributed. Connections are resolved from the
current project plus global profiles. Other project-scoped profiles are hidden.

The first release has no shared agent transaction handles. Each operation owns
its connection lifecycle, and schema/query results include the selected profile,
environment, database, and effective read-only status. The database role is the
final read/write boundary; client-side MCP approval is not a replacement for
database grants.
