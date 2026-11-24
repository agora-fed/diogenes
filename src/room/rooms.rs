//! Rooms and members.

use axum::extract::{Path, Query, State};
use axum::http::request::Parts;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::access::{actor, audit, clean_name, require_room, Role};
use crate::error::{ok, ApiError, ApiResult, Envelope};
use crate::DiogenesState;

const NAME_MAX: usize = 120;
const AUDIT_PAGE_MAX: i64 = 200;

/// A room, as its members see it.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct RoomView {
    /// The room id.
    pub id: Uuid,
    /// The name.
    pub name: String,
    /// The role of the caller.
    pub role: String,
    /// The creation time.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// A member of a room.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct MemberView {
    /// The host user id.
    pub user_id: Uuid,
    /// The public name.
    pub display_name: String,
    /// The role.
    pub role: String,
}

/// A project summary in a room.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ProjectSummary {
    /// The project id.
    pub id: Uuid,
    /// The name.
    pub name: String,
    /// The threshold.
    pub threshold: i32,
    /// The bill id.
    pub bill_id: Option<i32>,
}

/// A room with its members and projects.
#[derive(Debug, Serialize)]
pub struct RoomDetail {
    /// The room.
    #[serde(flatten)]
    pub room: RoomView,
    /// The members.
    pub members: Vec<MemberView>,
    /// The projects.
    pub projects: Vec<ProjectSummary>,
    /// True when the caller can make new rooms.
    pub can_create_rooms: bool,
}

/// The rooms of the caller and the permission to make new ones.
#[derive(Debug, Serialize)]
pub struct MyRooms {
    /// The rooms.
    pub rooms: Vec<RoomView>,
    /// True when the caller can make new rooms.
    pub can_create_rooms: bool,
    /// The public name of the caller.
    pub display_name: String,
}

pub(super) async fn list(
    State(state): State<DiogenesState>,
    parts: Parts,
) -> ApiResult<Json<Envelope<MyRooms>>> {
    let me = actor(&state, &parts).await?;
    let rooms = sqlx::query_as::<_, RoomView>(
        "SELECT r.id, r.name, m.role, r.created_at FROM diogenes_room r
           JOIN diogenes_room_member m ON m.room_id = r.id
          WHERE m.user_id = $1 AND r.org_id = $2
          ORDER BY r.created_at DESC",
    )
    .bind(me.user_id)
    .bind(me.org_id)
    .fetch_all(&state.db)
    .await?;
    Ok(ok(MyRooms {
        rooms,
        can_create_rooms: me.can_create_rooms,
        display_name: me.display_name,
    }))
}

#[derive(Debug, Deserialize)]
pub(super) struct NameBody {
    name: String,
}

pub(super) async fn create(
    State(state): State<DiogenesState>,
    parts: Parts,
    Json(body): Json<NameBody>,
) -> ApiResult<Json<Envelope<RoomView>>> {
    let me = actor(&state, &parts).await?;
    if !me.can_create_rooms {
        return Err(ApiError::Forbidden);
    }
    let name = clean_name(&body.name, NAME_MAX)?;
    let id = Uuid::now_v7();
    let mut tx = state.db.begin().await?;
    let created_at: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "INSERT INTO diogenes_room (id, org_id, name, created_by) VALUES ($1, $2, $3, $4) RETURNING created_at",
    )
    .bind(id)
    .bind(me.org_id)
    .bind(&name)
    .bind(me.user_id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO diogenes_room_member (room_id, user_id, display_name, role) VALUES ($1, $2, $3, 'owner')",
    )
    .bind(id)
    .bind(me.user_id)
    .bind(&me.display_name)
    .execute(&mut *tx)
    .await?;
    audit(
        &mut tx,
        id,
        &me,
        "room.create",
        serde_json::json!({ "name": name }),
    )
    .await?;
    tx.commit().await?;
    Ok(ok(RoomView {
        id,
        name,
        role: Role::Owner.as_str().to_owned(),
        created_at,
    }))
}

pub(super) async fn detail(
    State(state): State<DiogenesState>,
    Path(room_id): Path<Uuid>,
    parts: Parts,
) -> ApiResult<Json<Envelope<RoomDetail>>> {
    let me = actor(&state, &parts).await?;
    let access = require_room(&state.db, &me, room_id, Role::Viewer).await?;
    let room = sqlx::query_as::<_, RoomView>(
        "SELECT id, name, $2::text AS role, created_at FROM diogenes_room WHERE id = $1",
    )
    .bind(room_id)
    .bind(access.role.as_str())
    .fetch_one(&state.db)
    .await?;
    let members = sqlx::query_as::<_, MemberView>(
        "SELECT user_id, display_name, role FROM diogenes_room_member WHERE room_id = $1
          ORDER BY CASE role WHEN 'owner' THEN 0 WHEN 'editor' THEN 1 ELSE 2 END, display_name",
    )
    .bind(room_id)
    .fetch_all(&state.db)
    .await?;
    let projects = sqlx::query_as::<_, ProjectSummary>(
        "SELECT id, name, threshold, bill_id FROM diogenes_project WHERE room_id = $1 ORDER BY created_at",
    )
    .bind(room_id)
    .fetch_all(&state.db)
    .await?;
    Ok(ok(RoomDetail {
        room,
        members,
        projects,
        can_create_rooms: me.can_create_rooms,
    }))
}

pub(super) async fn rename(
    State(state): State<DiogenesState>,
    Path(room_id): Path<Uuid>,
    parts: Parts,
    Json(body): Json<NameBody>,
) -> ApiResult<Json<Envelope<&'static str>>> {
    let me = actor(&state, &parts).await?;
    require_room(&state.db, &me, room_id, Role::Owner).await?;
    let name = clean_name(&body.name, NAME_MAX)?;
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE diogenes_room SET name = $2 WHERE id = $1")
        .bind(room_id)
        .bind(&name)
        .execute(&mut *tx)
        .await?;
    audit(
        &mut tx,
        room_id,
        &me,
        "room.rename",
        serde_json::json!({ "name": name }),
    )
    .await?;
    tx.commit().await?;
    Ok(ok("ok"))
}

pub(super) async fn delete(
    State(state): State<DiogenesState>,
    Path(room_id): Path<Uuid>,
    parts: Parts,
) -> ApiResult<Json<Envelope<&'static str>>> {
    let me = actor(&state, &parts).await?;
    require_room(&state.db, &me, room_id, Role::Owner).await?;
    sqlx::query("DELETE FROM diogenes_room WHERE id = $1")
        .bind(room_id)
        .execute(&state.db)
        .await?;
    tracing::info!(%room_id, actor = %me.user_id, "diogenes room deleted");
    Ok(ok("ok"))
}

#[derive(Debug, Deserialize)]
pub(super) struct AddMember {
    handle: String,
    role: Role,
}

pub(super) async fn add_member(
    State(state): State<DiogenesState>,
    Path(room_id): Path<Uuid>,
    parts: Parts,
    Json(body): Json<AddMember>,
) -> ApiResult<Json<Envelope<MemberView>>> {
    let me = actor(&state, &parts).await?;
    require_room(&state.db, &me, room_id, Role::Owner).await?;
    let handle = body.handle.trim().trim_start_matches('@');
    if handle.is_empty() || handle.len() > 100 {
        return Err(ApiError::Invalid(
            "Informe o @ de quem você quer convidar.".into(),
        ));
    }
    let user = state
        .identity
        .find_user(me.org_id, handle)
        .await
        .ok_or_else(|| ApiError::Invalid("Nenhuma pessoa com esse @ nesta organização.".into()))?;
    let mut tx = state.db.begin().await?;
    // Lock the room row: two owners who leave at the same time must not both succeed.
    sqlx::query("SELECT id FROM diogenes_room WHERE id = $1 FOR UPDATE")
        .bind(room_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO diogenes_room_member (room_id, user_id, display_name, role) VALUES ($1, $2, $3, $4)
         ON CONFLICT (room_id, user_id) DO UPDATE SET role = EXCLUDED.role, display_name = EXCLUDED.display_name",
    )
    .bind(room_id)
    .bind(user.user_id)
    .bind(&user.display_name)
    .bind(body.role.as_str())
    .execute(&mut *tx)
    .await?;
    ensure_owner_left(&mut tx, room_id).await?;
    audit(
        &mut tx,
        room_id,
        &me,
        "member.set",
        serde_json::json!({ "user_id": user.user_id, "role": body.role.as_str() }),
    )
    .await?;
    tx.commit().await?;
    Ok(ok(MemberView {
        user_id: user.user_id,
        display_name: user.display_name,
        role: body.role.as_str().to_owned(),
    }))
}

pub(super) async fn remove_member(
    State(state): State<DiogenesState>,
    Path((room_id, user_id)): Path<(Uuid, Uuid)>,
    parts: Parts,
) -> ApiResult<Json<Envelope<&'static str>>> {
    let me = actor(&state, &parts).await?;
    // A member can leave a room. Only an owner can remove another member.
    let min = if user_id == me.user_id {
        Role::Viewer
    } else {
        Role::Owner
    };
    require_room(&state.db, &me, room_id, min).await?;
    let mut tx = state.db.begin().await?;
    // Lock the room row: two owners who leave at the same time must not both succeed.
    sqlx::query("SELECT id FROM diogenes_room WHERE id = $1 FOR UPDATE")
        .bind(room_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM diogenes_room_member WHERE room_id = $1 AND user_id = $2")
        .bind(room_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    ensure_owner_left(&mut tx, room_id).await?;
    audit(
        &mut tx,
        room_id,
        &me,
        "member.remove",
        serde_json::json!({ "user_id": user_id }),
    )
    .await?;
    tx.commit().await?;
    Ok(ok("ok"))
}

/// A room must keep at least one owner.
async fn ensure_owner_left(conn: &mut sqlx::PgConnection, room_id: Uuid) -> ApiResult<()> {
    let owners: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM diogenes_room_member WHERE room_id = $1 AND role = 'owner'",
    )
    .bind(room_id)
    .fetch_one(conn)
    .await?;
    if owners == 0 {
        return Err(ApiError::Invalid(
            "A sala precisa de pelo menos um responsável.".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub(super) struct AuditQuery {
    before: Option<i64>,
    limit: Option<i64>,
}

/// One audit row.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct AuditView {
    /// The row id.
    pub id: i64,
    /// The user id.
    pub actor_id: Uuid,
    /// The user name, when the user is still a member.
    pub actor_name: Option<String>,
    /// The action.
    pub action: String,
    /// The action data.
    pub payload: serde_json::Value,
    /// The time.
    pub at: chrono::DateTime<chrono::Utc>,
}

pub(super) async fn audit_log(
    State(state): State<DiogenesState>,
    Path(room_id): Path<Uuid>,
    Query(q): Query<AuditQuery>,
    parts: Parts,
) -> ApiResult<Json<Envelope<Vec<AuditView>>>> {
    let me = actor(&state, &parts).await?;
    require_room(&state.db, &me, room_id, Role::Viewer).await?;
    let rows = sqlx::query_as::<_, AuditView>(
        "SELECT a.id, a.actor_id, m.display_name AS actor_name, a.action, a.payload, a.at
           FROM diogenes_audit a
           LEFT JOIN diogenes_room_member m ON m.room_id = a.room_id AND m.user_id = a.actor_id
          WHERE a.room_id = $1 AND ($2::bigint IS NULL OR a.id < $2)
          ORDER BY a.id DESC LIMIT $3",
    )
    .bind(room_id)
    .bind(q.before)
    .bind(q.limit.unwrap_or(50).clamp(1, AUDIT_PAGE_MAX))
    .fetch_all(&state.db)
    .await?;
    Ok(ok(rows))
}
