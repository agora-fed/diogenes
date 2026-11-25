//! Job `roll_calls`: plenary roll calls, with votes, orientations and bills.
//!
//! The API accepts a date window of at most 3 months. The job splits the period into
//! windows of [`WINDOW_DAYS`]. A symbolic vote has an empty vote list: the job stores the
//! roll call with no votes. The job fetches votes again only for recent roll calls.

use chrono::{Days, NaiveDate, Utc};
use sqlx::PgPool;

use super::{Check, JobError, JobReport};
use crate::camara::dto::{Orientation, RollCallDetail, RollCallSummary, Vote};
use crate::camara::{CamaraClient, PLENARY_ID};
use crate::text::{DescribedTotals, VoteKind};

/// The size of one date window, in days. The API limit is 3 months.
pub const WINDOW_DAYS: u64 = 85;
/// Roll calls younger than this are fetched again (late corrections).
const REFRESH_DAYS: u64 = 3;

/// Split `[since, until]` into windows of at most [`WINDOW_DAYS`].
#[must_use]
pub fn windows(since: NaiveDate, until: NaiveDate) -> Vec<(NaiveDate, NaiveDate)> {
    let mut out = Vec::new();
    let mut start = since;
    while start <= until {
        let end = (start + Days::new(WINDOW_DAYS - 1)).min(until);
        out.push((start, end));
        start = end + Days::new(1);
    }
    out
}

/// Run the job.
///
/// # Errors
/// Returns the first API or database error.
pub async fn run(
    db: &PgPool,
    client: &CamaraClient,
    since: NaiveDate,
) -> Result<JobReport, JobError> {
    let mut report = JobReport::default();
    let mut errors = 0_i64;
    let today = Utc::now().date_naive();
    let refresh_after = today - Days::new(REFRESH_DAYS);
    for (start, end) in windows(since, today) {
        let path = format!(
            "votacoes?idOrgao={PLENARY_ID}&dataInicio={start}&dataFim={end}&ordem=ASC&ordenarPor=dataHoraRegistro"
        );
        let list: Vec<RollCallSummary> = client.list(&path).await?;
        for summary in list {
            let Ok(voted_on) = NaiveDate::parse_from_str(&summary.data, "%Y-%m-%d") else {
                continue;
            };
            if voted_on < refresh_after && is_stored(db, &summary.id).await? {
                continue;
            }
            // An error in one roll call does not stop the job. The job counts it.
            match sync_one(db, client, &summary, voted_on).await {
                Ok(check) => {
                    report.checks.extend(check);
                    report.items += 1;
                }
                Err(JobError::Db(err)) => return Err(JobError::Db(err)),
                Err(err) => {
                    tracing::warn!(roll_call = %summary.id, error = %err, "diogenes roll call skipped");
                    errors += 1;
                }
            }
        }
    }
    report.checks.push(Check {
        subject: "roll_call_errors".into(),
        expected: 0,
        actual: errors,
    });
    Ok(report)
}

/// Get a list. HTTP 404 means "no items" (the API sends 404 for some old roll calls).
async fn list_or_empty<T: serde::de::DeserializeOwned>(
    client: &CamaraClient,
    path: &str,
) -> Result<Vec<T>, crate::camara::CamaraError> {
    match client.get::<Vec<T>>(path).await {
        Err(crate::camara::CamaraError::Status { status: 404, .. }) => Ok(Vec::new()),
        other => other,
    }
}

async fn is_stored(db: &PgPool, id: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM diogenes_roll_call WHERE id = $1)")
        .bind(id)
        .fetch_one(db)
        .await
}

/// Fetch and store one roll call. Return the totals check when the description has totals.
async fn sync_one(
    db: &PgPool,
    client: &CamaraClient,
    summary: &RollCallSummary,
    voted_on: NaiveDate,
) -> Result<Option<Check>, JobError> {
    let votes: Vec<Vote> = list_or_empty(client, &format!("votacoes/{}/votos", summary.id)).await?;
    let (orientations, bill) = if votes.is_empty() {
        (Vec::new(), None)
    } else {
        let o: Vec<Orientation> =
            list_or_empty(client, &format!("votacoes/{}/orientacoes", summary.id)).await?;
        let d: RollCallDetail = client.get(&format!("votacoes/{}", summary.id)).await?;
        (o, d.proposicoes_afetadas.into_iter().next())
    };
    let description = summary.descricao.clone().unwrap_or_default();
    let totals = DescribedTotals::parse(&description);
    let check = totals
        .filter(|_| !votes.is_empty())
        .map(|t| totals_check(&summary.id, t, &votes));
    let totals_match = check.as_ref().map(Check::passed);

    let mut tx = db.begin().await?;
    if let Some(b) = &bill {
        sqlx::query(
            "INSERT INTO diogenes_bill (camara_id, kind, number, year, summary)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (camara_id) DO UPDATE SET summary = EXCLUDED.summary, updated_at = now()",
        )
        .bind(b.id)
        .bind(&b.sigla_tipo)
        .bind(b.numero)
        .bind(b.ano)
        .bind(b.ementa.as_deref().unwrap_or(""))
        .execute(&mut *tx)
        .await?;
    }
    let registered_at = summary
        .data_hora_registro
        .as_deref()
        .and_then(|s| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok())
        // The API sends Brasília time (UTC-3) with no offset.
        .map(|t| (t + chrono::Duration::hours(3)).and_utc());
    sqlx::query(
        "INSERT INTO diogenes_roll_call
            (id, voted_on, registered_at, body, description, approved, bill_id, totals_match)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (id) DO UPDATE
            SET description = EXCLUDED.description, approved = EXCLUDED.approved,
                bill_id = COALESCE(EXCLUDED.bill_id, diogenes_roll_call.bill_id),
                totals_match = EXCLUDED.totals_match, updated_at = now()",
    )
    .bind(&summary.id)
    .bind(voted_on)
    .bind(registered_at)
    .bind(summary.sigla_orgao.as_deref().unwrap_or("PLEN"))
    .bind(&description)
    .bind(summary.aprovacao.map(|a| a == 1))
    .bind(bill.as_ref().map(|b| b.id))
    .bind(totals_match)
    .execute(&mut *tx)
    .await?;

    let voters: Vec<_> = votes.iter().map(|v| v.deputado.clone()).collect();
    super::deputies::upsert_summaries(&mut tx, &voters, false).await?;
    sqlx::query("DELETE FROM diogenes_roll_call_vote WHERE roll_call_id = $1")
        .bind(&summary.id)
        .execute(&mut *tx)
        .await?;
    let ids: Vec<i32> = votes.iter().map(|v| v.deputado.id).collect();
    let kinds: Vec<&str> = votes.iter().map(|v| v.tipo_voto.as_str()).collect();
    let parties: Vec<&str> = votes
        .iter()
        .map(|v| v.deputado.sigla_partido.as_deref().unwrap_or(""))
        .collect();
    let ufs: Vec<&str> = votes
        .iter()
        .map(|v| v.deputado.sigla_uf.as_deref().unwrap_or("--"))
        .collect();
    sqlx::query(
        "INSERT INTO diogenes_roll_call_vote (roll_call_id, camara_id, vote, party, uf)
         SELECT $1, id, vote, party, uf
           FROM UNNEST($2::int[], $3::text[], $4::text[], $5::text[]) AS t(id, vote, party, uf)
         ON CONFLICT DO NOTHING",
    )
    .bind(&summary.id)
    .bind(&ids)
    .bind(&kinds)
    .bind(&parties)
    .bind(&ufs)
    .execute(&mut *tx)
    .await?;

    sqlx::query("DELETE FROM diogenes_roll_call_orientation WHERE roll_call_id = $1")
        .bind(&summary.id)
        .execute(&mut *tx)
        .await?;
    let blocs: Vec<&str> = orientations
        .iter()
        .map(|o| o.sigla_partido_bloco.as_str())
        .collect();
    let bloc_kinds: Vec<Option<&str>> = orientations
        .iter()
        .map(|o| o.cod_tipo_lideranca.as_deref())
        .collect();
    let values: Vec<&str> = orientations
        .iter()
        .map(|o| o.orientacao_voto.as_str())
        .collect();
    sqlx::query(
        "INSERT INTO diogenes_roll_call_orientation (roll_call_id, bloc, bloc_kind, orientation)
         SELECT $1, b, k, o FROM UNNEST($2::text[], $3::text[], $4::text[]) AS t(b, k, o)
         ON CONFLICT DO NOTHING",
    )
    .bind(&summary.id)
    .bind(&blocs)
    .bind(&bloc_kinds)
    .bind(&values)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(check)
}

/// Compare the stored votes with the totals in the description ("Sim" + "Não" + "Abstenção").
#[must_use]
pub fn totals_check(id: &str, totals: DescribedTotals, votes: &[Vote]) -> Check {
    let count = |kind: VoteKind| {
        votes
            .iter()
            .filter(|v| VoteKind::parse(&v.tipo_voto) == kind)
            .count() as i64
    };
    let expected =
        totals.yes.unwrap_or(0) + totals.no.unwrap_or(0) + totals.abstention.unwrap_or(0);
    let actual = count(VoteKind::Yes)
        + count(VoteKind::No)
        + if totals.abstention.is_some() {
            count(VoteKind::Abstention)
        } else {
            0
        };
    Check {
        subject: format!("roll_call:{id}"),
        expected,
        actual,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap_or_default()
    }

    #[test]
    fn windows_cover_the_period_without_gaps() {
        let w = windows(date("2026-01-01"), date("2026-11-24"));
        assert_eq!(w.first().map(|x| x.0), Some(date("2026-01-01")));
        assert_eq!(w.last().map(|x| x.1), Some(date("2026-11-24")));
        for pair in w.windows(2) {
            assert_eq!(pair[0].1 + Days::new(1), pair[1].0);
        }
        for (a, b) in &w {
            assert!((*b - *a).num_days() < WINDOW_DAYS as i64);
        }
    }

    #[test]
    fn windows_empty_when_since_is_after_until() {
        assert!(windows(date("2026-02-01"), date("2026-01-01")).is_empty());
    }

    fn vote(kind: &str, id: i32) -> Vote {
        Vote {
            tipo_voto: kind.to_owned(),
            deputado: crate::camara::dto::DeputySummary {
                id,
                nome: format!("D{id}"),
                sigla_partido: None,
                sigla_uf: None,
                url_foto: None,
                email: None,
                titulo: None,
            },
        }
    }

    #[test]
    fn totals_check_counts_abstentions_only_when_the_description_has_them() {
        let votes = [vote("Sim", 1), vote("Não", 2), vote("Abstenção", 3)];
        let without = DescribedTotals {
            yes: Some(1),
            no: Some(1),
            abstention: None,
        };
        let with = DescribedTotals {
            abstention: Some(1),
            ..without
        };

        let check_without = totals_check("r", without, &votes);
        let check_with = totals_check("r", with, &votes);

        assert_eq!(check_without.subject, "roll_call:r");
        assert!(
            check_without.passed(),
            "the abstention is not in the description"
        );
        assert_eq!((check_with.expected, check_with.actual), (3, 3));
        let short = totals_check("r", with, &votes[..2]);
        assert!(!short.passed());
    }
}
