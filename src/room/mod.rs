//! Articulation rooms: private vote counts for a team.
//!
//! A room has members with roles. A room has projects (one bill or one vote each).
//! A project has columns, priorities, notes and scenarios. A scenario holds one
//! position per deputy. Every change writes an audit row and sends a live event.

pub mod access;
pub mod events;
pub mod projects;
pub mod rooms;
pub mod scenarios;

use axum::routing::{delete, get, patch, post, put};
use axum::Router;

use crate::DiogenesState;

pub(crate) fn routes() -> Router<DiogenesState> {
    Router::new()
        .route("/diogenes/rooms", get(rooms::list).post(rooms::create))
        .route(
            "/diogenes/rooms/{room_id}",
            get(rooms::detail)
                .patch(rooms::rename)
                .delete(rooms::delete),
        )
        .route("/diogenes/rooms/{room_id}/members", post(rooms::add_member))
        .route(
            "/diogenes/rooms/{room_id}/members/{user_id}",
            delete(rooms::remove_member),
        )
        .route("/diogenes/rooms/{room_id}/audit", get(rooms::audit_log))
        .route("/diogenes/rooms/{room_id}/projects", post(projects::create))
        .route(
            "/diogenes/projects/{project_id}",
            get(projects::detail)
                .patch(projects::patch)
                .delete(projects::delete),
        )
        .route(
            "/diogenes/projects/{project_id}/columns",
            post(projects::add_column),
        )
        .route(
            "/diogenes/columns/{column_id}",
            patch(projects::rename_column).delete(projects::delete_column),
        )
        .route(
            "/diogenes/projects/{project_id}/priorities",
            put(projects::set_priorities),
        )
        .route(
            "/diogenes/projects/{project_id}/notes",
            post(projects::add_note),
        )
        .route(
            "/diogenes/notes/{note_id}",
            patch(projects::edit_note).delete(projects::delete_note),
        )
        .route(
            "/diogenes/projects/{project_id}/match-list",
            post(projects::match_list),
        )
        .route(
            "/diogenes/projects/{project_id}/events",
            get(projects::events),
        )
        .route(
            "/diogenes/projects/{project_id}/scenarios",
            post(scenarios::create),
        )
        .route(
            "/diogenes/scenarios/{scenario_id}",
            get(scenarios::detail)
                .patch(scenarios::rename)
                .delete(scenarios::delete),
        )
        .route(
            "/diogenes/scenarios/{scenario_id}/positions",
            put(scenarios::move_positions),
        )
}
