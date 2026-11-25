//! The schema runner. It applies the SQL files in `migrations/` in order.
//!
//! DIOGENES does not use the sqlx migrator. The host already uses `_sqlx_migrations`,
//! and two migrators on one table conflict. DIOGENES records its versions in
//! `diogenes_migration`. Each SQL file is idempotent and records its own version, so an
//! operator can also apply the files with `psql`.

use sqlx::PgPool;

/// The embedded migrations: (version, SQL). Keep the list in ascending order.
const MIGRATIONS: &[(i32, &str)] = &[
    (1, include_str!("../migrations/0001_init.sql")),
    (2, include_str!("../migrations/0002_project_rule.sql")),
    (3, include_str!("../migrations/0003_campaign_finance.sql")),
    (4, include_str!("../migrations/0004_companies.sql")),
];

/// Apply all migrations that are not applied yet. Each one runs in its own transaction.
///
/// # Errors
/// Returns the database error of the first migration that fails. The runner stops there.
pub async fn migrate(db: &PgPool) -> Result<usize, sqlx::Error> {
    // Many host processes can start at the same time (rolling deploy). One session-level
    // advisory lock lets one process apply the files; the others wait, then find them applied.
    let mut lock = db.acquire().await?;
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(MIGRATION_LOCK)
        .execute(&mut *lock)
        .await?;
    let result = apply(db).await;
    let _unlock = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(MIGRATION_LOCK)
        .execute(&mut *lock)
        .await;
    result
}

/// The advisory-lock key of the schema runner: "DIO" followed by 0x00FF.
const MIGRATION_LOCK: i64 = 0x0044_494F_00FF;

async fn apply(db: &PgPool) -> Result<usize, sqlx::Error> {
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS diogenes_migration (
            version integer PRIMARY KEY,
            applied_at timestamptz NOT NULL DEFAULT now())",
    )
    .execute(db)
    .await?;
    let applied: Vec<i32> = sqlx::query_scalar("SELECT version FROM diogenes_migration")
        .fetch_all(db)
        .await?;
    let mut count = 0;
    for (version, sql) in MIGRATIONS {
        if applied.contains(version) {
            continue;
        }
        let mut tx = db.begin().await?;
        sqlx::raw_sql(sql).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO diogenes_migration (version) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(version)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        tracing::info!(version, "diogenes migration applied");
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::MIGRATIONS;

    #[test]
    fn versions_are_ascending_and_unique() {
        let versions: Vec<i32> = MIGRATIONS.iter().map(|(v, _)| *v).collect();
        let mut sorted = versions.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(versions, sorted);
    }

    #[test]
    fn every_file_records_its_own_version() {
        for (version, sql) in MIGRATIONS {
            let marker = format!("INSERT INTO diogenes_migration (version) VALUES ({version})");
            assert!(
                sql.contains(&marker),
                "migration {version} must record itself"
            );
        }
    }
}
