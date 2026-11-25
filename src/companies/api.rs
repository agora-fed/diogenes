//! Company data in the answers of the finance API (ADR-0010), and the public list of
//! campaigns.
//!
//! * `GET /diogenes/campaigns`: the deputies in office with the total of their 2022
//!   campaign. Public (ADR-0009, open data).
//!
//! A link "person → company" means that the name and the six middle CPF digits in the
//! Receita file of partners are equal to the donor. The answer gives the role
//! ("qualificação") as the Receita writes it: a partner is not always an owner.

use std::collections::{BTreeMap, HashMap, HashSet};

use axum::extract::State;
use axum::Json;
use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use super::{DATASET, SOURCE_PAGE};
use crate::error::{ok, ApiResult, Envelope};
use crate::finance::ELECTION_YEAR;
use crate::DiogenesState;

/// The companies of one donor in an answer.
pub const COMPANIES_PER_DONOR: usize = 5;
/// The sectors in the answer for a scenario.
pub const SECTORS: usize = 15;

/// The version of the Receita data.
#[derive(Debug, Clone, Serialize)]
pub struct CompanySource {
    /// The Receita month, for example `2026-11`.
    pub version: String,
    /// The public page of the data set.
    pub source_url: &'static str,
    /// The end of the import.
    pub imported_at: DateTime<Utc>,
}

/// The version of the Receita data. `None`: no import yet.
///
/// # Errors
/// Returns a database error.
pub async fn company_source(db: &PgPool) -> Result<Option<CompanySource>, sqlx::Error> {
    let row: Option<(String, DateTime<Utc>)> =
        sqlx::query_as("SELECT version, imported_at FROM diogenes_dataset WHERE name = $1")
            .bind(DATASET)
            .fetch_optional(db)
            .await?;
    Ok(row.map(|(version, imported_at)| CompanySource {
        version,
        source_url: SOURCE_PAGE,
        imported_at,
    }))
}

/// A company and the role of one person in it.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct CompanyView {
    /// The first 8 digits of the CNPJ.
    pub cnpj_basico: String,
    /// The legal name.
    pub name: String,
    /// The trade name.
    pub trade_name: Option<String>,
    /// The main activity (CNAE text).
    pub cnae_text: Option<String>,
    /// `active`, `suspended`, `unfit`, `closed`, `null` or `unknown`.
    pub status: String,
    /// The state of the head office.
    pub uf: Option<String>,
    /// The city of the head office.
    pub city: Option<String>,
    /// The share capital in cents.
    pub capital_cents: Option<i64>,
    /// The role of the person, as the Receita writes it.
    pub role: String,
    /// The date when the person became a partner.
    pub since: Option<NaiveDate>,
    /// The other people with the same name in the partner file (other six CPF digits).
    pub namesakes: i32,
}

const COMPANY_COLUMNS: &str = "c.cnpj_basico, c.name, c.trade_name, c.cnae_text, c.status, c.uf,
    c.city, c.capital_cents, l.role, l.since, l.namesakes";

/// The companies of some donors, active companies and the largest capital first.
/// Returns (key → companies, at most `per_donor`) and (key → number of companies).
///
/// # Errors
/// Returns a database error.
pub async fn donor_companies(
    db: &PgPool,
    keys: &[&str],
    per_donor: usize,
) -> Result<(HashMap<String, Vec<CompanyView>>, HashMap<String, i64>), sqlx::Error> {
    #[derive(sqlx::FromRow)]
    struct Row {
        donor_key: String,
        #[sqlx(flatten)]
        company: CompanyView,
    }
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT l.donor_key, {COMPANY_COLUMNS}
           FROM diogenes_donor_company l
           JOIN diogenes_company c ON c.cnpj_basico = l.cnpj_basico
          WHERE l.election_year = $1 AND l.donor_key = ANY($2)
          ORDER BY l.donor_key, (c.status = 'active') DESC, c.capital_cents DESC NULLS LAST,
                   c.name"
    ))
    .bind(ELECTION_YEAR)
    .bind(keys)
    .fetch_all(db)
    .await?;
    let mut out: HashMap<String, Vec<CompanyView>> = HashMap::new();
    let mut counts: HashMap<String, i64> = HashMap::new();
    for r in rows {
        *counts.entry(r.donor_key.clone()).or_default() += 1;
        let list = out.entry(r.donor_key).or_default();
        if list.len() < per_donor {
            list.push(r.company);
        }
    }
    Ok((out, counts))
}

/// The companies where a deputy is a partner.
///
/// # Errors
/// Returns a database error.
pub async fn deputy_companies(
    db: &PgPool,
    camara_id: i32,
) -> Result<Vec<CompanyView>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {COMPANY_COLUMNS}
           FROM diogenes_deputy_company l
           JOIN diogenes_company c ON c.cnpj_basico = l.cnpj_basico
          WHERE l.camara_id = $1
          ORDER BY (c.status = 'active') DESC, c.capital_cents DESC NULLS LAST, c.name"
    ))
    .bind(camara_id)
    .fetch_all(db)
    .await
}

/// The donors of one economic activity in a scenario.
#[derive(Debug, Serialize)]
pub struct Sector {
    /// The main activity of the companies (CNAE text).
    pub cnae_text: String,
    /// The donors that are partners of a company with this activity.
    pub donors: i64,
    /// The money of these donors to the deputies of the scenario, in cents.
    pub cents: i64,
    /// The deputies that got money from these donors, per column id.
    pub by_column: BTreeMap<Uuid, i64>,
}

/// The activities of the companies of the donors, by the number of deputies they reach.
/// A donor with companies in two activities counts in both.
///
/// # Errors
/// Returns a database error.
pub async fn sectors(
    db: &PgPool,
    column_of: &HashMap<i32, Uuid>,
) -> Result<Vec<Sector>, sqlx::Error> {
    let rows: Vec<(String, String, i32, i64)> = sqlx::query_as(
        "SELECT c.cnae_text, d.donor_key, d.camara_id, sum(d.cents)::bigint
           FROM (SELECT DISTINCT l.donor_key, c.cnae_text
                   FROM diogenes_donor_company l
                   JOIN diogenes_company c ON c.cnpj_basico = l.cnpj_basico
                  WHERE l.election_year = $1 AND c.cnae_text IS NOT NULL) c
           JOIN diogenes_donation d ON d.election_year = $1 AND d.donor_key = c.donor_key
          GROUP BY c.cnae_text, d.donor_key, d.camara_id",
    )
    .bind(ELECTION_YEAR)
    .fetch_all(db)
    .await?;
    #[derive(Default)]
    struct Tally {
        donors: HashSet<String>,
        deputies: HashSet<i32>,
        by_column: BTreeMap<Uuid, i64>,
        cents: i64,
    }
    let mut by_sector: HashMap<String, Tally> = HashMap::new();
    for (sector, key, id, cents) in rows {
        let Some(column) = column_of.get(&id) else {
            continue;
        };
        let t = by_sector.entry(sector).or_default();
        t.donors.insert(key);
        t.cents += cents;
        if t.deputies.insert(id) {
            *t.by_column.entry(*column).or_default() += 1;
        }
    }
    let mut out: Vec<Sector> = by_sector
        .into_iter()
        .map(|(cnae_text, t)| Sector {
            cnae_text,
            donors: t.donors.len() as i64,
            cents: t.cents,
            by_column: t.by_column,
        })
        .collect();
    let reach = |s: &Sector| s.by_column.values().sum::<i64>();
    out.sort_by(|a, b| {
        reach(b)
            .cmp(&reach(a))
            .then(b.cents.cmp(&a.cents))
            .then(a.cnae_text.cmp(&b.cnae_text))
    });
    out.truncate(SECTORS);
    Ok(out)
}

/// One deputy in the public list.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct CampaignRow {
    /// The Câmara id.
    pub camara_id: i32,
    /// The parliamentary name.
    pub name: String,
    /// The party.
    pub party: Option<String>,
    /// The state.
    pub uf: Option<String>,
    /// The photo.
    pub photo_url: Option<String>,
    /// All receipts, in cents.
    pub total_cents: i64,
    /// The donations of people straight to the campaign, in cents (source `individuals`,
    /// the same value as in the campaign of the deputy).
    pub people_cents: i64,
    /// The donors that are partners of a company.
    pub donors_with_companies: i64,
    /// The companies where the deputy is a partner.
    pub own_companies: i64,
}

/// The public list.
#[derive(Debug, Serialize)]
pub struct CampaignList {
    /// The election year.
    pub year: i16,
    /// The public page of the TSE data set.
    pub source_url: &'static str,
    /// The Receita data. `None`: no import yet.
    pub companies: Option<CompanySource>,
    /// The deputies in office, largest campaign first.
    pub deputies: Vec<CampaignRow>,
}

/// `GET /diogenes/campaigns`.
pub async fn list(State(state): State<DiogenesState>) -> ApiResult<Json<Envelope<CampaignList>>> {
    let deputies = sqlx::query_as::<_, CampaignRow>(
        "SELECT d.camara_id, d.name, d.party, d.uf::text AS uf, d.photo_url,
                COALESCE((SELECT sum(t.cents) FROM diogenes_campaign_total t
                           WHERE t.election_year = $1 AND t.camara_id = d.camara_id), 0)::bigint
                    AS total_cents,
                COALESCE((SELECT sum(t.cents) FROM diogenes_campaign_total t
                           WHERE t.election_year = $1 AND t.camara_id = d.camara_id
                             AND t.source = 'individuals'), 0)::bigint
                    AS people_cents,
                (SELECT count(DISTINCT n.donor_key) FROM diogenes_donation n
                   JOIN diogenes_donor_company l ON l.election_year = n.election_year
                                                AND l.donor_key = n.donor_key
                  WHERE n.election_year = $1 AND n.camara_id = d.camara_id) AS donors_with_companies,
                (SELECT count(*) FROM diogenes_deputy_company c WHERE c.camara_id = d.camara_id)
                    AS own_companies
           FROM diogenes_deputy d
          WHERE d.in_office
          ORDER BY total_cents DESC, d.name",
    )
    .bind(ELECTION_YEAR)
    .fetch_all(&state.db)
    .await?;
    Ok(ok(CampaignList {
        year: ELECTION_YEAR,
        source_url: crate::finance::SOURCE_PAGE,
        companies: company_source(&state.db).await?,
        deputies,
    }))
}
