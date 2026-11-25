//! Development server. It runs DIOGENES alone, with a fixed development identity.
//!
//! WARNING: every request is the same user. Use this only on your own computer.
//!
//! ```sh
//! DATABASE_URL=postgres:///diogenes_dev cargo run --example dev_server
//! # Open http://[::1]:8090/articulacao/
//! ```
//!
//! Set `DIOGENES_DEV_SYNC=1` to run the data sync at start (several minutes).

#![allow(clippy::expect_used)]

use std::net::{Ipv6Addr, SocketAddr};
use std::sync::Arc;

use async_trait::async_trait;
use axum::http::request::Parts;
use axum::Router;
use diogenes::{Actor, DiogenesState, Identity, UserRef};
use uuid::Uuid;

struct DevIdentity {
    user: Uuid,
    org: Uuid,
}

#[async_trait]
impl Identity for DevIdentity {
    async fn actor(&self, _parts: &Parts) -> Option<Actor> {
        Some(Actor {
            user_id: self.user,
            org_id: self.org,
            display_name: "Pessoa de teste".into(),
            can_create_rooms: true,
            can_operate_sync: true,
        })
    }

    async fn find_user(&self, _org_id: Uuid, handle: &str) -> Option<UserRef> {
        // Every handle is a new fake user, so that the member flow can be tried.
        Some(UserRef {
            user_id: Uuid::from_u128(handle.bytes().fold(1_u128, |acc, b| {
                acc.wrapping_mul(31).wrapping_add(u128::from(b))
            })),
            display_name: format!("@{handle}"),
        })
    }
}

#[tokio::main]
async fn main() {
    let url = std::env::var("DATABASE_URL").expect("set DATABASE_URL");
    let db = sqlx::PgPool::connect(&url).await.expect("connect");
    diogenes::migrate(&db).await.expect("migrate");
    if std::env::var("DIOGENES_DEV_SYNC").is_ok() {
        let _sync =
            diogenes::sync::spawn_scheduler(db.clone(), diogenes::sync::SyncConfig::from_env());
    }
    let identity = DevIdentity {
        user: Uuid::from_u128(1),
        org: Uuid::from_u128(1),
    };
    let state = DiogenesState::new(db, Arc::new(identity));
    let app = Router::new()
        .nest("/api/v1", diogenes::api_router(state))
        .merge(diogenes::web_router());
    let addr = SocketAddr::from((Ipv6Addr::LOCALHOST, 8090));
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    println!("http://[::1]:8090/articulacao/");
    axum::serve(listener, app).await.expect("serve");
}
