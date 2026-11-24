//! Shared test support: a database, a fake identity and request helpers.
//!
//! The integration tests need PostgreSQL. Set `DATABASE_URL` and run
//! `cargo test -- --include-ignored`. The tests empty the `diogenes_` tables, so use a
//! database only for tests.

#![allow(dead_code, unreachable_pub, clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::request::Parts;
use axum::http::{Request, StatusCode};
use axum::Router;
use diogenes::{Actor, DiogenesState, Identity, UserRef};
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

/// One test at a time: the tests share the tables.
pub static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub async fn db() -> PgPool {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must point to a test database");
    let db = PgPool::connect(&url).await.expect("connect");
    diogenes::migrate(&db).await.expect("migrate");
    sqlx::raw_sql(
        "TRUNCATE diogenes_room, diogenes_deputy, diogenes_party, diogenes_group,
                  diogenes_bill, diogenes_roll_call, diogenes_sync_run CASCADE",
    )
    .execute(&db)
    .await
    .expect("truncate");
    db
}

/// A test user.
#[derive(Clone, Debug)]
pub struct User {
    pub id: Uuid,
    pub org: Uuid,
    pub name: &'static str,
    pub handle: &'static str,
    pub can_create: bool,
}

impl User {
    pub fn new(org: Uuid, name: &'static str, handle: &'static str, can_create: bool) -> Self {
        Self {
            id: Uuid::now_v7(),
            org,
            name,
            handle,
            can_create,
        }
    }
}

/// A fake identity: the caller is in the `x-test-user` header.
#[derive(Default)]
pub struct FakeIdentity {
    pub users: HashMap<Uuid, User>,
    pub disabled_orgs: Vec<Uuid>,
}

#[async_trait]
impl Identity for FakeIdentity {
    async fn actor(&self, parts: &Parts) -> Option<Actor> {
        let id: Uuid = parts
            .headers
            .get("x-test-user")?
            .to_str()
            .ok()?
            .parse()
            .ok()?;
        let u = self.users.get(&id)?;
        Some(Actor {
            user_id: u.id,
            org_id: u.org,
            display_name: u.name.to_owned(),
            can_create_rooms: u.can_create,
            can_operate_sync: false,
        })
    }

    async fn find_user(&self, org_id: Uuid, handle: &str) -> Option<UserRef> {
        self.users
            .values()
            .find(|u| u.org == org_id && u.handle == handle)
            .map(|u| UserRef {
                user_id: u.id,
                display_name: u.name.to_owned(),
            })
    }

    async fn module_active(&self, org_id: Uuid) -> bool {
        !self.disabled_orgs.contains(&org_id)
    }
}

pub fn app(db: PgPool, users: &[&User], disabled_orgs: Vec<Uuid>) -> Router {
    let identity = FakeIdentity {
        users: users.iter().map(|u| (u.id, (*u).clone())).collect(),
        disabled_orgs,
    };
    diogenes::api_router(DiogenesState::new(db, Arc::new(identity)))
}

/// Send a request. Return the status and the JSON body.
pub async fn call(
    app: &Router,
    method: &str,
    path: &str,
    user: Option<&User>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(u) = user {
        req = req.header("x-test-user", u.id.to_string());
    }
    let req = match body {
        Some(b) => req
            .header("content-type", "application/json")
            .body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

/// Insert `n` deputies in office with ids 1..=n. Deputy 1 is the chair.
pub async fn seed_deputies(db: &PgPool, n: i32) {
    for id in 1..=n {
        sqlx::query(
            "INSERT INTO diogenes_deputy (camara_id, name, party, uf, in_office) VALUES ($1, $2, $3, 'SP', true)",
        )
        .bind(id)
        .bind(format!("Deputado {id:03}"))
        .bind(if id % 2 == 0 { "PT" } else { "PL" })
        .execute(db)
        .await
        .unwrap();
    }
    sqlx::query("INSERT INTO diogenes_group (id, kind, title, source) VALUES ('caucus:board', 'caucus', 'Mesa', 'camara')")
        .execute(db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO diogenes_group_member (group_id, camara_id, role) VALUES ('caucus:board', 1, 'Presidente')")
        .execute(db)
        .await
        .unwrap();
}
