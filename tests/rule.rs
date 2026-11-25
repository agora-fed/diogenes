//! Integration tests: the approval rule of a project (fixed threshold or simple majority).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::http::StatusCode;
use common::{app, call, db, seed_deputies, User, SERIAL};
use serde_json::{json, Value};
use uuid::Uuid;

fn id(v: &Value) -> String {
    v["data"].as_str().map_or_else(
        || v["data"]["id"].as_str().unwrap().to_owned(),
        str::to_owned,
    )
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn bill_kind_sets_the_rule_and_simple_majority_counts() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    seed_deputies(&db, 10).await;
    sqlx::raw_sql(
        "INSERT INTO diogenes_bill (camara_id, kind, number, year) VALUES
           (1, 'PEC', 1, 2025), (2, 'PLP', 2, 2025), (3, 'PL', 3, 2025)",
    )
    .execute(&db)
    .await
    .unwrap();
    let org = Uuid::now_v7();
    let owner = User::new(org, "Ana", "ana", true);
    let app = app(db.clone(), &[&owner], vec![]);
    let (_, room) = call(
        &app,
        "POST",
        "/diogenes/rooms",
        Some(&owner),
        Some(json!({"name": "Regras"})),
    )
    .await;
    let room_id = id(&room);
    let new_project = |body: Value| {
        let app = app.clone();
        let owner = &owner;
        let room_id = room_id.clone();
        async move {
            let (s, p) = call(
                &app,
                "POST",
                &format!("/diogenes/rooms/{room_id}/projects"),
                Some(owner),
                Some(body),
            )
            .await;
            assert_eq!(s, StatusCode::OK);
            let (_, d) = call(
                &app,
                "GET",
                &format!("/diogenes/projects/{}", id(&p)),
                Some(owner),
                None,
            )
            .await;
            (
                id(&p),
                d["data"]["rule"].as_str().unwrap().to_owned(),
                d["data"]["threshold"].as_i64().unwrap(),
            )
        }
    };

    // The kind of the bill picks the rule when the caller sends nothing.
    let (_, rule, t) = new_project(json!({"name": "PEC", "bill_id": 1})).await;
    assert_eq!((rule.as_str(), t), ("fixed", 308));
    let (_, rule, t) = new_project(json!({"name": "PLP", "bill_id": 2})).await;
    assert_eq!((rule.as_str(), t), ("fixed", 257));
    let (pl, rule, t) = new_project(json!({"name": "PL", "bill_id": 3})).await;
    assert_eq!((rule.as_str(), t), ("simple", 257));
    // No bill, or an unknown bill: fixed 257, as before.
    let (_, rule, t) = new_project(json!({"name": "Sem", "bill_id": 999})).await;
    assert_eq!((rule.as_str(), t), ("fixed", 257));
    // A value that the caller sends wins.
    let (_, rule, t) = new_project(json!({"name": "Fixo", "bill_id": 3, "threshold": 6})).await;
    assert_eq!((rule.as_str(), t), ("fixed", 6));
    let (_, rule, t) =
        new_project(json!({"name": "Simples", "bill_id": 1, "rule": "simple"})).await;
    assert_eq!((rule.as_str(), t), ("simple", 257));
    let (_, rule, t) = new_project(json!({"name": "PEC fixo", "rule": "fixed"})).await;
    assert_eq!((rule.as_str(), t), ("fixed", 257));

    // The room list shows the rule.
    let (_, r) = call(
        &app,
        "GET",
        &format!("/diogenes/rooms/{room_id}"),
        Some(&owner),
        None,
    )
    .await;
    let projects = r["data"]["projects"].as_array().unwrap();
    assert_eq!(projects[2]["rule"], "simple");

    // Simple majority with a quorum of 4 in a house of 10: 3 × 1 with the chair.
    call(
        &app,
        "PATCH",
        &format!("/diogenes/projects/{pl}"),
        Some(&owner),
        Some(json!({"threshold": 4})),
    )
    .await;
    let (_, d) = call(
        &app,
        "GET",
        &format!("/diogenes/projects/{pl}"),
        Some(&owner),
        None,
    )
    .await;
    let col = |key: &str| {
        d["data"]["columns"]
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
        &format!("/diogenes/projects/{pl}/scenarios"),
        Some(&owner),
        Some(json!({"name": "Base"})),
    )
    .await;
    let sc_path = format!("/diogenes/scenarios/{}", id(&sc));
    let mut moves: Vec<Value> = (2..=4)
        .map(|d| json!({"camara_id": d, "column_id": col("yes")}))
        .collect();
    moves.push(json!({"camara_id": 5, "column_id": col("no")}));
    moves.extend((6..=10).map(|d| json!({"camara_id": d, "column_id": col("absent")})));
    let (s, _) = call(
        &app,
        "PUT",
        &format!("{sc_path}/positions"),
        Some(&owner),
        Some(json!({"moves": moves})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, view) = call(&app, "GET", &sc_path, Some(&owner), None).await;
    assert_eq!(view["data"]["rule"], "simple");
    assert_eq!(view["data"]["outcome"]["state"], "approved");
    assert_eq!(view["data"]["outcome"]["margin"], 2);

    // The same board under the fixed rule: 3 of 4 is open.
    let (s, _) = call(
        &app,
        "PATCH",
        &format!("/diogenes/projects/{pl}"),
        Some(&owner),
        Some(json!({"rule": "fixed"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, view) = call(&app, "GET", &sc_path, Some(&owner), None).await;
    assert_eq!(view["data"]["outcome"]["state"], "open");
    assert_eq!(view["data"]["outcome"]["missing"], 1);

    // An unknown rule is a bad request.
    let (s, _) = call(
        &app,
        "PATCH",
        &format!("/diogenes/projects/{pl}"),
        Some(&owner),
        Some(json!({"rule": "two_thirds"})),
    )
    .await;
    assert!(s.is_client_error());
}
