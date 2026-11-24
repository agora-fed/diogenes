//! Access control for rooms.
//!
//! Rules:
//! * The caller must be signed in, and the module must be active for the caller's org.
//! * A room belongs to one org. A caller from another org cannot see it (404).
//! * Roles: `viewer` < `editor` < `owner`. A non-member gets 404, never 403, so that
//!   nobody can test which room ids exist.

use axum::http::request::Parts;
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};
use crate::identity::Actor;
use crate::DiogenesState;

/// A role in a room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Can read.
    Viewer,
    /// Can read and change counts, priorities and notes.
    Editor,
    /// Can do all, and manage members.
    Owner,
}

impl Role {
    /// Parse a stored role.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "viewer" => Some(Self::Viewer),
            "editor" => Some(Self::Editor),
            "owner" => Some(Self::Owner),
            _ => None,
        }
    }

    /// The stored name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Editor => "editor",
            Self::Owner => "owner",
        }
    }
}

/// Return the signed-in caller. Fail when nobody is signed in or the module is off.
///
/// # Errors
/// `Unauthenticated` or `ModuleDisabled`.
pub async fn actor(state: &DiogenesState, parts: &Parts) -> ApiResult<Actor> {
    let actor = state
        .identity
        .actor(parts)
        .await
        .ok_or(ApiError::Unauthenticated)?;
    if !state.identity.module_active(actor.org_id).await {
        return Err(ApiError::ModuleDisabled);
    }
    Ok(actor)
}

/// The access of a caller to one room.
#[derive(Debug, Clone, Copy)]
pub struct RoomAccess {
    /// The room id.
    pub room_id: Uuid,
    /// The role of the caller.
    pub role: Role,
}

/// Check that the caller has at least `min` in the room.
///
/// # Errors
/// `NotFound` for a non-member or another org; `Forbidden` for a too-low role.
pub async fn require_room(
    db: &PgPool,
    actor: &Actor,
    room_id: Uuid,
    min: Role,
) -> ApiResult<RoomAccess> {
    let role: Option<String> = sqlx::query_scalar(
        "SELECT m.role FROM diogenes_room r
           JOIN diogenes_room_member m ON m.room_id = r.id
          WHERE r.id = $1 AND r.org_id = $2 AND m.user_id = $3",
    )
    .bind(room_id)
    .bind(actor.org_id)
    .bind(actor.user_id)
    .fetch_optional(db)
    .await?;
    let role = role
        .as_deref()
        .and_then(Role::parse)
        .ok_or(ApiError::NotFound)?;
    if role < min {
        return Err(ApiError::Forbidden);
    }
    Ok(RoomAccess { room_id, role })
}

/// Find the room of a project, then check access.
///
/// # Errors
/// As [`require_room`].
pub async fn require_project(
    db: &PgPool,
    actor: &Actor,
    project_id: Uuid,
    min: Role,
) -> ApiResult<RoomAccess> {
    let room: Option<Uuid> =
        sqlx::query_scalar("SELECT room_id FROM diogenes_project WHERE id = $1")
            .bind(project_id)
            .fetch_optional(db)
            .await?;
    require_room(db, actor, room.ok_or(ApiError::NotFound)?, min).await
}

/// Find the project and room of a scenario, then check access. Return (access, project id).
///
/// # Errors
/// As [`require_room`].
pub async fn require_scenario(
    db: &PgPool,
    actor: &Actor,
    scenario_id: Uuid,
    min: Role,
) -> ApiResult<(RoomAccess, Uuid)> {
    let row: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT p.room_id, p.id FROM diogenes_scenario s
           JOIN diogenes_project p ON p.id = s.project_id
          WHERE s.id = $1",
    )
    .bind(scenario_id)
    .fetch_optional(db)
    .await?;
    let (room, project) = row.ok_or(ApiError::NotFound)?;
    Ok((require_room(db, actor, room, min).await?, project))
}

/// Append one row to the audit trail.
///
/// # Errors
/// Returns a database error.
pub async fn audit(
    conn: &mut PgConnection,
    room_id: Uuid,
    actor: &Actor,
    action: &str,
    payload: serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO diogenes_audit (room_id, actor_id, action, payload) VALUES ($1, $2, $3, $4)",
    )
    .bind(room_id)
    .bind(actor.user_id)
    .bind(action)
    .bind(payload)
    .execute(conn)
    .await?;
    Ok(())
}

/// Validate a name: trim it and check the length.
///
/// # Errors
/// `Invalid` when the name is empty or too long.
pub fn clean_name(raw: &str, max: usize) -> ApiResult<String> {
    let name = raw.trim();
    if name.is_empty() || name.chars().count() > max {
        return Err(ApiError::Invalid(format!(
            "O nome deve ter entre 1 e {max} caracteres."
        )));
    }
    Ok(name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_are_ordered() {
        assert!(Role::Viewer < Role::Editor);
        assert!(Role::Editor < Role::Owner);
        for r in [Role::Viewer, Role::Editor, Role::Owner] {
            assert_eq!(Role::parse(r.as_str()), Some(r));
        }
        assert_eq!(Role::parse("admin"), None);
    }

    #[test]
    fn clean_name_trims_and_limits() {
        assert_eq!(clean_name("  PL 1  ", 10).ok().as_deref(), Some("PL 1"));
        assert!(clean_name("   ", 10).is_err());
        assert!(clean_name("abcdefghijk", 10).is_err());
    }
}
