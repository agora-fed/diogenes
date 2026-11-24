//! Job `groups`: parliamentary fronts and caucuses.
//!
//! Caucuses:
//! * `caucus:leaders`: current leaders (records with no end date).
//! * `caucus:board`: current members of the board (Mesa Diretora). The chair is here.
//! * `caucus:women`: women in office (from the deputy details).
//! * `caucus:black-brown`: deputies in office who self-declared PRETA or PARDA to the TSE.

use std::collections::BTreeMap;

use sqlx::PgPool;

use super::{Check, JobError, JobReport};
use crate::camara::dto::{
    BodyMember, DeputySummary, FrontDetail, FrontMember, FrontSummary, Leader,
};
use crate::camara::{CamaraClient, BOARD_ID, LEGISLATURE};
use crate::text::front_signatures;

/// A group to write: metadata and members (Câmara id → role).
#[derive(Debug, Clone)]
pub(crate) struct GroupRecord {
    pub(crate) id: String,
    pub(crate) kind: &'static str,
    pub(crate) camara_id: Option<i32>,
    pub(crate) title: String,
    pub(crate) source: &'static str,
    pub(crate) official_total: Option<i64>,
    pub(crate) details: serde_json::Value,
    pub(crate) members: BTreeMap<i32, Option<String>>,
}

/// Run the job.
///
/// # Errors
/// Returns the first API or database error.
pub async fn run(db: &PgPool, client: &CamaraClient) -> Result<JobReport, JobError> {
    let mut report = JobReport::default();

    let leaders: Vec<Leader> = client
        .list(&format!("legislaturas/{LEGISLATURE}/lideres"))
        .await?;
    let current: Vec<DeputySummary> = leaders
        .iter()
        .filter(|l| l.data_fim.is_none())
        .map(|l| l.parlamentar.clone())
        .collect();
    let mut conn = db.acquire().await?;
    super::deputies::upsert_summaries(&mut conn, &current, false).await?;
    drop(conn);
    let leaders = leaders_group(&leaders);
    report.items += write_group(db, &leaders).await?;

    let board: Vec<BodyMember> = client.list(&format!("orgaos/{BOARD_ID}/membros")).await?;
    report.items += write_group(db, &board_group(&board)).await?;

    report.items += write_derived(db).await?;

    let fronts: Vec<FrontSummary> = client
        .list(&format!("frentes?idLegislatura={LEGISLATURE}"))
        .await?;
    let mut errors = 0_i64;
    for front in &fronts {
        // `/frentes/{id}` and `/frentes/{id}/membros` are not paged: the API rejects `itens`.
        let fetched = async {
            let detail: FrontDetail = client.get(&format!("frentes/{}", front.id)).await?;
            let members: Vec<FrontMember> =
                client.get(&format!("frentes/{}/membros", front.id)).await?;
            Ok::<_, crate::camara::CamaraError>((detail, members))
        }
        .await;
        // An API error in one front does not stop the job. The job counts it.
        let (detail, members) = match fetched {
            Ok(pair) => pair,
            Err(err) => {
                tracing::warn!(front = front.id, error = %err, "diogenes front skipped");
                errors += 1;
                continue;
            }
        };
        let listed = members.len();
        let members: Vec<DeputySummary> = members
            .into_iter()
            .filter_map(FrontMember::into_summary)
            .collect();
        let mut conn = db.acquire().await?;
        super::deputies::upsert_summaries(&mut conn, &members, false).await?;
        drop(conn);
        let mut record = front_group(&detail, &members);
        // Entries with no deputy id are not members we can show. Keep their number.
        record.details["unidentified_members"] = serde_json::json!(listed - members.len());
        report.items += write_group(db, &record).await?;
    }
    report.checks.push(Check {
        subject: "front_errors".into(),
        expected: 0,
        actual: errors,
    });
    report.checks.push(Check {
        subject: "fronts_listed".into(),
        expected: fronts.len() as i64,
        actual: sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM diogenes_group WHERE kind = 'front'",
        )
        .fetch_one(db)
        .await?,
    });
    Ok(report)
}

/// Build the leaders caucus. A deputy with two current roles gets both, split by "; ".
pub(crate) fn leaders_group(records: &[Leader]) -> GroupRecord {
    let mut members: BTreeMap<i32, Option<String>> = BTreeMap::new();
    for leader in records.iter().filter(|l| l.data_fim.is_none()) {
        let role = format!("{} · {}", leader.titulo, leader.bancada.nome);
        let entry = members.entry(leader.parlamentar.id).or_insert(None);
        *entry = Some(match entry.take() {
            Some(prev) => format!("{prev}; {role}"),
            None => role,
        });
    }
    GroupRecord {
        id: "caucus:leaders".into(),
        kind: "caucus",
        camara_id: None,
        title: "Líderes e vice-líderes".into(),
        source: "camara",
        official_total: None,
        details: serde_json::json!({}),
        members,
    }
}

/// Build the board caucus from the current members.
///
/// The API lists each member two times: with the specific role ("Presidente") and with a
/// generic role ("Titular" or "Suplente"). The specific role wins.
pub(crate) fn board_group(records: &[BodyMember]) -> GroupRecord {
    let is_generic = |role: &str| matches!(role, "Titular" | "Suplente");
    let mut members: BTreeMap<i32, Option<String>> = BTreeMap::new();
    for m in records.iter().filter(|m| m.data_fim.is_none()) {
        let entry = members.entry(m.id).or_insert(None);
        let keep_old = entry.as_deref().is_some_and(|old| !is_generic(old));
        if !keep_old {
            *entry = m.titulo.clone();
        }
    }
    GroupRecord {
        id: "caucus:board".into(),
        kind: "caucus",
        camara_id: Some(BOARD_ID),
        title: "Mesa Diretora".into(),
        source: "camara",
        official_total: None,
        details: serde_json::json!({}),
        members,
    }
}

/// Build a front group.
pub(crate) fn front_group(detail: &FrontDetail, members: &[DeputySummary]) -> GroupRecord {
    let signatures = detail
        .situacao
        .as_deref()
        .map(front_signatures)
        .unwrap_or_default();
    GroupRecord {
        id: format!("front:{}", detail.id),
        kind: "front",
        camara_id: Some(detail.id),
        title: detail.titulo.trim().to_owned(),
        source: "camara",
        // The official number of a front is its deputy signatures at creation.
        official_total: signatures.at_creation,
        details: serde_json::json!({
            "phone": detail.telefone,
            "document_url": detail.url_documento,
            "coordinator": detail.coordenador.as_ref().and_then(|c| c.id),
            "coordinator_name": detail.coordenador.as_ref().and_then(|c| c.nome.clone()),
            "status": detail.situacao,
            "signatures_net": signatures.net,
        }),
        members: members.iter().map(|m| (m.id, m.titulo.clone())).collect(),
    }
}

async fn write_group(db: &PgPool, group: &GroupRecord) -> Result<i64, sqlx::Error> {
    let mut tx = db.begin().await?;
    sqlx::query(
        "INSERT INTO diogenes_group (id, kind, camara_id, title, source, official_total, details)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (id) DO UPDATE
            SET title = EXCLUDED.title, official_total = EXCLUDED.official_total,
                details = EXCLUDED.details, updated_at = now()",
    )
    .bind(&group.id)
    .bind(group.kind)
    .bind(group.camara_id)
    .bind(&group.title)
    .bind(group.source)
    .bind(group.official_total.and_then(|t| i32::try_from(t).ok()))
    .bind(&group.details)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM diogenes_group_member WHERE group_id = $1")
        .bind(&group.id)
        .execute(&mut *tx)
        .await?;
    let ids: Vec<i32> = group.members.keys().copied().collect();
    let roles: Vec<Option<&str>> = group.members.values().map(Option::as_deref).collect();
    sqlx::query(
        "INSERT INTO diogenes_group_member (group_id, camara_id, role)
         SELECT $1, id, role FROM UNNEST($2::int[], $3::text[]) AS t(id, role)",
    )
    .bind(&group.id)
    .bind(&ids)
    .bind(&roles)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(group.members.len() as i64)
}

/// Write the caucuses that come from the deputy table.
async fn write_derived(db: &PgPool) -> Result<i64, sqlx::Error> {
    let derived = [
        (
            "caucus:women",
            "Bancada feminina (deputadas em exercício)",
            "camara",
            "in_office AND sex = 'F'",
        ),
        (
            "caucus:black-brown",
            "Autodeclarados pretos e pardos (TSE 2022)",
            "tse",
            "in_office AND race IN ('PRETA', 'PARDA')",
        ),
    ];
    let mut items = 0;
    for (id, title, source, filter) in derived {
        let mut tx = db.begin().await?;
        sqlx::query(
            "INSERT INTO diogenes_group (id, kind, title, source) VALUES ($1, 'caucus', $2, $3)
             ON CONFLICT (id) DO UPDATE SET title = EXCLUDED.title, updated_at = now()",
        )
        .bind(id)
        .bind(title)
        .bind(source)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM diogenes_group_member WHERE group_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        // `filter` is a constant from the list above, never user input.
        let insert = format!(
            "INSERT INTO diogenes_group_member (group_id, camara_id)
             SELECT $1, camara_id FROM diogenes_deputy WHERE {filter}"
        );
        let done = sqlx::query(&insert).bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        items += done.rows_affected() as i64;
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(id: i32, titulo: Option<&str>) -> DeputySummary {
        DeputySummary {
            id,
            nome: format!("D{id}"),
            sigla_partido: None,
            sigla_uf: None,
            url_foto: None,
            email: None,
            titulo: titulo.map(str::to_owned),
        }
    }

    #[test]
    fn leaders_keep_only_current_and_merge_roles() {
        let make = |id, titulo: &str, bench: &str, end: Option<&str>| Leader {
            parlamentar: summary(id, None),
            titulo: titulo.into(),
            bancada: crate::camara::dto::LeaderBench {
                tipo: None,
                nome: bench.into(),
            },
            data_fim: end.map(str::to_owned),
        };
        let g = leaders_group(&[
            make(1, "Líder", "PT", None),
            make(1, "Vice-Líder", "Governo", None),
            make(2, "Líder", "PL", Some("2025-01-01")),
        ]);
        assert_eq!(g.members.len(), 1);
        assert_eq!(
            g.members.get(&1).cloned().flatten().as_deref(),
            Some("Líder · PT; Vice-Líder · Governo")
        );
    }

    #[test]
    fn front_reads_official_total() {
        let detail = FrontDetail {
            id: 7,
            titulo: " Frente X ".into(),
            situacao: Some("Confirmadas 2 assinaturas de deputados".into()),
            telefone: None,
            url_documento: None,
            coordenador: None,
        };
        let g = front_group(
            &detail,
            &[summary(1, Some("Coordenador")), summary(2, None)],
        );
        assert_eq!(g.id, "front:7");
        assert_eq!(g.title, "Frente X");
        assert_eq!(g.official_total, Some(2));
        assert_eq!(g.members.len(), 2);
    }

    #[test]
    fn board_ignores_past_members() {
        let g = board_group(&[
            BodyMember {
                id: 1,
                nome: "A".into(),
                titulo: Some("Presidente".into()),
                data_fim: None,
            },
            BodyMember {
                id: 2,
                nome: "B".into(),
                titulo: Some("Presidente".into()),
                data_fim: Some("2025-01-31".into()),
            },
        ]);
        assert_eq!(g.members.len(), 1);
    }

    #[test]
    fn board_specific_role_wins_over_generic() {
        let member = |titulo: &str| BodyMember {
            id: 1,
            nome: "A".into(),
            titulo: Some(titulo.into()),
            data_fim: None,
        };
        for order in [["Presidente", "Titular"], ["Titular", "Presidente"]] {
            let g = board_group(&[member(order[0]), member(order[1])]);
            assert_eq!(
                g.members.get(&1).cloned().flatten().as_deref(),
                Some("Presidente")
            );
        }
    }
}
