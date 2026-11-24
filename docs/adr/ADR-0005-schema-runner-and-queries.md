# ADR-0005: Use an own schema runner and runtime-checked queries

- Status: Accepted
- Date: 2026-11-24

## Context

The host uses the sqlx migrator and the table `_sqlx_migrations`. Two migrators on one
table conflict. In production, an operator applies host migrations with `psql`.

The host builds with `SQLX_OFFLINE=true` and a committed query cache. A git dependency
with compile-time query macros needs its own cache in the dependency.

## Decision

1. DIOGENES records its versions in `diogenes_migration`.
2. Each SQL file is idempotent. Each file records its own version as its last statement.
3. The `migrate` function applies the missing files at start, each file in a transaction.
4. An operator can also apply the files with `psql`. The result is the same.
5. DIOGENES uses runtime queries (`sqlx::query`, `sqlx::query_as`) with bound parameters.
6. Integration tests run each query against a real PostgreSQL database.

## Consequences

- The host build needs no database and no query cache for DIOGENES.
- The compiler does not check the SQL. The integration tests do this check. CI runs them.
- Run the files as the application role, or change the table owner after `psql`.
