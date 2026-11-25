//! Integration test: the `donations` job against a fake Câmara API and a fake TSE archive,
//! then the finance API (ADR-0009).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::io::{Cursor, Write};

use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Router;
use common::{app, call, db, seed_deputies, User, SERIAL};
use diogenes::camara::CamaraClient;
use diogenes::finance::parse::{ORIGINAL_DONORS_FILE, RECEIPTS_FILE};
use diogenes::sync::{run_job, Job, SyncConfig};
use serde_json::{json, Value};
use uuid::Uuid;

fn l1(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| u8::try_from(u32::from(c)).unwrap())
        .collect()
}

fn archive(files: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(&mut out);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, body) in files {
        writer.start_file(*name, options).unwrap();
        writer.write_all(body).unwrap();
    }
    writer.finish().unwrap();
    out.into_inner()
}

const HEAD: &str = "DS_CARGO;NR_CPF_CANDIDATO;SQ_RECEITA;DS_ORIGEM_RECEITA;NR_CPF_CNPJ_DOADOR;NM_DOADOR;DS_CNAE_DOADOR;SG_UF_DOADOR;VR_RECEITA\n";

fn receipts() -> Vec<u8> {
    // CPF of deputy n = n repeated 11 times. Donor 7… gives to deputies 1 and 3.
    let rows = [
        "Deputado Federal;11111111111;10;Recursos de pessoas físicas;77777777777;JOSÉ DOADOR;#NULO;SP;1000,00",
        "Deputado Federal;33333333333;11;Recursos de pessoas físicas;77777777777;JOSÉ DOADOR;#NULO;SP;500,00",
        "Deputado Federal;11111111111;12;Recursos de partido político;12345678000199;PARTIDO;#NULO;#NULO;200000,00",
        "Deputado Federal;22222222222;13;Recursos de pessoas físicas;88888888888;ANA;#NULO;RJ;50,00",
        "Deputado Federal;22222222222;14;Recursos de outros candidatos;98765432000100;ELEICAO 2022 FULANO;#NULO;RJ;300,00",
        "Deputado Federal;33333333333;15;Recursos próprios;33333333333;O PRÓPRIO;#NULO;SP;10,00",
        "Deputado Federal;33333333333;16;Recursos de pessoas físicas;99999999999;ERRO;#NULO;SP;x",
        "Deputado Estadual;11111111111;17;Recursos de pessoas físicas;77777777777;JOSÉ DOADOR;#NULO;SP;9,00",
    ];
    l1(&format!("{HEAD}{}\n", rows.join("\n")))
}

fn originals() -> Vec<u8> {
    l1("SQ_RECEITA;NR_CPF_CNPJ_DOADOR_ORIGINARIO;NM_DOADOR_ORIGINARIO;TP_DOADOR_ORIGINARIO;DS_CNAE_DOADOR_ORIGINARIO;VR_RECEITA\n\
12;77777777777;JOSÉ DOADOR;F;#NULO;150000,00\n\
12;-1;#NULO;#NULO;#NULO;-1\n")
}

/// A fake Câmara API (deputy details with CPF) and the TSE archive at `/tse.zip`.
async fn start(archive_body: Vec<u8>) -> (String, String) {
    let app = Router::new().fallback(move |req: Request| {
        let archive_body = archive_body.clone();
        async move {
            let path = req.uri().path().to_owned();
            if path == "/tse.zip" {
                return archive_body.into_response();
            }
            let Some(id) = path.strip_prefix("/api/v2/deputados/") else {
                return StatusCode::NOT_FOUND.into_response();
            };
            let id: u32 = id.parse().unwrap();
            // Deputy 4 has no CPF in the API.
            let cpf = (id != 4).then(|| id.to_string().repeat(11));
            axum::Json(json!({"dados": {"id": id, "cpf": cpf, "sexo": "F"}})).into_response()
        }
    });
    let listener = tokio::net::TcpListener::bind("[::1]:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (
        format!("http://{addr}/api/v2"),
        format!("http://{addr}/tse.zip"),
    )
}

fn config(url: String) -> SyncConfig {
    SyncConfig {
        roll_calls_since: chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
        roll_calls_every: std::time::Duration::from_secs(3600),
        reference_every: std::time::Duration::from_secs(3600),
        tse_enabled: false,
        finance_url: url,
    }
}

async fn last_run(db: &sqlx::PgPool) -> (String, Value, Option<String>) {
    sqlx::query_as(
        "SELECT status, checks, error FROM diogenes_sync_run WHERE job = 'donations' ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(db)
    .await
    .unwrap()
}

fn id(v: &Value) -> String {
    v["data"].as_str().map_or_else(
        || v["data"]["id"].as_str().unwrap().to_owned(),
        str::to_owned,
    )
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn donations_import_and_cross_with_a_scenario() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    seed_deputies(&db, 4).await;
    let body = archive(&[
        (RECEIPTS_FILE, receipts()),
        (ORIGINAL_DONORS_FILE, originals()),
    ]);
    let (base, url) = start(body).await;
    let client = CamaraClient::with_base(&base).unwrap();
    run_job(&db, &client, &config(url), Job::Donations)
        .await
        .unwrap()
        .expect("ran");
    let (status, checks, error) = last_run(&db).await;
    assert_eq!(error, None);
    // Deputy 4 has no CPF and one row has a bad value: the run is a warning.
    assert_eq!(status, "warning");
    let check = |subject: &str| {
        checks
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["subject"] == subject)
            .unwrap()
            .clone()
    };
    assert_eq!(check("deputies_with_accounts")["actual"], 3);
    assert_eq!(check("deputies_with_accounts")["expected"], 4);
    assert_eq!(check("rejected_rows")["actual"], 1);
    // No CPF and no raw document is stored.
    let leaked: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM diogenes_donor WHERE key LIKE '%7777777%' OR name LIKE '%7777%')",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert!(!leaked);

    // The API: signed-in users of an org with the module on.
    let org = Uuid::now_v7();
    let owner = User::new(org, "Ana", "ana", true);
    let stranger = User::new(org, "Bia", "bia", false);
    let app = app(db.clone(), &[&owner, &stranger], vec![]);
    assert_eq!(
        call(&app, "GET", "/diogenes/deputies/1/campaign", None, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/diogenes/deputies/999/campaign",
            Some(&stranger),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let (s, c) = call(
        &app,
        "GET",
        "/diogenes/deputies/1/campaign",
        Some(&stranger),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let c = &c["data"];
    assert_eq!(c["year"], 2022);
    assert!(c["source_url"].as_str().unwrap().contains("tse.jus.br"));
    assert!(c["imported_at"].is_string());
    assert_eq!(c["total_cents"], 20_100_000);
    assert_eq!(c["totals"][0]["source"], "party");
    let donors = c["donors"].as_array().unwrap();
    assert_eq!(donors.len(), 2);
    assert_eq!(
        (
            donors[0]["name"].as_str(),
            donors[0]["via_party"].as_bool(),
            donors[0]["cents"].as_i64()
        ),
        (Some("JOSÉ DOADOR"), Some(true), Some(15_000_000))
    );
    assert_eq!(donors[1]["via_party"], false);
    assert_eq!(donors[0]["key"], donors[1]["key"], "one person, one key");
    let (_, c2) = call(
        &app,
        "GET",
        "/diogenes/deputies/2/campaign",
        Some(&stranger),
        None,
    )
    .await;
    let kinds: Vec<&str> = c2["data"]["donors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"candidate"));
    let (_, c4) = call(
        &app,
        "GET",
        "/diogenes/deputies/4/campaign",
        Some(&stranger),
        None,
    )
    .await;
    assert_eq!(c4["data"]["total_cents"], 0);

    // A scenario: deputies 1 and 2 yes, 3 no, 4 has no position.
    let (_, room) = call(
        &app,
        "POST",
        "/diogenes/rooms",
        Some(&owner),
        Some(json!({"name": "R"})),
    )
    .await;
    let room_id = id(&room);
    let (_, project) = call(
        &app,
        "POST",
        &format!("/diogenes/rooms/{room_id}/projects"),
        Some(&owner),
        Some(json!({"name": "P"})),
    )
    .await;
    let project_id = id(&project);
    let (_, detail) = call(
        &app,
        "GET",
        &format!("/diogenes/projects/{project_id}"),
        Some(&owner),
        None,
    )
    .await;
    let col = |key: &str| {
        detail["data"]["columns"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["key"] == key)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let (_, sc) = call(
        &app,
        "POST",
        &format!("/diogenes/projects/{project_id}/scenarios"),
        Some(&owner),
        Some(json!({"name": "S"})),
    )
    .await;
    let sc_id = id(&sc);
    let moves = json!({"moves": [
        {"camara_id": 1, "column_id": col("yes")},
        {"camara_id": 2, "column_id": col("yes")},
        {"camara_id": 3, "column_id": col("no")},
    ]});
    call(
        &app,
        "PUT",
        &format!("/diogenes/scenarios/{sc_id}/positions"),
        Some(&owner),
        Some(moves),
    )
    .await;
    let path = format!("/diogenes/scenarios/{sc_id}/donors");
    assert_eq!(
        call(&app, "GET", &path, Some(&stranger), None).await.0,
        StatusCode::NOT_FOUND,
        "a non-member cannot see the room"
    );
    let (s, m) = call(&app, "GET", &path, Some(&owner), None).await;
    assert_eq!(s, StatusCode::OK);
    let m = &m["data"];
    let column = |key: &str| {
        m["columns"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["column_id"] == col(key))
            .unwrap()
            .clone()
    };
    let yes = column("yes");
    assert_eq!(
        (yes["deputies"].as_i64(), yes["with_accounts"].as_i64()),
        (Some(2), Some(2))
    );
    assert_eq!(yes["cents_by_source"]["party"], 20_000_000);
    assert_eq!(yes["cents_by_source"]["individuals"], 105_000);
    let undecided = column("undecided");
    assert_eq!(
        (
            undecided["deputies"].as_i64(),
            undecided["with_accounts"].as_i64()
        ),
        (Some(1), Some(0))
    );
    let shared = m["donors"].as_array().unwrap();
    assert_eq!(shared.len(), 1, "only José gave to two deputies");
    assert_eq!(shared[0]["deputies"], 2);
    assert_eq!(shared[0]["cents"], 15_150_000);
    assert_eq!(shared[0]["by_column"][col("yes")], 1);
    assert_eq!(shared[0]["by_column"][col("no")], 1);

    // A broken archive fails the run and keeps the data of the last good import.
    let (base, url) = start(archive(&[(RECEIPTS_FILE, receipts())])).await;
    let client = CamaraClient::with_base(&base).unwrap();
    run_job(&db, &client, &config(url), Job::Donations)
        .await
        .unwrap();
    let (status, _, error) = last_run(&db).await;
    assert_eq!(status, "failed");
    assert!(error.unwrap().contains("archive"));
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM diogenes_campaign_total")
        .fetch_one(&db)
        .await
        .unwrap();
    assert!(kept > 0);
}

#[tokio::test]
async fn download_refuses_errors_and_large_archives() {
    let (_, url) = start(vec![0_u8; 2048]).await;
    let dir = std::env::temp_dir().join(format!("diogenes-test-{}", Uuid::now_v7()));
    let err = diogenes::finance::download(&url, &dir, 1024)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("larger than"));
    let missing = url.replace("tse.zip", "nope.zip");
    assert!(diogenes::finance::download(&missing, &dir, 1024)
        .await
        .is_err());
    let n = diogenes::finance::download(&url, &dir, 4096).await.unwrap();
    assert_eq!(n, 2048);
    // Bytes that are not a zip archive.
    assert!(diogenes::finance::read_archive(&dir, &std::collections::HashMap::new()).is_err());
    std::fs::remove_file(&dir).unwrap();
    assert!(diogenes::finance::read_archive(&dir, &std::collections::HashMap::new()).is_err());
}
