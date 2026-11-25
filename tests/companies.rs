//! Integration test: the `companies` job against a fake Câmara API, a fake TSE archive and
//! a fake Receita share, then the company data in the finance API (ADR-0010).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::io::{Cursor, Write};

use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Router;
use common::{app, call, db, seed_deputies, User, SERIAL};
use diogenes::camara::CamaraClient;
use diogenes::companies::parse::each_row;
use diogenes::companies::{months, CnpjSource};
use diogenes::finance::parse::{ORIGINAL_DONORS_FILE, RECEIPTS_FILE};
use diogenes::sync::{run_job, Job, SyncConfig};
use serde_json::{json, Value};
use uuid::Uuid;

const SHARE: &str = "token123";

fn l1(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| u8::try_from(u32::from(c)).unwrap())
        .collect()
}

fn zip_of(name: &str, body: &[u8], method: zip::CompressionMethod) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(&mut out);
    let options = zip::write::SimpleFileOptions::default().compression_method(method);
    writer.start_file(name, options).unwrap();
    writer.write_all(body).unwrap();
    writer.finish().unwrap();
    out.into_inner()
}

fn deflated(name: &str, csv: &str) -> Vec<u8> {
    zip_of(name, &l1(csv), zip::CompressionMethod::Deflated)
}

fn tse_archive() -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(&mut out);
    let options = zip::write::SimpleFileOptions::default();
    writer.start_file(RECEIPTS_FILE, options).unwrap();
    writer.write_all(&l1("DS_CARGO;NR_CPF_CANDIDATO;SQ_RECEITA;DS_ORIGEM_RECEITA;NR_CPF_CNPJ_DOADOR;NM_DOADOR;DS_CNAE_DOADOR;SG_UF_DOADOR;VR_RECEITA\n\
Deputado Federal;11111111111;10;Recursos de pessoas físicas;12377777701;JOSÉ  DOADOR;#NULO;SP;1000,00\n\
Deputado Federal;33333333333;11;Recursos de pessoas físicas;12377777701;JOSÉ  DOADOR;#NULO;SP;500,00\n\
Deputado Federal;22222222222;13;Recursos de pessoas físicas;98788888801;ANA;#NULO#;#NULO#;50,00\n\
Deputado Federal;22222222222;14;Recursos de pessoas físicas;55566666601;JOAO HOMONIMO;#NULO;RJ;20,00\n\
Deputado Federal;22222222222;15;Recursos de pessoas físicas;44466666601;JOAO HOMONIMO;#NULO;RJ;30,00\n")).unwrap();
    writer.start_file(ORIGINAL_DONORS_FILE, options).unwrap();
    writer.write_all(&l1("SQ_RECEITA;NR_CPF_CNPJ_DOADOR_ORIGINARIO;NM_DOADOR_ORIGINARIO;TP_DOADOR_ORIGINARIO;DS_CNAE_DOADOR_ORIGINARIO;VR_RECEITA\n")).unwrap();
    writer.finish().unwrap();
    out.into_inner()
}

/// The fake Receita files. José (***777777**) is a partner of 11111111; the deputy 1
/// (civil name "PESSOA UM", CPF 11111111111) of 22222222; Ana's digits differ.
fn receita(file: &str) -> Option<Vec<u8>> {
    let empty = |name: &str| deflated(name, "");
    Some(match file {
        "Socios0.zip" => deflated(
            "K.SOCIOCSV",
            "\"11111111\";\"2\";\"JOSE DOADOR\";\"***777777**\";\"49\";\"20100105\";\"\";\"***000000**\";\"\";\"00\";\"6\"\n\
\"55555555\";\"2\";\"JOSE DOADOR\";\"***123456**\";\"22\";\"0\";\"\";\"\";\"\";\"00\";\"0\"\n\
\"11111111\";\"2\";\"ANA\";\"***888889**\";\"22\";\"20100105\";\"\";\"***000000**\";\"\";\"00\";\"6\"\n\
\"22222222\";\"2\";\"PESSOA UM\";\"***111111**\";\"22\";\"0\";\"\";\"***000000**\";\"\";\"00\";\"5\"\n\
\"33333333\";\"1\";\"HOLDING SA\";\"11111111000191\";\"22\";\"20000101\";\"\";\"\";\"\";\"00\";\"0\"\n\
\"44444444\";\"2\";\"JOAO HOMONIMO\";\"***666666**\";\"49\";\"20000101\";\"\";\"\";\"\";\"00\";\"0\"\n",
        ),
        "Empresas0.zip" => deflated(
            "K.EMPRECSV",
            "\"11111111\";\"SOJA GRANDE SA\";\"2046\";\"10\";\"5000000,00\";\"05\";\"\"\n\
\"22222222\";\"CONSULTORIA UM LTDA\";\"2062\";\"49\";\"1000,00\";\"01\";\"\"\n\
\"33333333\";\"OUTRA\";\"2062\";\"49\";\"0,00\";\"01\";\"\"\n",
        ),
        "Estabelecimentos0.zip" => {
            let row = |cnpj: &str, kind: &str, cnae: &str| {
                let mut f = vec![String::new(); 30];
                f[0] = cnpj.into();
                f[1] = "0001".into();
                f[3] = kind.into();
                f[4] = "SOJAO".into();
                f[5] = "02".into();
                f[10] = "19900101".into();
                f[11] = cnae.into();
                f[19] = "MT".into();
                f[20] = "9067".into();
                f.iter().map(|v| format!("\"{v}\"")).collect::<Vec<_>>().join(";")
            };
            deflated(
                "K.ESTABELE",
                &format!(
                    "{}\n{}\n",
                    row("11111111", "1", "0115600"),
                    row("11111111", "2", "4623199")
                ),
            )
        }
        "Qualificacoes.zip" => deflated("Q", "\"49\";\"Sócio-Administrador\"\n\"22\";\"Sócio\"\n"),
        "Naturezas.zip" => deflated("N", "\"2046\";\"Sociedade Anônima Aberta\"\n"),
        "Cnaes.zip" => deflated("C", "\"0115600\";\"Cultivo de soja\"\n"),
        "Municipios.zip" => deflated("M", "\"9067\";\"SORRISO\"\n"),
        f if f.starts_with("Socios") || f.starts_with("Empresas") || f.starts_with("Estabelecimentos") => {
            empty("X")
        }
        _ => return None,
    })
}

const LISTING: &str = r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:">
<d:response><d:href>/share/</d:href></d:response>
<d:response><d:href>/share/2026-10/</d:href></d:response>
<d:response><d:href>/share/2026-11/</d:href></d:response>
<d:response><d:href>/share/cnpj.tar.gz</d:href></d:response></d:multistatus>"#;

/// The fake servers. `/share/` needs the share token as the user name (basic auth).
async fn start(broken: Option<&'static str>) -> (String, String, String) {
    let app = Router::new().fallback(move |req: Request| async move {
        let path = req.uri().path().to_owned();
        if path == "/tse.zip" {
            return tse_archive().into_response();
        }
        if let Some(rest) = path.strip_prefix("/share/") {
            let auth = req
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            // "token123:" in base64.
            if auth != "Basic dG9rZW4xMjM6" {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            if rest.is_empty() && req.method().as_str() == "PROPFIND" {
                return (StatusCode::MULTI_STATUS, LISTING).into_response();
            }
            let file = rest.strip_prefix("2026-11/").unwrap_or_default();
            if Some(file) == broken {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            return receita(file).map_or_else(
                || StatusCode::NOT_FOUND.into_response(),
                IntoResponse::into_response,
            );
        }
        let Some(id) = path.strip_prefix("/api/v2/deputados/") else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let id: u32 = id.parse().unwrap();
        let civil = if id == 1 { "Pessoa Um" } else { "Outra Pessoa" };
        axum::Json(
            json!({"dados": {"id": id, "cpf": id.to_string().repeat(11), "nomeCivil": civil}}),
        )
        .into_response()
    });
    let listener = tokio::net::TcpListener::bind("[::1]:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (
        format!("http://{addr}/api/v2"),
        format!("http://{addr}/tse.zip"),
        format!("http://{addr}/share/"),
    )
}

fn config(tse: String, share: String, month: Option<&str>) -> SyncConfig {
    SyncConfig {
        roll_calls_since: chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
        roll_calls_every: std::time::Duration::from_secs(3600),
        reference_every: std::time::Duration::from_secs(3600),
        tse_enabled: false,
        finance_url: tse,
        cnpj: CnpjSource {
            base: share,
            share: SHARE.into(),
            month: month.map(str::to_owned),
        },
    }
}

async fn last_run(db: &sqlx::PgPool) -> (String, i32, Value, Option<String>) {
    sqlx::query_as(
        "SELECT status, items, checks, error FROM diogenes_sync_run
          WHERE job = 'companies' ORDER BY id DESC LIMIT 1",
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

fn check(checks: &Value, subject: &str) -> (i64, i64) {
    let c = checks
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["subject"] == subject)
        .unwrap();
    (
        c["expected"].as_i64().unwrap(),
        c["actual"].as_i64().unwrap(),
    )
}

async fn count(db: &sqlx::PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(db)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn companies_of_donors_and_deputies() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    seed_deputies(&db, 3).await;
    let (base, tse, share) = start(None).await;
    let client = CamaraClient::with_base(&base).unwrap();
    let cfg = config(tse.clone(), share.clone(), None);
    run_job(&db, &client, &cfg, Job::Donations)
        .await
        .unwrap()
        .unwrap();
    // The donor keeps the six middle CPF digits, and no full CPF.
    let parts: Vec<String> =
        sqlx::query_scalar("SELECT cpf_part FROM diogenes_donor WHERE name LIKE 'JOS%'")
            .fetch_all(&db)
            .await
            .unwrap();
    assert_eq!(parts, ["777777"]);
    // The TSE writes an empty value as `#NULO#` too.
    let ana: (Option<String>, Option<String>) =
        sqlx::query_as("SELECT uf, activity FROM diogenes_donor WHERE name = 'ANA'")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(ana, (None, None));

    run_job(&db, &client, &cfg, Job::Companies)
        .await
        .unwrap()
        .unwrap();
    let (status, items, checks, error) = last_run(&db).await;
    assert_eq!(error, None);
    // 22222222 has no head office in the file, and two donors share a name and digits.
    assert_eq!(status, "warning");
    assert_eq!(items, 2, "José → 11111111 and deputy 1 → 22222222");
    assert_eq!(check(&checks, "files_read"), (34, 34));
    assert_eq!(check(&checks, "companies_with_name"), (2, 2));
    assert_eq!(check(&checks, "companies_with_head_office"), (2, 1));
    assert_eq!(check(&checks, "ambiguous_donors"), (0, 1));
    assert_eq!(check(&checks, "rejected_rows"), (0, 0));
    let version: String =
        sqlx::query_scalar("SELECT version FROM diogenes_dataset WHERE name = 'rfb_cnpj'")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(version, "2026-11", "the last month of the share");

    // The public API.
    let org = Uuid::now_v7();
    let owner = User::new(org, "Ana", "ana", true);
    let app = app(db.clone(), &[&owner], vec![]);
    let (s, c) = call(&app, "GET", "/diogenes/deputies/1/campaign", None, None).await;
    assert_eq!(s, StatusCode::OK);
    let c = &c["data"];
    assert_eq!(c["companies_source"]["version"], "2026-11");
    assert!(c["companies_source"]["source_url"]
        .as_str()
        .unwrap()
        .contains("dados.gov.br"));
    let jose = &c["donors"][0];
    assert_eq!(jose["companies_total"], 1);
    let company = &jose["companies"][0];
    assert_eq!(company["name"], "SOJA GRANDE SA");
    assert_eq!(company["cnae_text"], "Cultivo de soja");
    assert_eq!(company["role"], "Sócio-Administrador");
    assert_eq!(company["status"], "active");
    assert_eq!(company["city"], "SORRISO");
    assert_eq!(company["since"], "2010-01-05");
    assert_eq!(company["capital_cents"], 500_000_000);
    assert_eq!(
        company["namesakes"], 1,
        "one other JOSE DOADOR in the partner file"
    );
    assert!(
        jose.get("cpf_part").is_none(),
        "the API never sends the CPF digits"
    );
    let own = c["deputy_companies"].as_array().unwrap();
    assert_eq!(own.len(), 1);
    assert_eq!(
        (
            own[0]["name"].as_str(),
            own[0]["role"].as_str(),
            own[0]["status"].as_str()
        ),
        (Some("CONSULTORIA UM LTDA"), Some("Sócio"), Some("unknown"))
    );
    assert_eq!(own[0]["namesakes"], 0);
    let (s, list) = call(&app, "GET", "/diogenes/campaigns", None, None).await;
    assert_eq!(s, StatusCode::OK);
    let rows = list["data"]["deputies"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    let one = rows.iter().find(|r| r["camara_id"] == 1).unwrap();
    assert_eq!(
        (
            one["donors_with_companies"].as_i64(),
            one["own_companies"].as_i64(),
            one["total_cents"].as_i64()
        ),
        (Some(1), Some(1), Some(100_000))
    );
    assert_eq!(one["people_cents"], 100_000);
    assert_eq!(rows[0]["camara_id"], 1, "the largest campaign first");
    assert_eq!(list["data"]["companies"]["version"], "2026-11");

    // A scenario: the sectors of the companies of the donors, by column.
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
    let yes = detail["data"]["columns"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["key"] == "yes")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (_, sc) = call(
        &app,
        "POST",
        &format!("/diogenes/projects/{project_id}/scenarios"),
        Some(&owner),
        Some(json!({"name": "S"})),
    )
    .await;
    let sc_id = id(&sc);
    call(
        &app,
        "PUT",
        &format!("/diogenes/scenarios/{sc_id}/positions"),
        Some(&owner),
        Some(json!({"moves": [{"camara_id": 1, "column_id": yes}, {"camara_id": 3, "column_id": yes}]})),
    )
    .await;
    let (s, m) = call(
        &app,
        "GET",
        &format!("/diogenes/scenarios/{sc_id}/donors"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let sectors = m["data"]["sectors"].as_array().unwrap();
    assert_eq!(sectors.len(), 1);
    assert_eq!(sectors[0]["cnae_text"], "Cultivo de soja");
    assert_eq!(sectors[0]["donors"], 1);
    assert_eq!(sectors[0]["cents"], 150_000);
    assert_eq!(sectors[0]["by_column"][&yes], 2);
    assert_eq!(
        m["data"]["donors"][0]["companies"][0]["name"],
        "SOJA GRANDE SA"
    );
    assert_eq!(m["data"]["companies_source"]["version"], "2026-11");

    // A new donations import gives new keys: the donor links go, the deputy links stay.
    run_job(&db, &client, &cfg, Job::Donations)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(count(&db, "diogenes_donor_company").await, 0);
    assert_eq!(count(&db, "diogenes_deputy_company").await, 1);

    // A part that fails: the run fails and the stored rows stay.
    let (base, tse, share) = start(Some("Empresas3.zip")).await;
    let client = CamaraClient::with_base(&base).unwrap();
    run_job(
        &db,
        &client,
        &config(tse, share, Some("2026-11")),
        Job::Companies,
    )
    .await
    .unwrap()
    .unwrap();
    let (status, _, _, error) = last_run(&db).await;
    assert_eq!(status, "failed");
    assert!(error.unwrap().contains("500"));
    assert_eq!(count(&db, "diogenes_company").await, 2);
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn a_wrong_share_token_fails_the_run() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    seed_deputies(&db, 1).await;
    let (base, tse, share) = start(None).await;
    let client = CamaraClient::with_base(&base).unwrap();
    let mut cfg = config(tse, share, None);
    cfg.cnpj.share = "wrong".into();
    run_job(&db, &client, &cfg, Job::Companies)
        .await
        .unwrap()
        .unwrap();
    let (status, _, _, error) = last_run(&db).await;
    assert_eq!(status, "failed");
    assert!(error.unwrap().contains("401"));
}

#[test]
fn months_come_from_the_listing_last_first() {
    assert_eq!(months(LISTING), ["2026-11", "2026-10"]);
    assert!(months("<d:href>/share/</d:href>").is_empty());
}

#[test]
fn each_row_reads_stored_and_deflated_entries() {
    let csv = b"\"1\";\"S\xc3O PAULO\"\n\"2\";\"B\"\n";
    for method in [
        zip::CompressionMethod::Stored,
        zip::CompressionMethod::Deflated,
    ] {
        let mut rows = Vec::new();
        let counts = each_row(Cursor::new(zip_of("x", csv, method)), |r| {
            rows.push(r.to_vec())
        })
        .unwrap();
        assert_eq!(counts.rows, 2);
        assert_eq!(rows[0][1], "SÃO PAULO", "latin-1");
    }
}

#[test]
fn each_row_refuses_what_it_cannot_read() {
    let err = |bytes: Vec<u8>| {
        each_row(Cursor::new(bytes), |_| {})
            .unwrap_err()
            .to_string()
    };
    assert!(err(b"short".to_vec()).contains("no local header"));
    assert!(err(vec![0u8; 64]).contains("not a zip file"));
    let mut z = zip_of("x", b"a;b\n", zip::CompressionMethod::Stored);
    z[6] |= 1;
    assert!(err(z.clone()).contains("encrypted"));
    z[6] &= !1;
    z[8] = 12;
    assert!(err(z).contains("method 12"));
    // A deflated entry with broken data: a read error, not a silent end.
    let mut d = zip_of("x", &[b'a'; 4096], zip::CompressionMethod::Deflated);
    let start = 30 + 1;
    for b in &mut d[start..start + 8] {
        *b = 0xff;
    }
    assert!(err(d).contains("read"));
}
