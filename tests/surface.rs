//! Integration tests: public reference API, sync endpoints, edit and delete rules,
//! the live-update stream and the web pages.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{app, call, db, seed_deputies, User, SERIAL};
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

fn data_id(v: &Value) -> String {
    v["data"].as_str().map_or_else(
        || v["data"]["id"].as_str().unwrap().to_owned(),
        str::to_owned,
    )
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn public_reference_endpoints() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    seed_deputies(&db, 4).await;
    sqlx::raw_sql(
        "INSERT INTO diogenes_group (id, kind, camara_id, title, source, official_total, details)
           VALUES ('front:9', 'front', 9, 'Frente Teste', 'camara', 3, '{\"document_url\": \"https://x\"}');
         INSERT INTO diogenes_group_member (group_id, camara_id, role) VALUES
           ('front:9', 2, 'Coordenador'), ('front:9', 3, NULL), ('front:9', 999, NULL);
         INSERT INTO diogenes_bill (camara_id, kind, number, year, summary) VALUES (77, 'PL', 1, 2026, 'Ementa');
         INSERT INTO diogenes_roll_call (id, voted_on, body, description, bill_id, totals_match)
           VALUES ('77-1', '2026-11-20', 'PLEN', 'Sim: 1; Não: 1', 77, true),
                  ('77-2', '2026-11-21', 'PLEN', 'Simbólica', 77, NULL);
         INSERT INTO diogenes_roll_call_vote (roll_call_id, camara_id, vote, party, uf) VALUES
           ('77-1', 2, 'Sim', 'PT', 'SP'), ('77-1', 3, 'Não', 'PL', 'SP');",
    )
    .execute(&db)
    .await
    .unwrap();
    let app = app(db, &[], vec![]);

    let (s, deps) = call(&app, "GET", "/diogenes/deputies", None, None).await;
    assert_eq!(s, StatusCode::OK);
    let deps = deps["data"].as_array().unwrap();
    assert_eq!(deps.len(), 4);
    let chair = deps.iter().find(|d| d["camara_id"] == 1).unwrap();
    assert_eq!(chair["board_role"], "Presidente");
    // The raw race value is never in the response.
    assert!(chair.get("race").is_none());

    let (_, groups) = call(&app, "GET", "/diogenes/groups", None, None).await;
    let front = groups["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == "front:9")
        .unwrap()
        .clone();
    assert_eq!(front["member_count"], 3);
    assert_eq!(front["in_office_count"], 2);
    let (s, g) = call(&app, "GET", "/diogenes/groups/front:9", None, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(g["data"]["members"].as_array().unwrap().len(), 3);
    assert_eq!(g["data"]["details"]["document_url"], "https://x");
    assert_eq!(
        call(&app, "GET", "/diogenes/groups/front:0", None, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    // The list has only nominal roll calls (with votes).
    let (_, list) = call(
        &app,
        "GET",
        "/diogenes/roll-calls?bill=77&limit=500",
        None,
        None,
    )
    .await;
    let list = list["data"].as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["bill_label"], "PL 1/2026");
    let (_, rc) = call(&app, "GET", "/diogenes/roll-calls/77-1", None, None).await;
    assert_eq!(rc["data"]["bill_summary"], "Ementa");
    assert_eq!(rc["data"]["counts"]["Sim"], 1);
    assert_eq!(
        call(&app, "GET", "/diogenes/roll-calls/nope", None, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn sync_endpoints_need_permission() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    sqlx::query(
        "INSERT INTO diogenes_sync_run (job, status, items, checks, finished_at)
         VALUES ('groups', 'warning', 5, '[{\"subject\":\"front:1\",\"expected\":3,\"actual\":2},
                                          {\"subject\":\"front:2\",\"expected\":4,\"actual\":4}]', now())",
    )
    .execute(&db)
    .await
    .unwrap();
    let user = User::new(Uuid::now_v7(), "Ana", "ana", true);
    let app = app(db, &[&user], vec![]);
    let (s, status) = call(&app, "GET", "/diogenes/sync/status", None, None).await;
    assert_eq!(s, StatusCode::OK);
    let run = &status["data"][0];
    assert_eq!(run["status"], "warning");
    // Only the failed check is in the status.
    assert_eq!(run["failed_checks"].as_array().unwrap().len(), 1);
    assert_eq!(
        call(&app, "POST", "/diogenes/sync/groups", None, Some(json!({})))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/diogenes/sync/groups",
            Some(&user),
            Some(json!({}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn edit_and_delete_rules() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    seed_deputies(&db, 5).await;
    let org = Uuid::now_v7();
    let owner = User::new(org, "Ana", "ana", true);
    let editor = User::new(org, "Bruno", "bruno", false);
    let app = app(db, &[&owner, &editor], vec![]);

    let (_, room) = call(
        &app,
        "POST",
        "/diogenes/rooms",
        Some(&owner),
        Some(json!({"name": "S"})),
    )
    .await;
    let room_id = data_id(&room);
    call(
        &app,
        "POST",
        &format!("/diogenes/rooms/{room_id}/members"),
        Some(&owner),
        Some(json!({"handle": "bruno", "role": "editor"})),
    )
    .await;

    // Rename room: owner only.
    let path = format!("/diogenes/rooms/{room_id}");
    assert_eq!(
        call(
            &app,
            "PATCH",
            &path,
            Some(&editor),
            Some(json!({"name": "X"}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "PATCH",
            &path,
            Some(&owner),
            Some(json!({"name": "Sala 2"}))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "PATCH",
            &path,
            Some(&owner),
            Some(json!({"name": ""}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );

    let (_, project) = call(
        &app,
        "POST",
        &format!("/diogenes/rooms/{room_id}/projects"),
        Some(&editor),
        Some(json!({"name": "P", "threshold": 257})),
    )
    .await;
    let project_id = data_id(&project);
    let ppath = format!("/diogenes/projects/{project_id}");

    // Patch project: threshold limits, bill set and clear.
    assert_eq!(
        call(
            &app,
            "PATCH",
            &ppath,
            Some(&editor),
            Some(json!({"threshold": 600}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &app,
            "PATCH",
            &ppath,
            Some(&editor),
            Some(json!({"threshold": 308, "bill_id": 5}))
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, d) = call(&app, "GET", &ppath, Some(&editor), None).await;
    assert_eq!(
        (
            d["data"]["threshold"].as_i64(),
            d["data"]["bill_id"].as_i64()
        ),
        (Some(308), Some(5))
    );
    call(
        &app,
        "PATCH",
        &ppath,
        Some(&editor),
        Some(json!({"bill_id": null})),
    )
    .await;
    let (_, d) = call(&app, "GET", &ppath, Some(&editor), None).await;
    assert!(d["data"]["bill_id"].is_null());

    // Rename a column; bad colour rejected.
    let col = d["data"]["columns"][3]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        call(
            &app,
            "PATCH",
            &format!("/diogenes/columns/{col}"),
            Some(&editor),
            Some(json!({"name": "Talvez", "color": "red"}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &app,
            "PATCH",
            &format!("/diogenes/columns/{col}"),
            Some(&editor),
            Some(json!({"name": "Talvez"}))
        )
        .await
        .0,
        StatusCode::OK
    );

    // Notes: only the author edits; the owner can delete (moderation).
    let (_, note) = call(
        &app,
        "POST",
        &format!("{ppath}/notes"),
        Some(&editor),
        Some(json!({"camara_id": 2, "body": "a"})),
    )
    .await;
    let npath = format!("/diogenes/notes/{}", data_id(&note));
    assert_eq!(
        call(
            &app,
            "PATCH",
            &npath,
            Some(&owner),
            Some(json!({"body": "b"}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "PATCH",
            &npath,
            Some(&editor),
            Some(json!({"body": "b"}))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "DELETE", &npath, Some(&owner), None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "DELETE", &npath, Some(&owner), None).await.0,
        StatusCode::NOT_FOUND
    );

    // Pasted list.
    let (s, m) = call(
        &app,
        "POST",
        &format!("{ppath}/match-list"),
        Some(&editor),
        Some(json!({"text": "Deputado 002\nNinguém Aqui"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(m["data"]["found"][0][0], 2);
    assert_eq!(m["data"]["missed"], json!(["Ninguém Aqui"]));
    assert_eq!(
        call(
            &app,
            "PUT",
            &format!("{ppath}/priorities"),
            Some(&editor),
            Some(json!({"camara_ids": [], "value": true}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );

    // Scenario rename and delete.
    let (_, sc) = call(
        &app,
        "POST",
        &format!("{ppath}/scenarios"),
        Some(&editor),
        Some(json!({"name": "A"})),
    )
    .await;
    let spath = format!("/diogenes/scenarios/{}", data_id(&sc));
    assert_eq!(
        call(
            &app,
            "PATCH",
            &spath,
            Some(&editor),
            Some(json!({"name": "B"}))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "DELETE", &spath, Some(&editor), None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "GET", &spath, Some(&editor), None).await.0,
        StatusCode::NOT_FOUND
    );
    // Copy from a scenario of another project is rejected.
    let (_, other) = call(
        &app,
        "POST",
        &format!("/diogenes/rooms/{room_id}/projects"),
        Some(&editor),
        Some(json!({"name": "Q"})),
    )
    .await;
    let (_, osc) = call(
        &app,
        "POST",
        &format!("/diogenes/projects/{}/scenarios", data_id(&other)),
        Some(&editor),
        Some(json!({"name": "O"})),
    )
    .await;
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{ppath}/scenarios"),
            Some(&editor),
            Some(json!({"name": "C", "copy_from": data_id(&osc)}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );

    // Live stream: open for a member, hidden for others.
    let req = Request::builder()
        .uri(format!("{ppath}/events"))
        .header("x-test-user", editor.id.to_string())
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()["content-type"], "text/event-stream");
    assert_eq!(
        call(&app, "GET", &format!("{ppath}/events"), None, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );

    // Delete project: owner only. Then delete room.
    assert_eq!(
        call(&app, "DELETE", &ppath, Some(&editor), None).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, "DELETE", &ppath, Some(&owner), None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "DELETE", &path, Some(&editor), None).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, "DELETE", &path, Some(&owner), None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "GET", &path, Some(&owner), None).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn web_pages_have_security_headers() {
    let web = diogenes::web_router();
    let get = |uri: &'static str| {
        let web = web.clone();
        async move {
            web.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap()
        }
    };
    let index = get("/articulacao/").await;
    assert_eq!(index.status(), StatusCode::OK);
    let csp = index.headers()["content-security-policy"].to_str().unwrap();
    assert!(csp.contains("script-src 'self'") && csp.contains("frame-ancestors 'none'"));
    assert_eq!(index.headers()["x-content-type-options"], "nosniff");
    let redirect = get("/articulacao").await;
    assert_eq!(redirect.status(), StatusCode::PERMANENT_REDIRECT);
    assert!(get("/articulacao/app.js").await.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/javascript"));
    assert!(get("/articulacao/app.css").await.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/css"));
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn api_hardening() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    let user = User::new(Uuid::now_v7(), "Ana", "ana", true);
    let app = app(db, &[&user], vec![]);

    // Every API response forbids caching.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/diogenes/groups")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.headers()["cache-control"], "no-store");

    // A body above the limit is rejected.
    let big = "x".repeat(diogenes::MAX_BODY_BYTES + 1);
    let (s, _) = call(
        &app,
        "POST",
        "/diogenes/rooms",
        Some(&user),
        Some(json!({ "name": big })),
    )
    .await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);

    // copy_from and roll_call together are rejected.
    let (_, room) = call(
        &app,
        "POST",
        "/diogenes/rooms",
        Some(&user),
        Some(json!({"name": "S"})),
    )
    .await;
    let (_, project) = call(
        &app,
        "POST",
        &format!("/diogenes/rooms/{}/projects", data_id(&room)),
        Some(&user),
        Some(json!({"name": "P"})),
    )
    .await;
    let (s, _) = call(
        &app,
        "POST",
        &format!("/diogenes/projects/{}/scenarios", data_id(&project)),
        Some(&user),
        Some(json!({"name": "X", "copy_from": Uuid::now_v7(), "roll_call": "1-1"})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // The sync trigger needs a JSON body.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/diogenes/sync/groups")
                .header("x-test-user", user.id.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}
