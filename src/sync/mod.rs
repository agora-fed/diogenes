//! The data sync. It copies public data from the Câmara API and the TSE into the
//! `diogenes_` reference tables.
//!
//! Jobs:
//! * `deputies`: deputies in office, details, parties, TSE colour/race.
//! * `groups`: fronts and members, caucuses (women, Black and brown, leaders, board).
//! * `roll_calls`: plenary roll calls with votes, orientations and bills.
//!
//! Each run writes one row in `diogenes_sync_run`, with the checks against the official
//! totals. A check that fails gives the status `warning`, not `failed`: the data is still
//! stored, and people can see the difference.

pub mod deputies;
pub mod groups;
pub mod roll_calls;

use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::request::Parts;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use sqlx::PgPool;

use crate::camara::CamaraClient;
use crate::error::{ok, ApiError, ApiResult, Envelope};
use crate::DiogenesState;

/// A check of stored data against a total that the source publishes.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Check {
    /// What the check counts, for example `front:55703`.
    pub subject: String,
    /// The total that the source publishes.
    pub expected: i64,
    /// The total that DIOGENES stored.
    pub actual: i64,
}

impl Check {
    /// True when the totals are equal.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.expected == self.actual
    }
}

/// The result of one job.
#[derive(Debug, Default)]
pub struct JobReport {
    /// The number of items written.
    pub items: i64,
    /// The checks against official totals.
    pub checks: Vec<Check>,
}

/// A sync job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Job {
    /// Deputies, parties, colour/race.
    Deputies,
    /// Fronts and caucuses.
    Groups,
    /// Plenary roll calls.
    RollCalls,
}

impl Job {
    /// All jobs, in dependency order.
    pub const ALL: [Self; 3] = [Self::Deputies, Self::Groups, Self::RollCalls];

    /// The name of the job.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Deputies => "deputies",
            Self::Groups => "groups",
            Self::RollCalls => "roll_calls",
        }
    }

    /// Parse a job name.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|j| j.name() == name)
    }

    const fn lock_key(self) -> i64 {
        // A fixed advisory-lock key per job: 0x44494F (DIO) followed by the job number.
        match self {
            Self::Deputies => 0x0044_494F_0001,
            Self::Groups => 0x0044_494F_0002,
            Self::RollCalls => 0x0044_494F_0003,
        }
    }
}

/// The configuration of the sync.
#[derive(Debug, Clone)]
pub struct SyncConfig {
    /// The first day of the roll-call backfill.
    pub roll_calls_since: chrono::NaiveDate,
    /// The interval between two runs of `roll_calls`.
    pub roll_calls_every: Duration,
    /// The interval between two runs of `deputies` and `groups`.
    pub reference_every: Duration,
    /// True to import colour/race from the TSE.
    pub tse_enabled: bool,
}

impl SyncConfig {
    /// Read the configuration from the environment:
    /// `DIOGENES_ROLL_CALLS_SINCE` (YYYY-MM-DD, default: 180 days ago),
    /// `DIOGENES_ROLL_CALLS_EVERY_MIN` (default 60),
    /// `DIOGENES_REFERENCE_EVERY_HOURS` (default 24),
    /// `DIOGENES_TSE_ENABLED` (default true).
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Read the configuration from a lookup function. `from_env` uses the environment.
    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let since = lookup("DIOGENES_ROLL_CALLS_SINCE")
            .and_then(|v| chrono::NaiveDate::parse_from_str(&v, "%Y-%m-%d").ok())
            .unwrap_or_else(|| chrono::Utc::now().date_naive() - chrono::Days::new(180));
        let number =
            |name: &str, default: u64| lookup(name).and_then(|v| v.parse().ok()).unwrap_or(default);
        let minutes = number("DIOGENES_ROLL_CALLS_EVERY_MIN", 60);
        let hours = number("DIOGENES_REFERENCE_EVERY_HOURS", 24);
        let tse_enabled = lookup("DIOGENES_TSE_ENABLED").map_or(true, |v| v != "false");
        Self {
            roll_calls_since: since,
            roll_calls_every: Duration::from_secs(minutes.max(5) * 60),
            reference_every: Duration::from_secs(hours.max(1) * 3600),
            tse_enabled,
        }
    }
}

/// Run one job and record the run. Return `None` when another process runs the same job.
///
/// # Errors
/// Returns a database error when the run cannot be recorded.
pub async fn run_job(
    db: &PgPool,
    client: &CamaraClient,
    config: &SyncConfig,
    job: Job,
) -> Result<Option<i64>, sqlx::Error> {
    // A session-level advisory lock on one dedicated connection: one run per job in the cluster.
    let mut lock_conn = db.acquire().await?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(job.lock_key())
        .fetch_one(&mut *lock_conn)
        .await?;
    if !locked {
        tracing::info!(job = job.name(), "diogenes sync already running elsewhere");
        return Ok(None);
    }
    let inserted: Result<i64, sqlx::Error> =
        sqlx::query_scalar("INSERT INTO diogenes_sync_run (job) VALUES ($1) RETURNING id")
            .bind(job.name())
            .fetch_one(db)
            .await;
    let run_id = match inserted {
        Ok(id) => id,
        Err(err) => {
            // The connection goes back to the pool. It must not keep the lock.
            unlock(&mut lock_conn, job).await;
            return Err(err);
        }
    };
    let result = match job {
        Job::Deputies => deputies::run(db, client, config.tse_enabled).await,
        Job::Groups => groups::run(db, client).await,
        Job::RollCalls => roll_calls::run(db, client, config.roll_calls_since).await,
    };
    let (status, items, checks, error) = match result {
        Ok(report) => {
            let status = if report.checks.iter().all(Check::passed) {
                "ok"
            } else {
                "warning"
            };
            (status, report.items, report.checks, None)
        }
        Err(err) => {
            tracing::error!(job = job.name(), error = %err, "diogenes sync failed");
            ("failed", 0, Vec::new(), Some(err.to_string()))
        }
    };
    let finish = sqlx::query(
        "UPDATE diogenes_sync_run
            SET finished_at = now(), status = $2, items = $3, checks = $4, error = $5
          WHERE id = $1",
    )
    .bind(run_id)
    .bind(status)
    .bind(i32::try_from(items).unwrap_or(i32::MAX))
    .bind(serde_json::to_value(&checks).unwrap_or_default())
    .bind(error)
    .execute(db)
    .await;
    unlock(&mut lock_conn, job).await;
    finish?;
    tracing::info!(job = job.name(), status, items, "diogenes sync finished");
    Ok(Some(run_id))
}

/// Release the advisory lock of a job on the connection that took it.
async fn unlock(conn: &mut sqlx::PgConnection, job: Job) {
    let released = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(job.lock_key())
        .execute(conn)
        .await;
    if let Err(err) = released {
        tracing::warn!(job = job.name(), error = %err, "diogenes sync cannot release its lock");
    }
}

/// An error of a job.
#[derive(Debug, thiserror::Error)]
pub enum JobError {
    /// The Câmara API failed.
    #[error(transparent)]
    Camara(#[from] crate::camara::CamaraError),
    /// The TSE import failed.
    #[error(transparent)]
    Tse(#[from] crate::tse::TseError),
    /// The database failed.
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Start the background scheduler. It runs all jobs at start, then on their intervals.
pub fn spawn_scheduler(db: PgPool, config: SyncConfig) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let client = match CamaraClient::new() {
            Ok(c) => c,
            Err(err) => {
                tracing::error!(error = %err, "diogenes sync cannot start the HTTP client");
                return;
            }
        };
        let mut reference_tick = tokio::time::interval(config.reference_every);
        let mut roll_call_tick = tokio::time::interval(config.roll_calls_every);
        loop {
            tokio::select! {
                _ = reference_tick.tick() => {
                    for job in [Job::Deputies, Job::Groups] {
                        let _report = run_job(&db, &client, &config, job).await;
                    }
                }
                _ = roll_call_tick.tick() => {
                    let _report = run_job(&db, &client, &config, Job::RollCalls).await;
                }
            }
        }
    })
}

/// The last run of one job.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct RunView {
    /// The job name.
    pub job: String,
    /// The start time.
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// The end time. `None` = running.
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    /// `running`, `ok`, `warning` or `failed`.
    pub status: String,
    /// Items written.
    pub items: i32,
    /// The checks that failed.
    pub failed_checks: serde_json::Value,
}

pub(crate) fn routes() -> Router<DiogenesState> {
    Router::new()
        .route("/diogenes/sync/status", get(status))
        .route("/diogenes/sync/{job}", post(trigger))
}

async fn status(State(state): State<DiogenesState>) -> ApiResult<Json<Envelope<Vec<RunView>>>> {
    let rows = sqlx::query_as::<_, RunView>(
        "SELECT DISTINCT ON (job) job, started_at, finished_at, status, items,
                COALESCE((SELECT jsonb_agg(c) FROM jsonb_array_elements(checks) c
                           WHERE c->>'expected' <> c->>'actual'), '[]'::jsonb) AS failed_checks
           FROM diogenes_sync_run
          ORDER BY job, started_at DESC",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(ok(rows))
}

async fn trigger(
    State(state): State<DiogenesState>,
    Path(job): Path<String>,
    parts: Parts,
    // A JSON body forces a CORS preflight: a cross-site form cannot start a sync.
    Json(_body): Json<serde_json::Value>,
) -> ApiResult<Json<Envelope<&'static str>>> {
    let actor = crate::room::access::actor(&state, &parts).await?;
    if !actor.can_operate_sync {
        return Err(ApiError::Forbidden);
    }
    let job = Job::parse(&job).ok_or(ApiError::NotFound)?;
    let client = CamaraClient::new().map_err(|e| ApiError::Invalid(e.to_string()))?;
    let db = state.db.clone();
    tokio::spawn(async move {
        let _report = run_job(&db, &client, &SyncConfig::from_env(), job).await;
    });
    Ok(ok("started"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn lookup(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn config_defaults_apply_when_nothing_is_set() {
        let config = SyncConfig::from_lookup(lookup(&[]));
        let expected_since = chrono::Utc::now().date_naive() - chrono::Days::new(180);
        assert!(
            config
                .roll_calls_since
                .signed_duration_since(expected_since)
                .num_days()
                .abs()
                <= 1
        );
        assert_eq!(config.roll_calls_every, Duration::from_secs(60 * 60));
        assert_eq!(config.reference_every, Duration::from_secs(24 * 3600));
        assert!(config.tse_enabled);
    }

    #[test]
    fn config_reads_the_values_that_are_set() {
        let config = SyncConfig::from_lookup(lookup(&[
            ("DIOGENES_ROLL_CALLS_SINCE", "2025-02-03"),
            ("DIOGENES_ROLL_CALLS_EVERY_MIN", "15"),
            ("DIOGENES_REFERENCE_EVERY_HOURS", "6"),
            ("DIOGENES_TSE_ENABLED", "false"),
        ]));
        assert_eq!(
            config.roll_calls_since,
            chrono::NaiveDate::from_ymd_opt(2025, 2, 3).unwrap()
        );
        assert_eq!(config.roll_calls_every, Duration::from_secs(15 * 60));
        assert_eq!(config.reference_every, Duration::from_secs(6 * 3600));
        assert!(!config.tse_enabled);
    }

    #[test]
    fn config_raises_too_small_intervals_and_ignores_bad_values() {
        let config = SyncConfig::from_lookup(lookup(&[
            ("DIOGENES_ROLL_CALLS_SINCE", "yesterday"),
            ("DIOGENES_ROLL_CALLS_EVERY_MIN", "1"),
            ("DIOGENES_REFERENCE_EVERY_HOURS", "0"),
            ("DIOGENES_TSE_ENABLED", "yes"),
        ]));
        assert_eq!(config.roll_calls_every, Duration::from_secs(5 * 60));
        assert_eq!(config.reference_every, Duration::from_secs(3600));
        assert!(config.tse_enabled, "only the value `false` turns it off");
        let bad_number =
            SyncConfig::from_lookup(lookup(&[("DIOGENES_ROLL_CALLS_EVERY_MIN", "abc")]));
        assert_eq!(bad_number.roll_calls_every, Duration::from_secs(60 * 60));
    }

    #[test]
    fn config_from_env_has_the_same_defaults_as_an_empty_lookup() {
        // The CI environment sets no DIOGENES_* variable.
        let config = SyncConfig::from_env();
        assert!(config.roll_calls_every >= Duration::from_secs(5 * 60));
        assert!(config.reference_every >= Duration::from_secs(3600));
    }

    #[test]
    fn job_names_round_trip() {
        for job in Job::ALL {
            assert_eq!(Job::parse(job.name()), Some(job));
        }
        assert_eq!(Job::parse("nope"), None);
    }

    #[test]
    fn lock_keys_are_distinct() {
        let keys: std::collections::HashSet<i64> = Job::ALL.iter().map(|j| j.lock_key()).collect();
        assert_eq!(keys.len(), Job::ALL.len());
    }

    #[test]
    fn check_pass_rule() {
        let c = Check {
            subject: "x".into(),
            expected: 3,
            actual: 3,
        };
        assert!(c.passed());
        let off = Check { actual: 4, ..c };
        assert!(!off.passed());
    }
}
