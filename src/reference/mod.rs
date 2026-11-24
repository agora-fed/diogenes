//! Public read API for the reference data. No sign-in is necessary: all data is public.
//! The raw colour/race value of a deputy is not in any response. Only the caucus
//! membership shows it, as the TSE publishes it.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::error::{ok, ApiError, ApiResult, Envelope};
use crate::text::{orientation_for, Orientation, VoteKind};
use crate::DiogenesState;

const MAX_ROLL_CALLS: i64 = 200;
const DEFAULT_ROLL_CALLS: i64 = 50;

pub(crate) fn routes() -> Router<DiogenesState> {
    Router::new()
        .route("/diogenes/deputies", get(deputies))
        .route("/diogenes/groups", get(groups))
        .route("/diogenes/groups/{id}", get(group))
        .route("/diogenes/roll-calls", get(roll_calls))
        .route("/diogenes/roll-calls/{id}", get(roll_call))
}

/// A deputy in office.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DeputyView {
    /// The official Câmara id.
    pub camara_id: i32,
    /// The parliamentary name.
    pub name: String,
    /// The party acronym.
    pub party: String,
    /// The state.
    pub uf: String,
    /// The photo URL.
    pub photo_url: Option<String>,
    /// The ids of the groups of the deputy.
    pub groups: Vec<String>,
    /// The current role in the board, for example "Presidente".
    pub board_role: Option<String>,
    /// The current leader roles.
    pub leader_role: Option<String>,
}

/// Load the deputies in office, in name order.
///
/// # Errors
/// Returns a database error.
pub async fn load_deputies(db: &PgPool) -> Result<Vec<DeputyView>, sqlx::Error> {
    sqlx::query_as::<_, DeputyView>(
        "SELECT d.camara_id, d.name, d.party, d.uf, d.photo_url,
                COALESCE(array_agg(m.group_id ORDER BY m.group_id)
                         FILTER (WHERE m.group_id IS NOT NULL), '{}') AS groups,
                max(m.role) FILTER (WHERE m.group_id = 'caucus:board') AS board_role,
                max(m.role) FILTER (WHERE m.group_id = 'caucus:leaders') AS leader_role
           FROM diogenes_deputy d
           LEFT JOIN diogenes_group_member m ON m.camara_id = d.camara_id
          WHERE d.in_office
          GROUP BY d.camara_id
          ORDER BY d.name",
    )
    .fetch_all(db)
    .await
}

/// The Câmara id of the current chair (board member with the role "Presidente").
///
/// # Errors
/// Returns a database error.
pub async fn current_chair(db: &PgPool) -> Result<Option<i32>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT camara_id FROM diogenes_group_member
          WHERE group_id = 'caucus:board' AND role = 'Presidente' LIMIT 1",
    )
    .fetch_optional(db)
    .await
}

async fn deputies(
    State(state): State<DiogenesState>,
) -> ApiResult<Json<Envelope<Vec<DeputyView>>>> {
    Ok(ok(load_deputies(&state.db).await?))
}

/// A group summary.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct GroupView {
    /// The group id, for example `front:55703`.
    pub id: String,
    /// `front` or `caucus`.
    pub kind: String,
    /// The title.
    pub title: String,
    /// `camara` or `tse`.
    pub source: String,
    /// The total that the source publishes.
    pub official_total: Option<i32>,
    /// The number of members that DIOGENES stored.
    pub member_count: i64,
    /// The number of members in office.
    pub in_office_count: i64,
}

async fn groups(State(state): State<DiogenesState>) -> ApiResult<Json<Envelope<Vec<GroupView>>>> {
    let rows = sqlx::query_as::<_, GroupView>(
        "SELECT g.id, g.kind, g.title, g.source, g.official_total,
                count(m.camara_id) AS member_count,
                count(m.camara_id) FILTER (WHERE d.in_office) AS in_office_count
           FROM diogenes_group g
           LEFT JOIN diogenes_group_member m ON m.group_id = g.id
           LEFT JOIN diogenes_deputy d ON d.camara_id = m.camara_id
          GROUP BY g.id
          ORDER BY g.kind DESC, count(m.camara_id) DESC, g.title",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(ok(rows))
}

/// A group with its members.
#[derive(Debug, Serialize)]
pub struct GroupDetail {
    /// The summary.
    #[serde(flatten)]
    pub group: GroupView,
    /// Extra data from the source (front phone, document, status).
    pub details: serde_json::Value,
    /// The members.
    pub members: Vec<GroupMember>,
}

/// A member of a group.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct GroupMember {
    /// The Câmara id.
    pub camara_id: i32,
    /// The name, when the deputy is known.
    pub name: Option<String>,
    /// The role in the group.
    pub role: Option<String>,
    /// True when the deputy is in office.
    pub in_office: bool,
}

async fn group(
    State(state): State<DiogenesState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Envelope<GroupDetail>>> {
    let view = sqlx::query_as::<_, GroupView>(
        "SELECT g.id, g.kind, g.title, g.source, g.official_total,
                count(m.camara_id) AS member_count,
                count(m.camara_id) FILTER (WHERE d.in_office) AS in_office_count
           FROM diogenes_group g
           LEFT JOIN diogenes_group_member m ON m.group_id = g.id
           LEFT JOIN diogenes_deputy d ON d.camara_id = m.camara_id
          WHERE g.id = $1
          GROUP BY g.id",
    )
    .bind(&id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)?;
    let details: serde_json::Value =
        sqlx::query_scalar("SELECT details FROM diogenes_group WHERE id = $1")
            .bind(&id)
            .fetch_one(&state.db)
            .await?;
    let members = sqlx::query_as::<_, GroupMember>(
        "SELECT m.camara_id, d.name, m.role, COALESCE(d.in_office, false) AS in_office
           FROM diogenes_group_member m
           LEFT JOIN diogenes_deputy d ON d.camara_id = m.camara_id
          WHERE m.group_id = $1
          ORDER BY d.name NULLS LAST",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await?;
    Ok(ok(GroupDetail {
        group: view,
        details,
        members,
    }))
}

#[derive(Debug, Deserialize)]
struct RollCallQuery {
    limit: Option<i64>,
    bill: Option<i32>,
}

/// A roll call summary.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct RollCallView {
    /// The official id.
    pub id: String,
    /// The date.
    pub voted_on: chrono::NaiveDate,
    /// The description.
    pub description: String,
    /// True = approved; false = rejected; null = unknown.
    pub approved: Option<bool>,
    /// The bill id.
    pub bill_id: Option<i32>,
    /// The bill label, for example "PLP 299/2026".
    pub bill_label: Option<String>,
    /// The number of individual votes.
    pub vote_count: i64,
    /// Result of the check against the described totals.
    pub totals_match: Option<bool>,
}

async fn roll_calls(
    State(state): State<DiogenesState>,
    Query(q): Query<RollCallQuery>,
) -> ApiResult<Json<Envelope<Vec<RollCallView>>>> {
    let limit = q
        .limit
        .unwrap_or(DEFAULT_ROLL_CALLS)
        .clamp(1, MAX_ROLL_CALLS);
    let rows = sqlx::query_as::<_, RollCallView>(
        "SELECT r.id, r.voted_on, r.description, r.approved, r.bill_id,
                b.kind || ' ' || b.number || '/' || b.year AS bill_label,
                (SELECT count(*) FROM diogenes_roll_call_vote v WHERE v.roll_call_id = r.id) AS vote_count,
                r.totals_match
           FROM diogenes_roll_call r
           LEFT JOIN diogenes_bill b ON b.camara_id = r.bill_id
          WHERE EXISTS (SELECT 1 FROM diogenes_roll_call_vote v WHERE v.roll_call_id = r.id)
            AND ($2::int IS NULL OR r.bill_id = $2)
          ORDER BY r.voted_on DESC, r.registered_at DESC NULLS LAST
          LIMIT $1",
    )
    .bind(limit)
    .bind(q.bill)
    .fetch_all(&state.db)
    .await?;
    Ok(ok(rows))
}

/// One vote of a roll call.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct VoteView {
    /// The Câmara id.
    pub camara_id: i32,
    /// The name.
    pub name: String,
    /// The party at the time of the vote.
    pub party: String,
    /// The state.
    pub uf: String,
    /// The vote, as the API sends it.
    pub vote: String,
}

/// A roll call with votes, orientations and dissidents.
#[derive(Debug, Serialize)]
pub struct RollCallDetailView {
    /// The summary.
    #[serde(flatten)]
    pub roll_call: RollCallView,
    /// The bill summary ("ementa").
    pub bill_summary: Option<String>,
    /// Orientations: (bloc, orientation).
    pub orientations: Vec<(String, String)>,
    /// Count per vote value.
    pub counts: HashMap<String, i64>,
    /// Votes.
    pub votes: Vec<VoteView>,
    /// Câmara ids of the deputies who voted against the orientation of their party or bloc.
    pub dissidents: Vec<i32>,
}

/// Load the votes and orientations of a roll call.
///
/// # Errors
/// Returns a database error.
pub async fn load_votes(
    db: &PgPool,
    id: &str,
) -> Result<(Vec<VoteView>, Vec<(String, String)>), sqlx::Error> {
    let votes = sqlx::query_as::<_, VoteView>(
        "SELECT v.camara_id, COALESCE(d.name, '?') AS name, v.party, v.uf, v.vote
           FROM diogenes_roll_call_vote v
           LEFT JOIN diogenes_deputy d ON d.camara_id = v.camara_id
          WHERE v.roll_call_id = $1
          ORDER BY name",
    )
    .bind(id)
    .fetch_all(db)
    .await?;
    let orientations: Vec<(String, String)> = sqlx::query_as(
        "SELECT bloc, orientation FROM diogenes_roll_call_orientation WHERE roll_call_id = $1 ORDER BY bloc",
    )
    .bind(id)
    .fetch_all(db)
    .await?;
    Ok((votes, orientations))
}

/// The deputies who voted "Sim" or "Não" against a "Sim" or "Não" orientation.
#[must_use]
pub fn dissidents(votes: &[VoteView], orientations: &[(String, String)]) -> Vec<i32> {
    votes
        .iter()
        .filter(|v| {
            let vote = VoteKind::parse(&v.vote);
            let wanted = match orientation_for(&v.party, orientations) {
                Some(Orientation::Yes) => VoteKind::Yes,
                Some(Orientation::No) => VoteKind::No,
                _ => return false,
            };
            matches!(vote, VoteKind::Yes | VoteKind::No) && vote != wanted
        })
        .map(|v| v.camara_id)
        .collect()
}

async fn roll_call(
    State(state): State<DiogenesState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Envelope<RollCallDetailView>>> {
    let view = sqlx::query_as::<_, RollCallView>(
        "SELECT r.id, r.voted_on, r.description, r.approved, r.bill_id,
                b.kind || ' ' || b.number || '/' || b.year AS bill_label,
                (SELECT count(*) FROM diogenes_roll_call_vote v WHERE v.roll_call_id = r.id) AS vote_count,
                r.totals_match
           FROM diogenes_roll_call r
           LEFT JOIN diogenes_bill b ON b.camara_id = r.bill_id
          WHERE r.id = $1",
    )
    .bind(&id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)?;
    let bill_summary: Option<String> = match view.bill_id {
        Some(b) => {
            sqlx::query_scalar("SELECT summary FROM diogenes_bill WHERE camara_id = $1")
                .bind(b)
                .fetch_optional(&state.db)
                .await?
        }
        None => None,
    };
    let (votes, orientations) = load_votes(&state.db, &id).await?;
    let mut counts = HashMap::new();
    for v in &votes {
        *counts.entry(v.vote.clone()).or_insert(0) += 1;
    }
    let dissidents = dissidents(&votes, &orientations);
    Ok(ok(RollCallDetailView {
        roll_call: view,
        bill_summary,
        orientations,
        counts,
        votes,
        dissidents,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vote(id: i32, party: &str, vote: &str) -> VoteView {
        VoteView {
            camara_id: id,
            name: String::new(),
            party: party.into(),
            uf: "SP".into(),
            vote: vote.into(),
        }
    }

    #[test]
    fn dissidents_only_count_yes_no_against_yes_no() {
        let o = vec![
            ("PT".to_owned(), "Sim".to_owned()),
            ("PL".to_owned(), "Liberado".to_owned()),
        ];
        let votes = vec![
            vote(1, "PT", "Sim"),
            vote(2, "PT", "Não"),
            vote(3, "PT", "Abstenção"),
            vote(4, "PL", "Não"),
            vote(5, "NOVO", "Não"),
        ];
        assert_eq!(dissidents(&votes, &o), vec![2]);
    }
}
