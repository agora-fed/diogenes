//! # DIOGENES
//!
//! Vote count ("whip count") and articulation rooms for the Brazilian Chamber of Deputies.
//! DIOGENES is an add-on for the AGORA framework. It has no dependency on AGORA crates.
//!
//! The host gives DIOGENES three things:
//! 1. A PostgreSQL pool ([`sqlx::PgPool`]).
//! 2. An [`Identity`] implementation. It tells DIOGENES who the caller is.
//! 3. A mount point. The host merges [`api_router`] under its API prefix and
//!    [`web_router`] at the site root.
//!
//! DIOGENES owns all tables with the prefix `diogenes_`. Call [`migrate`] at start.

pub mod camara;
pub mod error;
pub mod identity;
pub mod migrate;
pub mod reference;
pub mod room;
pub mod scoreboard;
pub mod sync;
pub mod text;
pub mod tse;
pub mod web;

use std::sync::Arc;

use axum::Router;
use sqlx::PgPool;

pub use error::{ApiError, ApiResult, Envelope};
pub use identity::{Actor, Identity, UserRef};
pub use migrate::migrate;

/// The shared state of all DIOGENES handlers.
#[derive(Clone)]
pub struct DiogenesState {
    /// The database pool. The host owns it.
    pub db: PgPool,
    /// The identity adapter. The host implements it.
    pub identity: Arc<dyn Identity>,
    /// Live updates for open scenarios (server-sent events).
    pub hub: room::events::Hub,
}

impl std::fmt::Debug for DiogenesState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiogenesState").finish_non_exhaustive()
    }
}

impl DiogenesState {
    /// Make the state from a pool and an identity adapter.
    #[must_use]
    pub fn new(db: PgPool, identity: Arc<dyn Identity>) -> Self {
        Self {
            db,
            identity,
            hub: room::events::Hub::default(),
        }
    }
}

/// The JSON API. The host merges it under its API prefix (for example `/api/v1`).
/// All routes start with `/diogenes/`.
pub fn api_router(state: DiogenesState) -> Router<()> {
    Router::new()
        .merge(reference::routes())
        .merge(room::routes())
        .merge(sync::routes())
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(axum::middleware::map_response(no_store))
        .with_state(state)
}

/// The largest request body that the API accepts.
pub const MAX_BODY_BYTES: usize = 256 * 1024;

/// Room data is private: no shared cache and no browser cache may keep it.
async fn no_store(mut response: axum::response::Response) -> axum::response::Response {
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}

/// The web pages of the articulation area. The host merges it at the site root.
/// All routes start with `/articulacao`.
pub fn web_router() -> Router<()> {
    web::routes()
}
