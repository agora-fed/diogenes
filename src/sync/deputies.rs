//! Job `deputies`: deputies in office, their details, parties and TSE colour/race.

use std::collections::HashMap;

use sqlx::PgPool;

use super::{Check, JobError, JobReport};
use crate::camara::dto::{DeputyDetail, DeputySummary, Party};
use crate::camara::{CamaraClient, LEGISLATURE};
use crate::scoreboard::SEATS;

/// Run the job.
///
/// # Errors
/// Returns the first API or database error. The TSE import is optional: when it fails,
/// the job logs the error and keeps the race values from the last good run.
pub async fn run(
    db: &PgPool,
    client: &CamaraClient,
    tse_enabled: bool,
) -> Result<JobReport, JobError> {
    let mut report = JobReport::default();

    let parties: Vec<Party> = client
        .list(&format!("partidos?idLegislatura={LEGISLATURE}"))
        .await?;
    upsert_parties(db, &parties).await?;
    report.items += parties.len() as i64;

    // Without filters, `/deputados` returns the deputies in office now.
    let in_office: Vec<DeputySummary> = client.list("deputados").await?;
    upsert_in_office(db, &in_office).await?;
    report.items += in_office.len() as i64;
    report.checks.push(Check {
        subject: "deputies_in_office".into(),
        expected: SEATS,
        actual: in_office.len() as i64,
    });

    let races = if tse_enabled {
        match crate::tse::fetch_races_2022().await {
            Ok(map) => Some(map),
            Err(err) => {
                tracing::warn!(error = %err, "diogenes tse import failed; keep old values");
                None
            }
        }
    } else {
        None
    };

    let mut women = 0_i64;
    let mut race_hits = 0_i64;
    for deputy in &in_office {
        let detail: DeputyDetail = client.get(&format!("deputados/{}", deputy.id)).await?;
        let race = races
            .as_ref()
            .and_then(|map| race_for(map, detail.cpf.as_deref()));
        if race.is_some() {
            race_hits += 1;
        }
        if detail.sexo.as_deref() == Some("F") {
            women += 1;
        }
        update_detail(db, &detail, race.as_deref()).await?;
    }

    // Cross-check: the API filter for women must agree with the detail records.
    let women_list: Vec<DeputySummary> = client.list("deputados?siglaSexo=F").await?;
    report.checks.push(Check {
        subject: "women_in_office".into(),
        expected: women_list.len() as i64,
        actual: women,
    });
    if races.is_some() {
        report.checks.push(Check {
            subject: "tse_race_matches".into(),
            expected: in_office.len() as i64,
            actual: race_hits,
        });
    }
    Ok(report)
}

fn race_for(map: &HashMap<String, String>, cpf: Option<&str>) -> Option<String> {
    let digits: String = cpf?.chars().filter(char::is_ascii_digit).collect();
    map.get(&digits).cloned()
}

async fn upsert_parties(db: &PgPool, parties: &[Party]) -> Result<(), sqlx::Error> {
    let ids: Vec<i32> = parties.iter().map(|p| p.id).collect();
    let acronyms: Vec<&str> = parties.iter().map(|p| p.sigla.as_str()).collect();
    let names: Vec<&str> = parties.iter().map(|p| p.nome.as_str()).collect();
    sqlx::query(
        "INSERT INTO diogenes_party (camara_id, acronym, name)
         SELECT * FROM UNNEST($1::int[], $2::text[], $3::text[])
         ON CONFLICT (camara_id) DO UPDATE
            SET acronym = EXCLUDED.acronym, name = EXCLUDED.name, updated_at = now()",
    )
    .bind(&ids)
    .bind(&acronyms)
    .bind(&names)
    .execute(db)
    .await?;
    Ok(())
}

/// Upsert the deputies in office. Every other deputy becomes "not in office".
pub(crate) async fn upsert_in_office(
    db: &PgPool,
    deputies: &[DeputySummary],
) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;
    sqlx::query("UPDATE diogenes_deputy SET in_office = false WHERE in_office")
        .execute(&mut *tx)
        .await?;
    upsert_summaries(&mut tx, deputies, true).await?;
    tx.commit().await
}

/// Upsert deputy summaries. `in_office` = true marks them as in office.
pub(crate) async fn upsert_summaries(
    conn: &mut sqlx::PgConnection,
    deputies: &[DeputySummary],
    in_office: bool,
) -> Result<(), sqlx::Error> {
    // One deputy can be in a list two times (a leader with two roles). PostgreSQL rejects
    // an upsert that changes one row two times, so keep the last record of each id.
    let mut last: std::collections::BTreeMap<i32, &DeputySummary> =
        std::collections::BTreeMap::new();
    for d in deputies {
        last.insert(d.id, d);
    }
    let deputies: Vec<&DeputySummary> = last.into_values().collect();
    let ids: Vec<i32> = deputies.iter().map(|d| d.id).collect();
    let names: Vec<&str> = deputies.iter().map(|d| d.nome.as_str()).collect();
    let parties: Vec<&str> = deputies
        .iter()
        .map(|d| d.sigla_partido.as_deref().unwrap_or(""))
        .collect();
    let ufs: Vec<&str> = deputies
        .iter()
        .map(|d| d.sigla_uf.as_deref().unwrap_or("--"))
        .collect();
    let photos: Vec<Option<&str>> = deputies.iter().map(|d| d.url_foto.as_deref()).collect();
    let emails: Vec<Option<&str>> = deputies.iter().map(|d| d.email.as_deref()).collect();
    sqlx::query(
        "INSERT INTO diogenes_deputy (camara_id, name, party, uf, photo_url, email, in_office)
         SELECT id, name, party, uf, photo, email, $7
           FROM UNNEST($1::int[], $2::text[], $3::text[], $4::text[], $5::text[], $6::text[])
                AS t(id, name, party, uf, photo, email)
         ON CONFLICT (camara_id) DO UPDATE
            SET name = EXCLUDED.name, party = EXCLUDED.party, uf = EXCLUDED.uf,
                photo_url = COALESCE(EXCLUDED.photo_url, diogenes_deputy.photo_url),
                email = COALESCE(EXCLUDED.email, diogenes_deputy.email),
                in_office = diogenes_deputy.in_office OR EXCLUDED.in_office,
                updated_at = now()",
    )
    .bind(&ids)
    .bind(&names)
    .bind(&parties)
    .bind(&ufs)
    .bind(&photos)
    .bind(&emails)
    .bind(in_office)
    .execute(conn)
    .await?;
    Ok(())
}

async fn update_detail(
    db: &PgPool,
    detail: &DeputyDetail,
    race: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE diogenes_deputy
            SET civil_name = $2, sex = $3, status = $4,
                race = COALESCE($5, race), updated_at = now()
          WHERE camara_id = $1",
    )
    .bind(detail.id)
    .bind(detail.nome_civil.as_deref())
    .bind(detail.sexo.as_deref().filter(|s| s.len() == 1))
    .bind(
        detail
            .ultimo_status
            .as_ref()
            .and_then(|s| s.situacao.as_deref()),
    )
    .bind(race)
    .execute(db)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::race_for;
    use std::collections::HashMap;

    #[test]
    fn race_lookup_ignores_formatting() {
        let map = HashMap::from([("12345678901".to_owned(), "PARDA".to_owned())]);
        assert_eq!(
            race_for(&map, Some("123.456.789-01")).as_deref(),
            Some("PARDA")
        );
        assert_eq!(race_for(&map, None), None);
        assert_eq!(race_for(&map, Some("000")), None);
    }
}
