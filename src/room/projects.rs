//! Projects, columns, priorities, notes and the live-update stream.

use axum::extract::{Path, State};
use axum::http::request::Parts;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::access::{actor, audit, clean_name, require_project, require_room, Role};
use super::events::Change;
use crate::error::{ok, ApiError, ApiResult, Envelope};
use crate::scoreboard::Rule;
use crate::text::DeputyIndex;
use crate::DiogenesState;

const NAME_MAX: usize = 120;
const COLUMN_NAME_MAX: usize = 60;
const NOTE_MAX: usize = 10_000;
const LIST_TEXT_MAX: usize = 100_000;
const BULK_MAX: usize = 600;

/// The fixed columns of every project: (key, name, colour).
pub const FIXED_COLUMNS: [(&str, &str, &str); 6] = [
    ("undecided", "Sem posição", "#9aa0a6"),
    ("yes", "Sim", "#2e7d32"),
    ("no", "Não", "#c62828"),
    ("disputed", "Em disputa", "#f9a825"),
    ("absent", "Ausente", "#757575"),
    ("chair", "Presidente", "#5e35b1"),
];

/// A column.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ColumnView {
    /// The column id.
    pub id: Uuid,
    /// The key of a fixed column; `None` for a custom column.
    pub key: Option<String>,
    /// The name.
    pub name: String,
    /// The colour (#rrggbb).
    pub color: String,
    /// The display order.
    pub position: i32,
}

/// A scenario summary.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ScenarioSummary {
    /// The scenario id.
    pub id: Uuid,
    /// The name.
    pub name: String,
    /// The roll call that the scenario came from.
    pub source_roll_call: Option<String>,
    /// Deputies in office in the "yes" column.
    pub yes: i64,
}

/// A note.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct NoteView {
    /// The note id.
    pub id: Uuid,
    /// The deputy.
    pub camara_id: i32,
    /// The author.
    pub author_id: Uuid,
    /// The author name, when the author is still a member.
    pub author_name: Option<String>,
    /// The text.
    pub body: String,
    /// `room` or `author`.
    pub visibility: String,
    /// The last change.
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// A project with all its parts that the caller can see.
#[derive(Debug, Serialize)]
pub struct ProjectDetail {
    /// The project id.
    pub id: Uuid,
    /// The room id.
    pub room_id: Uuid,
    /// The name.
    pub name: String,
    /// The threshold. For the rule `simple` it is the quorum.
    pub threshold: i32,
    /// The approval rule.
    pub rule: Rule,
    /// The bill id.
    pub bill_id: Option<i32>,
    /// The role of the caller.
    pub role: Role,
    /// The columns.
    pub columns: Vec<ColumnView>,
    /// The scenarios.
    pub scenarios: Vec<ScenarioSummary>,
    /// The Câmara ids of the priority deputies.
    pub priorities: Vec<i32>,
    /// The notes that the caller can read.
    pub notes: Vec<NoteView>,
}

#[derive(Debug, Deserialize)]
pub(super) struct NewProject {
    name: String,
    threshold: Option<i32>,
    rule: Option<Rule>,
    bill_id: Option<i32>,
}

/// The rule and threshold of a new project. A value that the caller sends wins. Without
/// values, the kind of a stored bill picks them (PEC 308, PLP 257, other kinds simple
/// majority). Without a known bill: fixed, 257.
async fn rule_and_threshold(
    db: &sqlx::PgPool,
    body: &NewProject,
) -> Result<(Rule, i32), sqlx::Error> {
    let kind: Option<String> = match body.bill_id {
        Some(bill) => {
            sqlx::query_scalar("SELECT kind FROM diogenes_bill WHERE camara_id = $1")
                .bind(bill)
                .fetch_optional(db)
                .await?
        }
        None => None,
    };
    let by_kind = kind.as_deref().map(Rule::for_bill_kind);
    let rule = body
        .rule
        .or(body.threshold.map(|_| Rule::Fixed))
        .or(by_kind.map(|(r, _)| r))
        .unwrap_or_default();
    let threshold = body.threshold.unwrap_or_else(|| match (rule, by_kind) {
        (Rule::Fixed, Some((Rule::Fixed, t))) => i32::try_from(t).unwrap_or(257),
        _ => 257,
    });
    Ok((rule, threshold))
}

fn check_threshold(t: i32) -> ApiResult<i32> {
    if (1..=513).contains(&t) {
        Ok(t)
    } else {
        Err(ApiError::Invalid(
            "O mínimo deve estar entre 1 e 513.".into(),
        ))
    }
}

pub(super) async fn create(
    State(state): State<DiogenesState>,
    Path(room_id): Path<Uuid>,
    parts: Parts,
    Json(body): Json<NewProject>,
) -> ApiResult<Json<Envelope<Uuid>>> {
    let me = actor(&state, &parts).await?;
    require_room(&state.db, &me, room_id, Role::Editor).await?;
    let name = clean_name(&body.name, NAME_MAX)?;
    let (rule, threshold) = rule_and_threshold(&state.db, &body).await?;
    let threshold = check_threshold(threshold)?;
    let id = Uuid::now_v7();
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "INSERT INTO diogenes_project (id, room_id, name, threshold, rule, bill_id, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(room_id)
    .bind(&name)
    .bind(threshold)
    .bind(rule)
    .bind(body.bill_id)
    .bind(me.user_id)
    .execute(&mut *tx)
    .await?;
    for (position, (key, col_name, color)) in FIXED_COLUMNS.iter().enumerate() {
        sqlx::query(
            "INSERT INTO diogenes_column (id, project_id, key, name, color, position) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(Uuid::now_v7())
        .bind(id)
        .bind(key)
        .bind(col_name)
        .bind(color)
        .bind(i32::try_from(position).unwrap_or(0))
        .execute(&mut *tx)
        .await?;
    }
    audit(
        &mut tx,
        room_id,
        &me,
        "project.create",
        serde_json::json!({ "project_id": id, "name": name, "rule": rule, "threshold": threshold }),
    )
    .await?;
    tx.commit().await?;
    Ok(ok(id))
}

pub(super) async fn detail(
    State(state): State<DiogenesState>,
    Path(project_id): Path<Uuid>,
    parts: Parts,
) -> ApiResult<Json<Envelope<ProjectDetail>>> {
    let me = actor(&state, &parts).await?;
    let access = require_project(&state.db, &me, project_id, Role::Viewer).await?;
    let (name, threshold, rule, bill_id): (String, i32, Rule, Option<i32>) =
        sqlx::query_as("SELECT name, threshold, rule, bill_id FROM diogenes_project WHERE id = $1")
            .bind(project_id)
            .fetch_one(&state.db)
            .await?;
    let columns = load_columns(&state.db, project_id).await?;
    let scenarios = sqlx::query_as::<_, ScenarioSummary>(
        "SELECT s.id, s.name, s.source_roll_call,
                (SELECT count(*) FROM diogenes_position p
                   JOIN diogenes_column c ON c.id = p.column_id
                   JOIN diogenes_deputy d ON d.camara_id = p.camara_id
                  WHERE p.scenario_id = s.id AND c.key = 'yes' AND d.in_office) AS yes
           FROM diogenes_scenario s WHERE s.project_id = $1 ORDER BY s.created_at",
    )
    .bind(project_id)
    .fetch_all(&state.db)
    .await?;
    let priorities: Vec<i32> = sqlx::query_scalar(
        "SELECT camara_id FROM diogenes_priority WHERE project_id = $1 ORDER BY camara_id",
    )
    .bind(project_id)
    .fetch_all(&state.db)
    .await?;
    let notes = sqlx::query_as::<_, NoteView>(
        "SELECT n.id, n.camara_id, n.author_id, m.display_name AS author_name, n.body, n.visibility, n.updated_at
           FROM diogenes_note n
           LEFT JOIN diogenes_room_member m ON m.room_id = $2 AND m.user_id = n.author_id
          WHERE n.project_id = $1 AND (n.visibility = 'room' OR n.author_id = $3)
          ORDER BY n.updated_at DESC",
    )
    .bind(project_id)
    .bind(access.room_id)
    .bind(me.user_id)
    .fetch_all(&state.db)
    .await?;
    Ok(ok(ProjectDetail {
        id: project_id,
        room_id: access.room_id,
        name,
        threshold,
        rule,
        bill_id,
        role: access.role,
        columns,
        scenarios,
        priorities,
        notes,
    }))
}

/// Load the columns of a project, in display order.
///
/// # Errors
/// Returns a database error.
pub async fn load_columns(
    db: &sqlx::PgPool,
    project_id: Uuid,
) -> Result<Vec<ColumnView>, sqlx::Error> {
    sqlx::query_as::<_, ColumnView>(
        "SELECT id, key, name, color, position FROM diogenes_column WHERE project_id = $1 ORDER BY position, name",
    )
    .bind(project_id)
    .fetch_all(db)
    .await
}

#[derive(Debug, Deserialize)]
pub(super) struct PatchProject {
    name: Option<String>,
    threshold: Option<i32>,
    rule: Option<Rule>,
    /// `Some(None)` clears the bill.
    #[serde(default, with = "double_option")]
    bill_id: Option<Option<i32>>,
}

mod double_option {
    use serde::{Deserialize, Deserializer};

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<Option<i32>>, D::Error> {
        Option::<i32>::deserialize(d).map(Some)
    }
}

pub(super) async fn patch(
    State(state): State<DiogenesState>,
    Path(project_id): Path<Uuid>,
    parts: Parts,
    Json(body): Json<PatchProject>,
) -> ApiResult<Json<Envelope<&'static str>>> {
    let me = actor(&state, &parts).await?;
    let access = require_project(&state.db, &me, project_id, Role::Editor).await?;
    let name = body
        .name
        .as_deref()
        .map(|n| clean_name(n, NAME_MAX))
        .transpose()?;
    let threshold = body.threshold.map(check_threshold).transpose()?;
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "UPDATE diogenes_project
            SET name = COALESCE($2, name), threshold = COALESCE($3, threshold),
                bill_id = CASE WHEN $4 THEN $5 ELSE bill_id END,
                rule = COALESCE($6, rule)
          WHERE id = $1",
    )
    .bind(project_id)
    .bind(&name)
    .bind(threshold)
    .bind(body.bill_id.is_some())
    .bind(body.bill_id.flatten())
    .bind(body.rule)
    .execute(&mut *tx)
    .await?;
    audit(
        &mut tx,
        access.room_id,
        &me,
        "project.update",
        serde_json::json!({ "project_id": project_id, "name": name, "threshold": threshold, "rule": body.rule, "bill_id": body.bill_id }),
    )
    .await?;
    tx.commit().await?;
    publish(&state, project_id, "project", None, &me);
    Ok(ok("ok"))
}

pub(super) async fn delete(
    State(state): State<DiogenesState>,
    Path(project_id): Path<Uuid>,
    parts: Parts,
) -> ApiResult<Json<Envelope<&'static str>>> {
    let me = actor(&state, &parts).await?;
    let access = require_project(&state.db, &me, project_id, Role::Owner).await?;
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM diogenes_project WHERE id = $1")
        .bind(project_id)
        .execute(&mut *tx)
        .await?;
    audit(
        &mut tx,
        access.room_id,
        &me,
        "project.delete",
        serde_json::json!({ "project_id": project_id }),
    )
    .await?;
    tx.commit().await?;
    publish(&state, project_id, "project", None, &me);
    Ok(ok("ok"))
}

#[derive(Debug, Deserialize)]
pub(super) struct ColumnBody {
    name: String,
    color: Option<String>,
}

fn clean_color(raw: Option<&str>) -> ApiResult<Option<String>> {
    match raw {
        None => Ok(None),
        Some(c)
            if c.len() == 7
                && c.starts_with('#')
                && c[1..].chars().all(|x| x.is_ascii_hexdigit()) =>
        {
            Ok(Some(c.to_lowercase()))
        }
        Some(_) => Err(ApiError::Invalid("Cor inválida: use #rrggbb.".into())),
    }
}

pub(super) async fn add_column(
    State(state): State<DiogenesState>,
    Path(project_id): Path<Uuid>,
    parts: Parts,
    Json(body): Json<ColumnBody>,
) -> ApiResult<Json<Envelope<ColumnView>>> {
    let me = actor(&state, &parts).await?;
    let access = require_project(&state.db, &me, project_id, Role::Editor).await?;
    let name = clean_name(&body.name, COLUMN_NAME_MAX)?;
    let color = clean_color(body.color.as_deref())?.unwrap_or_else(|| "#607d8b".into());
    let mut tx = state.db.begin().await?;
    let column = insert_custom_column(&mut tx, project_id, &name, &color).await?;
    audit(
        &mut tx,
        access.room_id,
        &me,
        "column.create",
        serde_json::json!({ "project_id": project_id, "name": name }),
    )
    .await?;
    tx.commit().await?;
    publish(&state, project_id, "columns", None, &me);
    Ok(ok(column))
}

/// Insert a custom column at the end of a project.
///
/// # Errors
/// Returns a database error.
pub async fn insert_custom_column(
    conn: &mut sqlx::PgConnection,
    project_id: Uuid,
    name: &str,
    color: &str,
) -> Result<ColumnView, sqlx::Error> {
    sqlx::query_as::<_, ColumnView>(
        "INSERT INTO diogenes_column (id, project_id, key, name, color, position)
         VALUES ($1, $2, NULL, $3, $4,
                 (SELECT COALESCE(max(position), 0) + 1 FROM diogenes_column WHERE project_id = $2))
         RETURNING id, key, name, color, position",
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(name)
    .bind(color)
    .fetch_one(conn)
    .await
}

async fn column_project(db: &sqlx::PgPool, column_id: Uuid) -> ApiResult<(Uuid, Option<String>)> {
    sqlx::query_as("SELECT project_id, key FROM diogenes_column WHERE id = $1")
        .bind(column_id)
        .fetch_optional(db)
        .await?
        .ok_or(ApiError::NotFound)
}

pub(super) async fn rename_column(
    State(state): State<DiogenesState>,
    Path(column_id): Path<Uuid>,
    parts: Parts,
    Json(body): Json<ColumnBody>,
) -> ApiResult<Json<Envelope<&'static str>>> {
    let me = actor(&state, &parts).await?;
    let (project_id, _key) = column_project(&state.db, column_id).await?;
    let access = require_project(&state.db, &me, project_id, Role::Editor).await?;
    let name = clean_name(&body.name, COLUMN_NAME_MAX)?;
    let color = clean_color(body.color.as_deref())?;
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE diogenes_column SET name = $2, color = COALESCE($3, color) WHERE id = $1")
        .bind(column_id)
        .bind(&name)
        .bind(&color)
        .execute(&mut *tx)
        .await?;
    audit(
        &mut tx,
        access.room_id,
        &me,
        "column.update",
        serde_json::json!({ "column_id": column_id, "name": name }),
    )
    .await?;
    tx.commit().await?;
    publish(&state, project_id, "columns", None, &me);
    Ok(ok("ok"))
}

pub(super) async fn delete_column(
    State(state): State<DiogenesState>,
    Path(column_id): Path<Uuid>,
    parts: Parts,
) -> ApiResult<Json<Envelope<&'static str>>> {
    let me = actor(&state, &parts).await?;
    let (project_id, key) = column_project(&state.db, column_id).await?;
    let access = require_project(&state.db, &me, project_id, Role::Editor).await?;
    if key.is_some() {
        return Err(ApiError::Invalid(
            "Colunas fixas não podem ser excluídas.".into(),
        ));
    }
    let mut tx = state.db.begin().await?;
    // Positions in the column go with it (ON DELETE CASCADE): those deputies become undecided.
    sqlx::query("DELETE FROM diogenes_column WHERE id = $1")
        .bind(column_id)
        .execute(&mut *tx)
        .await?;
    audit(
        &mut tx,
        access.room_id,
        &me,
        "column.delete",
        serde_json::json!({ "column_id": column_id }),
    )
    .await?;
    tx.commit().await?;
    publish(&state, project_id, "columns", None, &me);
    Ok(ok("ok"))
}

#[derive(Debug, Deserialize)]
pub(super) struct PriorityBody {
    camara_ids: Vec<i32>,
    value: bool,
}

pub(super) async fn set_priorities(
    State(state): State<DiogenesState>,
    Path(project_id): Path<Uuid>,
    parts: Parts,
    Json(body): Json<PriorityBody>,
) -> ApiResult<Json<Envelope<usize>>> {
    let me = actor(&state, &parts).await?;
    let access = require_project(&state.db, &me, project_id, Role::Editor).await?;
    if body.camara_ids.is_empty() || body.camara_ids.len() > BULK_MAX {
        return Err(ApiError::Invalid(format!(
            "Envie entre 1 e {BULK_MAX} deputados."
        )));
    }
    let mut tx = state.db.begin().await?;
    if body.value {
        sqlx::query(
            "INSERT INTO diogenes_priority (project_id, camara_id, marked_by)
             SELECT $1, id, $3 FROM UNNEST($2::int[]) AS t(id) ON CONFLICT DO NOTHING",
        )
        .bind(project_id)
        .bind(&body.camara_ids)
        .bind(me.user_id)
        .execute(&mut *tx)
        .await?;
    } else {
        sqlx::query("DELETE FROM diogenes_priority WHERE project_id = $1 AND camara_id = ANY($2)")
            .bind(project_id)
            .bind(&body.camara_ids)
            .execute(&mut *tx)
            .await?;
    }
    audit(
        &mut tx,
        access.room_id,
        &me,
        "priority.set",
        serde_json::json!({ "project_id": project_id, "camara_ids": body.camara_ids, "value": body.value }),
    )
    .await?;
    tx.commit().await?;
    publish(&state, project_id, "priorities", None, &me);
    Ok(ok(body.camara_ids.len()))
}

#[derive(Debug, Deserialize)]
pub(super) struct NewNote {
    camara_id: i32,
    body: String,
    #[serde(default)]
    private: bool,
}

fn clean_note(raw: &str) -> ApiResult<String> {
    let body = raw.trim();
    if body.is_empty() || body.chars().count() > NOTE_MAX {
        return Err(ApiError::Invalid(format!(
            "A anotação deve ter entre 1 e {NOTE_MAX} caracteres."
        )));
    }
    Ok(body.to_owned())
}

pub(super) async fn add_note(
    State(state): State<DiogenesState>,
    Path(project_id): Path<Uuid>,
    parts: Parts,
    Json(body): Json<NewNote>,
) -> ApiResult<Json<Envelope<Uuid>>> {
    let me = actor(&state, &parts).await?;
    let access = require_project(&state.db, &me, project_id, Role::Editor).await?;
    let text = clean_note(&body.body)?;
    let id = Uuid::now_v7();
    let visibility = if body.private { "author" } else { "room" };
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "INSERT INTO diogenes_note (id, project_id, camara_id, author_id, body, visibility)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(project_id)
    .bind(body.camara_id)
    .bind(me.user_id)
    .bind(&text)
    .bind(visibility)
    .execute(&mut *tx)
    .await?;
    // The audit row has no note text: private notes must stay private.
    audit(
        &mut tx,
        access.room_id,
        &me,
        "note.create",
        // A private note leaves no trace of its deputy for the other members.
        if body.private {
            serde_json::json!({ "note_id": id, "visibility": visibility })
        } else {
            serde_json::json!({ "note_id": id, "camara_id": body.camara_id, "visibility": visibility })
        },
    )
    .await?;
    tx.commit().await?;
    if !body.private {
        publish(&state, project_id, "notes", None, &me);
    }
    Ok(ok(id))
}

#[derive(Debug, Deserialize)]
pub(super) struct EditNote {
    body: String,
}

/// Return (project, author, visibility) of a note.
async fn note_owner(db: &sqlx::PgPool, note_id: Uuid) -> ApiResult<(Uuid, Uuid, String)> {
    sqlx::query_as("SELECT project_id, author_id, visibility FROM diogenes_note WHERE id = $1")
        .bind(note_id)
        .fetch_optional(db)
        .await?
        .ok_or(ApiError::NotFound)
}

pub(super) async fn edit_note(
    State(state): State<DiogenesState>,
    Path(note_id): Path<Uuid>,
    parts: Parts,
    Json(body): Json<EditNote>,
) -> ApiResult<Json<Envelope<&'static str>>> {
    let me = actor(&state, &parts).await?;
    let (project_id, author, visibility) = note_owner(&state.db, note_id).await?;
    let access = require_project(&state.db, &me, project_id, Role::Editor).await?;
    if author != me.user_id {
        return Err(ApiError::Forbidden);
    }
    let text = clean_note(&body.body)?;
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE diogenes_note SET body = $2, updated_at = now() WHERE id = $1")
        .bind(note_id)
        .bind(&text)
        .execute(&mut *tx)
        .await?;
    audit(
        &mut tx,
        access.room_id,
        &me,
        "note.update",
        serde_json::json!({ "note_id": note_id }),
    )
    .await?;
    tx.commit().await?;
    if visibility == "room" {
        publish(&state, project_id, "notes", None, &me);
    }
    Ok(ok("ok"))
}

pub(super) async fn delete_note(
    State(state): State<DiogenesState>,
    Path(note_id): Path<Uuid>,
    parts: Parts,
) -> ApiResult<Json<Envelope<&'static str>>> {
    let me = actor(&state, &parts).await?;
    let (project_id, author, visibility) = note_owner(&state.db, note_id).await?;
    let access = require_project(&state.db, &me, project_id, Role::Viewer).await?;
    // The author can delete a note. An owner can delete any note (moderation).
    if author != me.user_id && access.role != Role::Owner {
        return Err(ApiError::Forbidden);
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM diogenes_note WHERE id = $1")
        .bind(note_id)
        .execute(&mut *tx)
        .await?;
    audit(
        &mut tx,
        access.room_id,
        &me,
        "note.delete",
        serde_json::json!({ "note_id": note_id }),
    )
    .await?;
    tx.commit().await?;
    if visibility == "room" {
        publish(&state, project_id, "notes", None, &me);
    }
    Ok(ok("ok"))
}

#[derive(Debug, Deserialize)]
pub(super) struct ListBody {
    text: String,
}

/// The result of a pasted-list match.
#[derive(Debug, Serialize)]
pub struct ListMatchView {
    /// The matched deputies: (Câmara id, name).
    pub found: Vec<(i32, String)>,
    /// The lines with no match.
    pub missed: Vec<String>,
}

pub(super) async fn match_list(
    State(state): State<DiogenesState>,
    Path(project_id): Path<Uuid>,
    parts: Parts,
    Json(body): Json<ListBody>,
) -> ApiResult<Json<Envelope<ListMatchView>>> {
    let me = actor(&state, &parts).await?;
    require_project(&state.db, &me, project_id, Role::Viewer).await?;
    if body.text.len() > LIST_TEXT_MAX {
        return Err(ApiError::Invalid("Texto longo demais.".into()));
    }
    let deputies = crate::reference::load_deputies(&state.db).await?;
    let index = DeputyIndex::new(
        deputies
            .iter()
            .map(|d| (d.camara_id, d.name.as_str(), d.uf.as_str())),
    );
    let result = index.match_list(&body.text);
    let name_of = |id: i32| {
        deputies
            .iter()
            .find(|d| d.camara_id == id)
            .map(|d| d.name.clone())
            .unwrap_or_default()
    };
    Ok(ok(ListMatchView {
        found: result
            .found
            .into_iter()
            .map(|id| (id, name_of(id)))
            .collect(),
        missed: result.missed,
    }))
}

pub(super) async fn events(
    State(state): State<DiogenesState>,
    Path(project_id): Path<Uuid>,
    parts: Parts,
) -> ApiResult<impl IntoResponse> {
    let me = actor(&state, &parts).await?;
    require_project(&state.db, &me, project_id, Role::Viewer).await?;
    state
        .hub
        .subscribe(project_id, me.user_id)
        .ok_or(ApiError::TooManyStreams)
}

pub(super) fn publish(
    state: &DiogenesState,
    project_id: Uuid,
    what: &'static str,
    scenario_id: Option<Uuid>,
    me: &crate::Actor,
) {
    state.hub.publish(
        project_id,
        Change {
            what,
            scenario_id,
            by: me.user_id,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_validation() {
        assert_eq!(
            clean_color(Some("#A1B2C3")).ok().flatten().as_deref(),
            Some("#a1b2c3")
        );
        assert!(clean_color(Some("red")).is_err());
        assert!(clean_color(Some("#12345g")).is_err());
        assert_eq!(clean_color(None).ok().flatten(), None);
    }

    #[test]
    fn threshold_limits() {
        assert!(check_threshold(0).is_err());
        assert!(check_threshold(514).is_err());
        assert_eq!(check_threshold(308).ok(), Some(308));
    }

    #[test]
    fn note_limits() {
        assert!(clean_note("  ").is_err());
        assert_eq!(clean_note(" ok ").ok().as_deref(), Some("ok"));
    }

    #[test]
    fn fixed_columns_have_unique_keys() {
        let keys: std::collections::HashSet<_> = FIXED_COLUMNS.iter().map(|c| c.0).collect();
        assert_eq!(keys.len(), FIXED_COLUMNS.len());
    }
}
