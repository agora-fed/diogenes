//! The campaign-finance API (ADR-0009).
//!
//! * `GET /diogenes/deputies/{camara_id}/campaign`: totals by source and the largest donors
//!   of one deputy. A signed-in user of an org with the module on can read it.
//! * `GET /diogenes/scenarios/{id}/donors`: the money of each column of a scenario, and the
//!   donors that gave to two or more deputies, by column. Room members only.
//!
//! The answers carry the source link and the import date. They state facts only.

use std::collections::{BTreeMap, HashMap};

use axum::extract::{Path, State};
use axum::http::request::Parts;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use super::{ELECTION_YEAR, SOURCE_PAGE};
use crate::error::{ok, ApiError, ApiResult, Envelope};
use crate::room::access::{actor, require_scenario, Role};
use crate::room::projects::load_columns;
use crate::DiogenesState;

/// The number of donors in the answer for one deputy.
pub const DEPUTY_DONORS: i64 = 15;
/// The number of shared donors in the answer for a scenario.
pub const SHARED_DONORS: usize = 40;

/// The routes of the finance API.
pub fn routes() -> Router<DiogenesState> {
    Router::new()
        .route("/diogenes/deputies/{camara_id}/campaign", get(deputy))
        .route("/diogenes/scenarios/{scenario_id}/donors", get(scenario))
}

/// The provenance of the data.
#[derive(Debug, Serialize)]
pub struct Provenance {
    /// The election year.
    pub year: i16,
    /// The public page of the TSE data set.
    pub source_url: &'static str,
    /// The end of the last import that worked. `None`: no import yet.
    pub imported_at: Option<DateTime<Utc>>,
}

async fn provenance(db: &PgPool) -> Result<Provenance, sqlx::Error> {
    let imported_at = sqlx::query_scalar(
        "SELECT max(finished_at) FROM diogenes_sync_run
          WHERE job = 'donations' AND status IN ('ok', 'warning')",
    )
    .fetch_one(db)
    .await?;
    Ok(Provenance {
        year: ELECTION_YEAR,
        source_url: SOURCE_PAGE,
        imported_at,
    })
}

/// The money of one source.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct SourceTotal {
    /// `individuals`, `party`, `own`, `candidates`, `crowdfunding` or `other`.
    pub source: String,
    /// The value in cents.
    pub cents: i64,
    /// The number of receipts.
    pub receipts: i32,
}

/// One donor of one deputy.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct DonorView {
    /// The donor key (valid until the next import).
    pub key: String,
    /// The name, as the TSE publishes it.
    pub name: String,
    /// `person` or `candidate`.
    pub kind: String,
    /// The economic activity, when the TSE gives one.
    pub activity: Option<String>,
    /// The state.
    pub uf: Option<String>,
    /// The value in cents.
    pub cents: i64,
    /// True: the person gave to a party, and the party sent the money to the deputy.
    pub via_party: bool,
}

/// The campaign of one deputy.
#[derive(Debug, Serialize)]
pub struct Campaign {
    /// The provenance.
    #[serde(flatten)]
    pub provenance: Provenance,
    /// All receipts, in cents.
    pub total_cents: i64,
    /// The money by source, largest first.
    pub totals: Vec<SourceTotal>,
    /// The largest donors.
    pub donors: Vec<DonorView>,
}

async fn deputy(
    State(state): State<DiogenesState>,
    Path(camara_id): Path<i32>,
    parts: Parts,
) -> ApiResult<Json<Envelope<Campaign>>> {
    actor(&state, &parts).await?;
    let known: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM diogenes_deputy WHERE camara_id = $1)")
            .bind(camara_id)
            .fetch_one(&state.db)
            .await?;
    if !known {
        return Err(ApiError::NotFound);
    }
    let totals = sqlx::query_as::<_, SourceTotal>(
        "SELECT source, cents, receipts FROM diogenes_campaign_total
          WHERE election_year = $1 AND camara_id = $2 ORDER BY cents DESC, source",
    )
    .bind(ELECTION_YEAR)
    .bind(camara_id)
    .fetch_all(&state.db)
    .await?;
    let donors = sqlx::query_as::<_, DonorView>(
        "SELECT n.key, n.name, n.kind, n.activity, n.uf, d.cents, d.via_party
           FROM diogenes_donation d
           JOIN diogenes_donor n ON n.election_year = d.election_year AND n.key = d.donor_key
          WHERE d.election_year = $1 AND d.camara_id = $2
          ORDER BY d.cents DESC, n.name LIMIT $3",
    )
    .bind(ELECTION_YEAR)
    .bind(camara_id)
    .bind(DEPUTY_DONORS)
    .fetch_all(&state.db)
    .await?;
    Ok(ok(Campaign {
        provenance: provenance(&state.db).await?,
        total_cents: totals.iter().map(|t| t.cents).sum(),
        totals,
        donors,
    }))
}

/// The money of the deputies in one column.
#[derive(Debug, Default, Serialize)]
pub struct ColumnMoney {
    /// The column id.
    pub column_id: Uuid,
    /// The deputies in the column.
    pub deputies: i64,
    /// The deputies with accounts in the data.
    pub with_accounts: i64,
    /// Cents by source.
    pub cents_by_source: BTreeMap<String, i64>,
}

/// A donor that gave to two or more deputies in office.
#[derive(Debug, Serialize)]
pub struct SharedDonor {
    /// The donor key.
    pub key: String,
    /// The name.
    pub name: String,
    /// `person` or `candidate`.
    pub kind: String,
    /// The economic activity.
    pub activity: Option<String>,
    /// The deputies that got money from this donor (directly or through a party).
    pub deputies: i64,
    /// The value in cents, to all these deputies.
    pub cents: i64,
    /// Deputies per column id.
    pub by_column: BTreeMap<Uuid, i64>,
}

/// The money of a scenario.
#[derive(Debug, Serialize)]
pub struct ScenarioMoney {
    /// The provenance.
    #[serde(flatten)]
    pub provenance: Provenance,
    /// The money per column, in the order of the project columns.
    pub columns: Vec<ColumnMoney>,
    /// The shared donors: most deputies first, then the largest value.
    pub donors: Vec<SharedDonor>,
}

async fn scenario(
    State(state): State<DiogenesState>,
    Path(scenario_id): Path<Uuid>,
    parts: Parts,
) -> ApiResult<Json<Envelope<ScenarioMoney>>> {
    let me = actor(&state, &parts).await?;
    let (_access, project_id) = require_scenario(&state.db, &me, scenario_id, Role::Viewer).await?;
    let columns = load_columns(&state.db, project_id).await?;
    let undecided = columns
        .iter()
        .find(|c| c.key.as_deref() == Some("undecided"))
        .map(|c| c.id);
    let placed: Vec<(i32, Option<Uuid>)> = sqlx::query_as(
        "SELECT d.camara_id, p.column_id FROM diogenes_deputy d
           LEFT JOIN diogenes_position p ON p.camara_id = d.camara_id AND p.scenario_id = $1
          WHERE d.in_office",
    )
    .bind(scenario_id)
    .fetch_all(&state.db)
    .await?;
    let column_of: HashMap<i32, Uuid> = placed
        .into_iter()
        .filter_map(|(id, c)| c.or(undecided).map(|c| (id, c)))
        .collect();
    let mut money: HashMap<Uuid, ColumnMoney> = HashMap::new();
    for c in column_of.values() {
        money.entry(*c).or_default().deputies += 1;
    }
    let totals: Vec<(i32, String, i64)> = sqlx::query_as(
        "SELECT camara_id, source, cents FROM diogenes_campaign_total WHERE election_year = $1",
    )
    .bind(ELECTION_YEAR)
    .fetch_all(&state.db)
    .await?;
    let mut counted: std::collections::HashSet<i32> = std::collections::HashSet::new();
    for (id, source, cents) in totals {
        let Some(c) = column_of.get(&id) else {
            continue;
        };
        let m = money.entry(*c).or_default();
        *m.cents_by_source.entry(source).or_default() += cents;
        if counted.insert(id) {
            m.with_accounts += 1;
        }
    }
    let columns_out = columns
        .iter()
        .map(|c| {
            let mut m = money.remove(&c.id).unwrap_or_default();
            m.column_id = c.id;
            m
        })
        .collect();
    let donors = shared_donors(&state.db, &column_of).await?;
    Ok(ok(ScenarioMoney {
        provenance: provenance(&state.db).await?,
        columns: columns_out,
        donors,
    }))
}

/// The running count of one donor in a scenario.
#[derive(Debug, Default)]
struct Tally {
    deputies: i64,
    by_column: BTreeMap<Uuid, i64>,
    cents: i64,
}

async fn shared_donors(
    db: &PgPool,
    column_of: &HashMap<i32, Uuid>,
) -> Result<Vec<SharedDonor>, sqlx::Error> {
    let rows: Vec<(String, i32, i64)> = sqlx::query_as(
        "SELECT d.donor_key, d.camara_id, sum(d.cents)::bigint
           FROM diogenes_donation d
           JOIN diogenes_deputy p ON p.camara_id = d.camara_id AND p.in_office
          WHERE d.election_year = $1
          GROUP BY d.donor_key, d.camara_id",
    )
    .bind(ELECTION_YEAR)
    .fetch_all(db)
    .await?;
    let mut by_key: HashMap<String, Tally> = HashMap::new();
    for (key, id, cents) in rows {
        let Some(c) = column_of.get(&id) else {
            continue;
        };
        let e = by_key.entry(key).or_default();
        e.deputies += 1;
        *e.by_column.entry(*c).or_default() += 1;
        e.cents += cents;
    }
    let mut ranked: Vec<(String, Tally)> = by_key
        .into_iter()
        .filter(|(_, v)| v.deputies >= 2)
        .collect();
    ranked.sort_by(|a, b| {
        b.1.deputies
            .cmp(&a.1.deputies)
            .then(b.1.cents.cmp(&a.1.cents))
            .then(a.0.cmp(&b.0))
    });
    ranked.truncate(SHARED_DONORS);
    let keys: Vec<&str> = ranked.iter().map(|(k, _)| k.as_str()).collect();
    let names: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT key, name, kind, activity FROM diogenes_donor
          WHERE election_year = $1 AND key = ANY($2)",
    )
    .bind(ELECTION_YEAR)
    .bind(&keys)
    .fetch_all(db)
    .await?;
    let names: HashMap<String, (String, String, Option<String>)> = names
        .into_iter()
        .map(|(k, n, kind, a)| (k, (n, kind, a)))
        .collect();
    Ok(ranked
        .into_iter()
        .filter_map(|(key, t)| {
            let (name, kind, activity) = names.get(&key)?.clone();
            Some(SharedDonor {
                key,
                name,
                kind,
                activity,
                deputies: t.deputies,
                cents: t.cents,
                by_column: t.by_column,
            })
        })
        .collect())
}
