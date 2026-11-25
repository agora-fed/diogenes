//! Integration test of the sync jobs against a fake Câmara API.
//! The fake API serves real responses (trimmed) from `tests/fixtures/`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Router;
use chrono::NaiveDate;
use common::{db, SERIAL};
use diogenes::camara::CamaraClient;
use diogenes::sync::{run_job, Job, SyncConfig};
use serde_json::{json, Value};

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[derive(Clone, Default)]
struct Fake {
    calls: Arc<AtomicUsize>,
    /// True: the list of fronts has a second front that the API does not know (HTTP 404).
    broken_front: bool,
}

async fn fake_api(State(fake): State<Fake>, req: Request) -> Response {
    // The first request gets HTTP 429: the client must wait and retry.
    if fake.calls.fetch_add(1, Ordering::SeqCst) == 0 {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [("retry-after", "0")],
            "slow down",
        )
            .into_response();
    }
    let path = req.uri().path().trim_start_matches("/api/v2/").to_owned();
    let query = req.uri().query().unwrap_or("").to_owned();
    let deputies = fixture("deputados.json");
    let body = match path.as_str() {
        "partidos" => {
            json!({"dados": [{"id": 37906, "sigla": "PL", "nome": "Partido Liberal"}], "links": []})
        }
        "deputados" if query.contains("siglaSexo=F") => {
            let women: Vec<Value> = deputies["dados"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|d| d["id"].as_i64().unwrap() % 2 == 0)
                .cloned()
                .collect();
            json!({"dados": women, "links": []})
        }
        "deputados" => deputies,
        p if p.starts_with("deputados/") => {
            let id: i64 = p["deputados/".len()..].parse().unwrap();
            json!({"dados": {"id": id, "nomeCivil": "NOME CIVIL", "cpf": null,
                   "sexo": if id % 2 == 0 { "F" } else { "M" },
                   "ultimoStatus": {"situacao": "Exercício"}}})
        }
        "legislaturas/57/lideres" => {
            // A current leader with two roles: the deputy is two times in the list.
            let mut body = fixture("lideres.json");
            let list = body["dados"].as_array_mut().unwrap();
            let mut twin = list
                .iter()
                .find(|l| l["dataFim"].is_null())
                .unwrap()
                .clone();
            twin["titulo"] = json!("Vice-Líder");
            list.push(twin);
            body
        }
        "orgaos/4/membros" => fixture("mesa.json"),
        "frentes" => {
            let mut fronts = vec![
                json!({"id": 55703, "titulo": "Frente Parlamentar Mista do Serviço Exterior Brasileiro"}),
            ];
            if fake.broken_front {
                fronts.push(json!({"id": 99999, "titulo": "Frente sem dados"}));
            }
            json!({"dados": fronts, "links": []})
        }
        "frentes/55703" => fixture("frente_detail.json"),
        // Like the real API: this list is not paged and rejects `itens`.
        "frentes/55703/membros" if query.contains("itens") => {
            return (StatusCode::BAD_REQUEST, "itens not allowed").into_response();
        }
        "frentes/55703/membros" => {
            // Like the real API: one entry with no deputy ("id": null, "nome": null).
            let mut body = fixture("frente_membros.json");
            body["dados"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id": null, "nome": null, "titulo": "Membro"}));
            body
        }
        "votacoes" => {
            // A second roll call: its vote list gives HTTP 404 (the fallback below).
            let mut body = fixture("votacoes.json");
            let mut old = body["dados"][0].clone();
            old["id"] = json!("1-404");
            body["dados"].as_array_mut().unwrap().push(old);
            body
        }
        "votacoes/2655570-60/votos" => fixture("votos.json"),
        "votacoes/2655570-60/orientacoes" => fixture("orientacoes.json"),
        "votacoes/2655570-60" => fixture("votacao_detail.json"),
        _ => return (StatusCode::NOT_FOUND, "no fixture").into_response(),
    };
    axum::Json(body).into_response()
}

async fn start_fake() -> (String, Fake) {
    start_fake_with(Fake::default()).await
}

async fn start_fake_with(fake: Fake) -> (String, Fake) {
    let app = Router::new().fallback(fake_api).with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("[::1]:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/api/v2"), fake)
}

async fn last_run(db: &sqlx::PgPool, job: &str) -> (String, Value) {
    sqlx::query_as(
        "SELECT status, checks FROM diogenes_sync_run WHERE job = $1 ORDER BY id DESC LIMIT 1",
    )
    .bind(job)
    .fetch_one(db)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn sync_jobs_store_reference_data_and_checks() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    let (base, fake) = start_fake().await;
    let client = CamaraClient::with_base(&base).unwrap();
    let config = SyncConfig {
        roll_calls_since: NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
        roll_calls_every: std::time::Duration::from_secs(3600),
        reference_every: std::time::Duration::from_secs(3600),
        tse_enabled: false,
    };

    // --- deputies ---
    run_job(&db, &client, &config, Job::Deputies)
        .await
        .unwrap()
        .expect("ran");
    let in_office: i64 = sqlx::query_scalar("SELECT count(*) FROM diogenes_deputy WHERE in_office")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(in_office, 6);
    let (status, checks) = last_run(&db, "deputies").await;
    // The fixture has 6 deputies, not 513: the check fails and the run is a warning.
    assert_eq!(status, "warning");
    let women = checks
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["subject"] == "women_in_office")
        .unwrap();
    assert_eq!(women["expected"], women["actual"]);
    assert!(
        fake.calls.load(Ordering::SeqCst) > 1,
        "the client retried after 429"
    );

    // --- groups ---
    run_job(&db, &client, &config, Job::Groups)
        .await
        .unwrap()
        .expect("ran");
    let (status, checks) = last_run(&db, "groups").await;
    assert_eq!(status, "ok", "{checks}");
    let (total, members): (Option<i32>, i64) = sqlx::query_as(
        "SELECT g.official_total, (SELECT count(*) FROM diogenes_group_member m WHERE m.group_id = g.id)
           FROM diogenes_group g WHERE g.id = 'front:55703'",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!((total, members), (Some(197), 197));
    // The fixture of the board has the chair (Câmara id 160674) with no end date.
    let chair: Option<i32> = sqlx::query_scalar(
        "SELECT camara_id FROM diogenes_group_member WHERE group_id = 'caucus:board' AND role = 'Presidente'",
    )
    .fetch_optional(&db)
    .await
    .unwrap();
    assert_eq!(chair, Some(160_674), "the chair is known");
    let leaders: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM diogenes_group_member WHERE group_id = 'caucus:leaders'",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert!(leaders > 0);
    let women: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM diogenes_group_member WHERE group_id = 'caucus:women'",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    let women_expected: i64 =
        sqlx::query_scalar("SELECT count(*) FROM diogenes_deputy WHERE in_office AND sex = 'F'")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(women, women_expected);

    // --- roll calls ---
    run_job(&db, &client, &config, Job::RollCalls)
        .await
        .unwrap()
        .expect("ran");
    let (status, checks) = last_run(&db, "roll_calls").await;
    assert_eq!(status, "ok", "{checks}");
    let (votes, totals_match, bill): (i64, Option<bool>, Option<i32>) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM diogenes_roll_call_vote v WHERE v.roll_call_id = r.id), r.totals_match, r.bill_id
           FROM diogenes_roll_call r WHERE r.id = '2655570-60'",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(votes, 455);
    assert_eq!(
        totals_match,
        Some(true),
        "319 + 127 + 1 match the description"
    );
    assert_eq!(bill, Some(2_655_570));
    let orientations: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM diogenes_roll_call_orientation WHERE roll_call_id = '2655570-60'",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(orientations, 15);
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn groups_job_skips_a_front_that_fails_and_reports_it() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    let (base, _) = start_fake_with(Fake {
        broken_front: true,
        ..Fake::default()
    })
    .await;
    let client = CamaraClient::with_base(&base).unwrap();
    let config = SyncConfig {
        roll_calls_since: NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
        roll_calls_every: std::time::Duration::from_secs(3600),
        reference_every: std::time::Duration::from_secs(3600),
        tse_enabled: false,
    };
    run_job(&db, &client, &config, Job::Deputies)
        .await
        .unwrap()
        .expect("ran");

    run_job(&db, &client, &config, Job::Groups)
        .await
        .unwrap()
        .expect("ran");

    let (status, checks) = last_run(&db, "groups").await;
    assert_eq!(status, "warning", "{checks}");
    let errors = checks
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["subject"] == "front_errors")
        .unwrap();
    assert_eq!(
        (errors["expected"].as_i64(), errors["actual"].as_i64()),
        (Some(0), Some(1))
    );
    let stored: Vec<String> =
        sqlx::query_scalar("SELECT id FROM diogenes_group WHERE id LIKE 'front:%'")
            .fetch_all(&db)
            .await
            .unwrap();
    assert_eq!(
        stored,
        vec!["front:55703".to_owned()],
        "the good front is stored"
    );
}
