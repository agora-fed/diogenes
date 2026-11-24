//! Scenarios: positions, moves and the scoreboard.

use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::http::request::Parts;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::access::{actor, audit, clean_name, require_project, require_scenario, Role};
use super::projects::{insert_custom_column, load_columns, publish, ColumnView};
use crate::error::{ok, ApiError, ApiResult, Envelope};
use crate::reference::{current_chair, dissidents, load_votes};
use crate::scoreboard::{outcome, Counts, Outcome};
use crate::text::VoteKind;
use crate::DiogenesState;

const NAME_MAX: usize = 120;
const MOVES_MAX: usize = 600;
const ROLL_CALL_ID_MAX: usize = 32;

#[derive(Debug, Deserialize)]
pub(super) struct NewScenario {
    name: String,
    /// Copy the positions of this scenario.
    copy_from: Option<Uuid>,
    /// Fill the positions from this roll call.
    roll_call: Option<String>,
    /// With `roll_call`: put deputies in office who did not vote in "absent".
    #[serde(default = "yes")]
    absent_if_missing: bool,
}

const fn yes() -> bool {
    true
}

pub(super) async fn create(
    State(state): State<DiogenesState>,
    Path(project_id): Path<Uuid>,
    parts: Parts,
    Json(body): Json<NewScenario>,
) -> ApiResult<Json<Envelope<Uuid>>> {
    let me = actor(&state, &parts).await?;
    let access = require_project(&state.db, &me, project_id, Role::Editor).await?;
    let name = clean_name(&body.name, NAME_MAX)?;
    if body.copy_from.is_some() && body.roll_call.is_some() {
        return Err(ApiError::Invalid(
            "Escolha cópia ou votação, não as duas.".into(),
        ));
    }
    if body
        .roll_call
        .as_ref()
        .is_some_and(|r| r.is_empty() || r.len() > ROLL_CALL_ID_MAX)
    {
        return Err(ApiError::Invalid("Votação inválida.".into()));
    }
    let columns = load_columns(&state.db, project_id).await?;
    let key_col = |key: &str| {
        columns
            .iter()
            .find(|c| c.key.as_deref() == Some(key))
            .map(|c| c.id)
    };
    let id = Uuid::now_v7();

    let mut tx = state.db.begin().await?;
    sqlx::query(
        "INSERT INTO diogenes_scenario (id, project_id, name, source_roll_call, created_by) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(project_id)
    .bind(&name)
    .bind(&body.roll_call)
    .bind(me.user_id)
    .execute(&mut *tx)
    .await?;

    let mut positions: Vec<(i32, Uuid)> = Vec::new();
    if let Some(source) = body.copy_from {
        let same_project: Option<Uuid> =
            sqlx::query_scalar("SELECT project_id FROM diogenes_scenario WHERE id = $1")
                .bind(source)
                .fetch_optional(&mut *tx)
                .await?;
        if same_project != Some(project_id) {
            return Err(ApiError::Invalid(
                "O cenário de origem não é deste projeto.".into(),
            ));
        }
        positions = sqlx::query_as(
            "SELECT camara_id, column_id FROM diogenes_position WHERE scenario_id = $1",
        )
        .bind(source)
        .fetch_all(&mut *tx)
        .await?;
    } else if let Some(roll_call) = &body.roll_call {
        let (votes, _) = load_votes(&state.db, roll_call).await?;
        if votes.is_empty() {
            return Err(ApiError::Invalid(
                "Esta votação não tem votos nominais.".into(),
            ));
        }
        let mut custom: HashMap<&'static str, Uuid> = HashMap::new();
        for vote in &votes {
            let column = match VoteKind::parse(&vote.vote) {
                VoteKind::Yes => key_col("yes"),
                VoteKind::No => key_col("no"),
                VoteKind::Chair => key_col("chair"),
                kind @ (VoteKind::Abstention | VoteKind::Obstruction) => {
                    let (label, color) = if kind == VoteKind::Abstention {
                        ("Abstenção", "#8d6e63")
                    } else {
                        ("Obstrução", "#455a64")
                    };
                    let existing = columns
                        .iter()
                        .find(|c| c.key.is_none() && c.name == label)
                        .map(|c| c.id);
                    match (existing, custom.get(label)) {
                        (Some(c), _) | (None, Some(&c)) => Some(c),
                        (None, None) => {
                            let c = insert_custom_column(&mut tx, project_id, label, color)
                                .await?
                                .id;
                            custom.insert(label, c);
                            Some(c)
                        }
                    }
                }
                VoteKind::Other => None,
            };
            if let Some(c) = column {
                positions.push((vote.camara_id, c));
            }
        }
        if body.absent_if_missing {
            if let Some(absent) = key_col("absent") {
                let voted: Vec<i32> = votes.iter().map(|v| v.camara_id).collect();
                let missing: Vec<i32> = sqlx::query_scalar(
                    "SELECT camara_id FROM diogenes_deputy WHERE in_office AND NOT (camara_id = ANY($1))",
                )
                .bind(&voted)
                .fetch_all(&mut *tx)
                .await?;
                positions.extend(missing.into_iter().map(|id| (id, absent)));
            }
        }
    } else if let (Some(chair), Some(column)) = (current_chair(&state.db).await?, key_col("chair"))
    {
        // A new blank scenario puts the current chair in the "chair" column.
        positions.push((chair, column));
    }

    write_positions(&mut tx, id, &positions, me.user_id).await?;
    audit(
        &mut tx,
        access.room_id,
        &me,
        "scenario.create",
        serde_json::json!({
            "scenario_id": id, "name": name, "copy_from": body.copy_from,
            "roll_call": body.roll_call, "positions": positions.len()
        }),
    )
    .await?;
    tx.commit().await?;
    publish(&state, project_id, "scenario", Some(id), &me);
    Ok(ok(id))
}

async fn write_positions(
    conn: &mut sqlx::PgConnection,
    scenario_id: Uuid,
    positions: &[(i32, Uuid)],
    by: Uuid,
) -> Result<(), sqlx::Error> {
    let ids: Vec<i32> = positions.iter().map(|p| p.0).collect();
    let columns: Vec<Uuid> = positions.iter().map(|p| p.1).collect();
    sqlx::query(
        "INSERT INTO diogenes_position (scenario_id, camara_id, column_id, updated_by)
         SELECT $1, id, col, $4 FROM UNNEST($2::int[], $3::uuid[]) AS t(id, col)
         ON CONFLICT (scenario_id, camara_id) DO UPDATE
            SET column_id = EXCLUDED.column_id, updated_by = EXCLUDED.updated_by, updated_at = now()",
    )
    .bind(scenario_id)
    .bind(&ids)
    .bind(&columns)
    .bind(by)
    .execute(conn)
    .await?;
    Ok(())
}

/// A position.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct PositionView {
    /// The deputy.
    pub camara_id: i32,
    /// The column.
    pub column_id: Uuid,
    /// The user who made the last change.
    pub updated_by: Uuid,
    /// The time of the last change.
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// A scenario with positions and scoreboard.
#[derive(Debug, Serialize)]
pub struct ScenarioDetail {
    /// The scenario id.
    pub id: Uuid,
    /// The project id.
    pub project_id: Uuid,
    /// The name.
    pub name: String,
    /// The source roll call.
    pub source_roll_call: Option<String>,
    /// The columns of the project.
    pub columns: Vec<ColumnView>,
    /// The positions of deputies in office. A missing deputy is undecided.
    pub positions: Vec<PositionView>,
    /// Count per column id (deputies in office).
    pub column_counts: HashMap<Uuid, i64>,
    /// Counts of the fixed columns.
    pub counts: Counts,
    /// The number of deputies in office.
    pub total: i64,
    /// The threshold of the project.
    pub threshold: i32,
    /// The outcome.
    pub outcome: Outcome,
    /// Orientations of the source roll call.
    pub orientations: Vec<(String, String)>,
    /// Dissidents in the source roll call.
    pub dissidents: Vec<i32>,
}

pub(super) async fn detail(
    State(state): State<DiogenesState>,
    Path(scenario_id): Path<Uuid>,
    parts: Parts,
) -> ApiResult<Json<Envelope<ScenarioDetail>>> {
    let me = actor(&state, &parts).await?;
    let (_access, project_id) = require_scenario(&state.db, &me, scenario_id, Role::Viewer).await?;
    let (name, source_roll_call, threshold): (String, Option<String>, i32) = sqlx::query_as(
        "SELECT s.name, s.source_roll_call, p.threshold FROM diogenes_scenario s
           JOIN diogenes_project p ON p.id = s.project_id WHERE s.id = $1",
    )
    .bind(scenario_id)
    .fetch_one(&state.db)
    .await?;
    let columns = load_columns(&state.db, project_id).await?;
    let positions = sqlx::query_as::<_, PositionView>(
        "SELECT p.camara_id, p.column_id, p.updated_by, p.updated_at FROM diogenes_position p
           JOIN diogenes_deputy d ON d.camara_id = p.camara_id
          WHERE p.scenario_id = $1 AND d.in_office",
    )
    .bind(scenario_id)
    .fetch_all(&state.db)
    .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM diogenes_deputy WHERE in_office")
        .fetch_one(&state.db)
        .await?;
    let (column_counts, counts) = count(&columns, &positions, total);
    let (orientations, dissidents) = match &source_roll_call {
        Some(rc) => {
            let (votes, orientations) = load_votes(&state.db, rc).await?;
            let d = dissidents(&votes, &orientations);
            (orientations, d)
        }
        None => (Vec::new(), Vec::new()),
    };
    Ok(ok(ScenarioDetail {
        id: scenario_id,
        project_id,
        name,
        source_roll_call,
        outcome: outcome(counts, i64::from(threshold), total),
        columns,
        positions,
        column_counts,
        counts,
        total,
        threshold,
        orientations,
        dissidents,
    }))
}

/// Count positions per column and per fixed key.
#[must_use]
pub fn count(
    columns: &[ColumnView],
    positions: &[PositionView],
    total: i64,
) -> (HashMap<Uuid, i64>, Counts) {
    let mut per_column: HashMap<Uuid, i64> = columns.iter().map(|c| (c.id, 0)).collect();
    for p in positions {
        *per_column.entry(p.column_id).or_insert(0) += 1;
    }
    let by_key = |key: &str| {
        columns
            .iter()
            .find(|c| c.key.as_deref() == Some(key))
            .and_then(|c| per_column.get(&c.id).copied())
            .unwrap_or(0)
    };
    let mut counts = Counts {
        yes: by_key("yes"),
        no: by_key("no"),
        disputed: by_key("disputed"),
        absent: by_key("absent"),
        chair: by_key("chair"),
        undecided: 0,
    };
    counts.undecided =
        total - counts.yes - counts.no - counts.disputed - counts.absent - counts.chair;
    if let Some(undecided) = columns
        .iter()
        .find(|c| c.key.as_deref() == Some("undecided"))
    {
        // Deputies with no row are in the "undecided" column.
        let placed: i64 = positions.len() as i64;
        let explicit = per_column.get(&undecided.id).copied().unwrap_or(0);
        per_column.insert(undecided.id, total - placed + explicit);
    }
    (per_column, counts)
}

#[derive(Debug, Deserialize)]
pub(super) struct Move {
    camara_id: i32,
    /// `None` = undecided (the row goes).
    column_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MoveBody {
    moves: Vec<Move>,
}

pub(super) async fn move_positions(
    State(state): State<DiogenesState>,
    Path(scenario_id): Path<Uuid>,
    parts: Parts,
    Json(body): Json<MoveBody>,
) -> ApiResult<Json<Envelope<usize>>> {
    let me = actor(&state, &parts).await?;
    let (access, project_id) = require_scenario(&state.db, &me, scenario_id, Role::Editor).await?;
    if body.moves.is_empty() || body.moves.len() > MOVES_MAX {
        return Err(ApiError::Invalid(format!(
            "Envie entre 1 e {MOVES_MAX} movimentos."
        )));
    }
    let columns = load_columns(&state.db, project_id).await?;
    let undecided = columns
        .iter()
        .find(|c| c.key.as_deref() == Some("undecided"))
        .map(|c| c.id);
    let mut set: Vec<(i32, Uuid)> = Vec::new();
    let mut clear: Vec<i32> = Vec::new();
    for m in &body.moves {
        match m.column_id {
            None => clear.push(m.camara_id),
            Some(c) if Some(c) == undecided => clear.push(m.camara_id),
            Some(c) if columns.iter().any(|col| col.id == c) => set.push((m.camara_id, c)),
            Some(_) => {
                return Err(ApiError::Invalid(
                    "Coluna desconhecida neste projeto.".into(),
                ))
            }
        }
    }
    let mut tx = state.db.begin().await?;
    write_positions(&mut tx, scenario_id, &set, me.user_id).await?;
    if !clear.is_empty() {
        sqlx::query("DELETE FROM diogenes_position WHERE scenario_id = $1 AND camara_id = ANY($2)")
            .bind(scenario_id)
            .bind(&clear)
            .execute(&mut *tx)
            .await?;
    }
    let moves: Vec<serde_json::Value> = body
        .moves
        .iter()
        .map(|m| serde_json::json!([m.camara_id, m.column_id]))
        .collect();
    audit(
        &mut tx,
        access.room_id,
        &me,
        "position.move",
        serde_json::json!({ "scenario_id": scenario_id, "moves": moves }),
    )
    .await?;
    tx.commit().await?;
    publish(&state, project_id, "positions", Some(scenario_id), &me);
    Ok(ok(body.moves.len()))
}

#[derive(Debug, Deserialize)]
pub(super) struct RenameBody {
    name: String,
}

pub(super) async fn rename(
    State(state): State<DiogenesState>,
    Path(scenario_id): Path<Uuid>,
    parts: Parts,
    Json(body): Json<RenameBody>,
) -> ApiResult<Json<Envelope<&'static str>>> {
    let me = actor(&state, &parts).await?;
    let (access, project_id) = require_scenario(&state.db, &me, scenario_id, Role::Editor).await?;
    let name = clean_name(&body.name, NAME_MAX)?;
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE diogenes_scenario SET name = $2 WHERE id = $1")
        .bind(scenario_id)
        .bind(&name)
        .execute(&mut *tx)
        .await?;
    audit(
        &mut tx,
        access.room_id,
        &me,
        "scenario.rename",
        serde_json::json!({ "scenario_id": scenario_id, "name": name }),
    )
    .await?;
    tx.commit().await?;
    publish(&state, project_id, "scenario", Some(scenario_id), &me);
    Ok(ok("ok"))
}

pub(super) async fn delete(
    State(state): State<DiogenesState>,
    Path(scenario_id): Path<Uuid>,
    parts: Parts,
) -> ApiResult<Json<Envelope<&'static str>>> {
    let me = actor(&state, &parts).await?;
    let (access, project_id) = require_scenario(&state.db, &me, scenario_id, Role::Editor).await?;
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM diogenes_scenario WHERE id = $1")
        .bind(scenario_id)
        .execute(&mut *tx)
        .await?;
    audit(
        &mut tx,
        access.room_id,
        &me,
        "scenario.delete",
        serde_json::json!({ "scenario_id": scenario_id }),
    )
    .await?;
    tx.commit().await?;
    publish(&state, project_id, "scenario", Some(scenario_id), &me);
    Ok(ok("ok"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(key: Option<&str>) -> ColumnView {
        ColumnView {
            id: Uuid::now_v7(),
            key: key.map(str::to_owned),
            name: String::new(),
            color: String::new(),
            position: 0,
        }
    }

    fn pos(id: i32, column: Uuid) -> PositionView {
        PositionView {
            camara_id: id,
            column_id: column,
            updated_by: Uuid::nil(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn count_puts_unplaced_and_custom_in_undecided() {
        let cols = vec![
            col(Some("undecided")),
            col(Some("yes")),
            col(Some("no")),
            col(None),
        ];
        let positions = vec![
            pos(1, cols[1].id),
            pos(2, cols[1].id),
            pos(3, cols[2].id),
            pos(4, cols[3].id),
        ];
        let (per_column, counts) = count(&cols, &positions, 10);
        assert_eq!(counts.yes, 2);
        assert_eq!(counts.no, 1);
        // 10 total - 2 yes - 1 no = 7 (6 unplaced + 1 in a custom column).
        assert_eq!(counts.undecided, 7);
        assert_eq!(per_column.get(&cols[0].id), Some(&6));
        assert_eq!(per_column.get(&cols[3].id), Some(&1));
    }
}
