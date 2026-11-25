//! Integration tests: sync edge cases. Failed runs, locks, the scheduler, the trigger
//! endpoint, skipped roll calls and the TSE race join.
//!
//! The tests never reach the real Câmara API. They use a local server on `[::1]`, or they
//! hold the job locks so that a job returns at once, before any request.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Router;
use chrono::{Days, NaiveDate, Utc};
use common::{app_with_operators, call, db, User, SERIAL};
use diogenes::camara::CamaraClient;
use diogenes::sync::{run_job, spawn_scheduler, Job, SyncConfig};
use serde_json::{json, Value};
use uuid::Uuid;

/// The advisory-lock keys of the jobs (see `Job::lock_key`): "DIO" and the job number.
const LOCK_KEYS: [i64; 4] = [
    0x0044_494F_0001,
    0x0044_494F_0002,
    0x0044_494F_0003,
    0x0044_494F_0004,
];

type Handler = Arc<dyn Fn(&str, &str) -> Response + Send + Sync>;

#[derive(Clone)]
struct Fake {
    handler: Handler,
    /// Every path that the client asked for.
    seen: Arc<Mutex<Vec<String>>>,
}

async fn respond(State(fake): State<Fake>, req: Request) -> Response {
    let path = req.uri().path().trim_start_matches("/api/v2/").to_owned();
    let query = req.uri().query().unwrap_or("").to_owned();
    fake.seen.lock().unwrap().push(path.clone());
    (fake.handler)(&path, &query)
}

/// Start a fake API. The handler gets the path (after `/api/v2/`) and the query.
async fn start_fake(
    handler: impl Fn(&str, &str) -> Response + Send + Sync + 'static,
) -> (CamaraClient, Fake) {
    let fake = Fake {
        handler: Arc::new(handler),
        seen: Arc::default(),
    };
    let app = Router::new().fallback(respond).with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("[::1]:0").await.unwrap();
    let base = format!("http://{}/api/v2", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (CamaraClient::with_base(&base).unwrap(), fake)
}

fn json_ok(body: &Value) -> Response {
    axum::Json(body.clone()).into_response()
}

fn config(since: NaiveDate) -> SyncConfig {
    SyncConfig {
        roll_calls_since: since,
        roll_calls_every: Duration::from_secs(3600),
        reference_every: Duration::from_secs(3600),
        tse_enabled: false,
        finance_url: String::new(),
    }
}

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

async fn run_count(db: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM diogenes_sync_run")
        .fetch_one(db)
        .await
        .unwrap()
}

/// Hold the advisory locks of all jobs on one dedicated connection.
/// Drop the connection to release them.
async fn hold_all_locks(db: &sqlx::PgPool) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
    let mut conn = db.acquire().await.unwrap();
    for key in LOCK_KEYS {
        let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(key)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert!(got, "the test must get the lock {key:#x}");
    }
    conn
}

async fn release_all_locks(mut conn: sqlx::pool::PoolConnection<sqlx::Postgres>) {
    for key in LOCK_KEYS {
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(key)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn a_job_that_cannot_reach_the_api_is_recorded_as_failed() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    let (client, _) = start_fake(|_, _| StatusCode::NOT_FOUND.into_response()).await;

    let run_id = run_job(
        &db,
        &client,
        &config(Utc::now().date_naive()),
        Job::Deputies,
    )
    .await
    .unwrap()
    .expect("the job ran");

    let (status, items, error, finished): (String, i32, Option<String>, bool) = sqlx::query_as(
        "SELECT status, items, error, finished_at IS NOT NULL FROM diogenes_sync_run WHERE id = $1",
    )
    .bind(run_id)
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!((status.as_str(), items, finished), ("failed", 0, true));
    assert!(
        error.as_deref().is_some_and(|e| e.contains("status 404")),
        "{error:?}"
    );
    // The lock is free again: the job can run a second time.
    assert!(run_job(
        &db,
        &client,
        &config(Utc::now().date_naive()),
        Job::Deputies
    )
    .await
    .unwrap()
    .is_some());
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn a_job_does_not_run_when_another_process_holds_its_lock() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    let (client, fake) = start_fake(|_, _| StatusCode::NOT_FOUND.into_response()).await;
    let held = hold_all_locks(&db).await;

    for job in Job::ALL {
        let result = run_job(&db, &client, &config(Utc::now().date_naive()), job)
            .await
            .unwrap();
        assert_eq!(result, None, "{}", job.name());
    }

    assert_eq!(run_count(&db).await, 0, "a refused job leaves no run row");
    assert!(fake.seen.lock().unwrap().is_empty(), "and sends no request");
    release_all_locks(held).await;
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn scheduler_tries_every_job_at_start_and_keeps_running() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    // With all locks held, each job returns at once. The scheduler sends no request.
    let held = hold_all_locks(&db).await;

    let handle = spawn_scheduler(db.clone(), config(Utc::now().date_naive()));
    tokio::time::sleep(Duration::from_millis(500)).await;

    assert!(!handle.is_finished(), "the scheduler loop does not end");
    assert_eq!(run_count(&db).await, 0);
    handle.abort();
    let joined = handle.await;
    assert!(joined.is_err_and(|e| e.is_cancelled()));
    release_all_locks(held).await;
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn trigger_endpoint_checks_permission_and_job_name() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    let org = Uuid::now_v7();
    let operator = User::new(org, "Ops", "ops", false);
    let user = User::new(org, "Ana", "ana", true);
    let app = app_with_operators(db.clone(), &[&operator, &user], &[&operator]);

    let (s, body) = call(
        &app,
        "POST",
        "/diogenes/sync/groups",
        Some(&user),
        Some(json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
    let (s, body) = call(
        &app,
        "POST",
        "/diogenes/sync/nope",
        Some(&operator),
        Some(json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");

    // A valid job starts in the background. The locks are held, so it ends at once.
    let held = hold_all_locks(&db).await;
    let (s, body) = call(
        &app,
        "POST",
        "/diogenes/sync/roll_calls",
        Some(&operator),
        Some(json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["data"], "started");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(run_count(&db).await, 0);
    release_all_locks(held).await;
}

/// A roll-call list that has: a good roll call, one with a bad date and one whose votes fail.
fn roll_call_fake(today: NaiveDate) -> impl Fn(&str, &str) -> Response + Send + Sync + 'static {
    let old = (today - Days::new(10)).to_string();
    let recent = (today - Days::new(1)).to_string();
    let votes = Arc::new(AtomicUsize::new(0));
    move |path, _| match path {
        "votacoes" => json_ok(&json!({"dados": [
            {"id": "100-1", "data": old, "descricao": "Sim: 1; Não: 0", "siglaOrgao": "PLEN"},
            {"id": "100-2", "data": "not-a-date", "siglaOrgao": "PLEN"},
            {"id": "100-3", "data": recent, "siglaOrgao": "PLEN"},
        ], "links": []})),
        "votacoes/100-1/votos" => {
            votes.fetch_add(1, Ordering::SeqCst);
            let mut body = fixture("votos.json");
            body["dados"].as_array_mut().unwrap().truncate(1);
            json_ok(&body)
        }
        "votacoes/100-1/orientacoes" => json_ok(&json!({"dados": []})),
        "votacoes/100-1" => json_ok(&fixture("votacao_detail.json")),
        // An error that is not 404 and not retried: the job skips this roll call.
        "votacoes/100-3/votos" => StatusCode::FORBIDDEN.into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn roll_calls_job_skips_bad_entries_and_old_stored_roll_calls() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    let today = Utc::now().date_naive();
    let (client, fake) = start_fake(roll_call_fake(today)).await;
    let since = today - Days::new(12);

    run_job(&db, &client, &config(since), Job::RollCalls)
        .await
        .unwrap()
        .expect("ran");

    let (status, items, checks): (String, i32, Value) = sqlx::query_as(
        "SELECT status, items, checks FROM diogenes_sync_run WHERE job = 'roll_calls'",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!((status.as_str(), items), ("warning", 1));
    let errors = checks
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["subject"] == "roll_call_errors")
        .unwrap();
    assert_eq!(
        (errors["expected"].as_i64(), errors["actual"].as_i64()),
        (Some(0), Some(1))
    );
    let stored: Vec<String> = sqlx::query_scalar("SELECT id FROM diogenes_roll_call ORDER BY id")
        .fetch_all(&db)
        .await
        .unwrap();
    assert_eq!(stored, vec!["100-1".to_owned()]);

    // The second run: "100-1" is older than the refresh period and stored, so it is not
    // fetched again. "100-3" is young and fails again.
    run_job(&db, &client, &config(since), Job::RollCalls)
        .await
        .unwrap()
        .expect("ran");
    let fetched = fake
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter(|p| p.as_str() == "votacoes/100-1/votos")
        .count();
    assert_eq!(fetched, 1, "a stored old roll call is not fetched again");
    let (_, items2): (String, i32) = sqlx::query_as(
        "SELECT status, items FROM diogenes_sync_run WHERE job = 'roll_calls' ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(items2, 0);
}

/// Two deputies in office. Deputy 1 has a CPF that the TSE map knows. Deputy 2 has no CPF.
fn deputies_fake(path: &str, query: &str) -> Response {
    match path {
        "partidos" => json_ok(&json!({"dados": [], "links": []})),
        "deputados" if query.contains("siglaSexo=F") => json_ok(&json!({"dados": [], "links": []})),
        "deputados" => json_ok(&json!({"dados": [
            {"id": 1, "nome": "Um", "siglaPartido": "PT", "siglaUf": "SP"},
            {"id": 2, "nome": "Dois", "siglaPartido": "PL", "siglaUf": "RJ"}], "links": []})),
        "deputados/1" => json_ok(&json!({"dados": {"id": 1, "nomeCivil": "UM", "sexo": "M",
            "cpf": "123.456.789-01", "ultimoStatus": {"situacao": "Exercício"}}})),
        "deputados/2" => json_ok(&json!({"dados": {"id": 2, "nomeCivil": "DOIS", "sexo": "M",
            "cpf": null, "ultimoStatus": {"situacao": "Exercício"}}})),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn deputies_job_joins_the_tse_races_by_cpf() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    let (client, _) = start_fake(deputies_fake).await;
    let races: HashMap<String, String> =
        HashMap::from([("12345678901".to_owned(), "PARDA".to_owned())]);

    let report = diogenes::sync::deputies::run_with_races(&db, &client, Some(races))
        .await
        .unwrap();

    let stored: Vec<(i32, Option<String>)> =
        sqlx::query_as("SELECT camara_id, race FROM diogenes_deputy ORDER BY camara_id")
            .fetch_all(&db)
            .await
            .unwrap();
    assert_eq!(stored, vec![(1, Some("PARDA".to_owned())), (2, None)]);
    let check = report
        .checks
        .iter()
        .find(|c| c.subject == "tse_race_matches")
        .expect("the race check exists when a map is given");
    assert_eq!((check.expected, check.actual), (2, 1));
    assert!(!check.passed());

    // Without a map, the job keeps the stored values and adds no race check.
    let report = diogenes::sync::deputies::run_with_races(&db, &client, None)
        .await
        .unwrap();
    assert!(report
        .checks
        .iter()
        .all(|c| c.subject != "tse_race_matches"));
    let kept: Option<String> =
        sqlx::query_scalar("SELECT race FROM diogenes_deputy WHERE camara_id = 1")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(kept.as_deref(), Some("PARDA"));
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn a_job_frees_its_lock_when_the_run_cannot_be_recorded() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    let (client, _) = start_fake(|_, _| StatusCode::NOT_FOUND.into_response()).await;
    // Make the INSERT of the run row fail.
    sqlx::raw_sql("ALTER TABLE diogenes_sync_run RENAME TO diogenes_sync_run_off")
        .execute(&db)
        .await
        .unwrap();

    let result = run_job(&db, &client, &config(Utc::now().date_naive()), Job::Groups).await;

    sqlx::raw_sql("ALTER TABLE diogenes_sync_run_off RENAME TO diogenes_sync_run")
        .execute(&db)
        .await
        .unwrap();
    assert!(result.is_err(), "the run row cannot be written");
    // Another session must get the lock: the failed run must not keep it on a pooled connection.
    let other = sqlx::PgPool::connect(&std::env::var("DATABASE_URL").unwrap())
        .await
        .unwrap();
    let mut conn = other.acquire().await.unwrap();
    let free: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(LOCK_KEYS[1])
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    sqlx::query("SELECT pg_advisory_unlock_all()")
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(free, "the job lock is still held by the pool");
}
