//! Integration tests: rooms, access control, scenarios and the scoreboard.

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
async fn full_room_flow_and_access_rules() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    seed_deputies(&db, 10).await;
    let org = Uuid::now_v7();
    let other_org = Uuid::now_v7();
    let owner = User::new(org, "Ana", "ana", true);
    let editor = User::new(org, "Bruno", "bruno", false);
    let viewer = User::new(org, "Carla", "carla", false);
    let stranger = User::new(org, "Davi", "davi", false);
    let foreign = User::new(other_org, "Eva", "eva", true);
    let app = app(
        db.clone(),
        &[&owner, &editor, &viewer, &stranger, &foreign],
        vec![],
    );

    // Signed out: 401. No permission to create: 403.
    assert_eq!(
        call(&app, "GET", "/diogenes/rooms", None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (s, _) = call(
        &app,
        "POST",
        "/diogenes/rooms",
        Some(&editor),
        Some(json!({"name": "X"})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let (s, room) = call(
        &app,
        "POST",
        "/diogenes/rooms",
        Some(&owner),
        Some(json!({"name": "  Sala PL 1  "})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(room["data"]["name"], "Sala PL 1");
    let room_id = id(&room);

    for (user, role) in [(&editor, "editor"), (&viewer, "viewer")] {
        let (s, _) = call(
            &app,
            "POST",
            &format!("/diogenes/rooms/{room_id}/members"),
            Some(&owner),
            Some(json!({"handle": format!("@{}", user.handle), "role": role})),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
    }
    // Unknown handle, and a handle from another org: both rejected.
    let (s, _) = call(
        &app,
        "POST",
        &format!("/diogenes/rooms/{room_id}/members"),
        Some(&owner),
        Some(json!({"handle": "eva", "role": "viewer"})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // A non-member and a user of another org cannot see the room (404, not 403).
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/diogenes/rooms/{room_id}"),
            Some(&stranger),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/diogenes/rooms/{room_id}"),
            Some(&foreign),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let (_, mine) = call(&app, "GET", "/diogenes/rooms", Some(&foreign), None).await;
    assert_eq!(mine["data"]["rooms"].as_array().unwrap().len(), 0);

    // A viewer cannot make a project.
    let (s, _) = call(
        &app,
        "POST",
        &format!("/diogenes/rooms/{room_id}/projects"),
        Some(&viewer),
        Some(json!({"name": "P"})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, project) = call(
        &app,
        "POST",
        &format!("/diogenes/rooms/{room_id}/projects"),
        Some(&editor),
        Some(json!({"name": "PEC 1", "threshold": 6})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let project_id = id(&project);

    let (_, detail) = call(
        &app,
        "GET",
        &format!("/diogenes/projects/{project_id}"),
        Some(&viewer),
        None,
    )
    .await;
    let columns = detail["data"]["columns"].as_array().unwrap().clone();
    assert_eq!(columns.len(), 6);
    let col = |key: &str| {
        columns.iter().find(|c| c["key"] == key).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    // A blank scenario puts the chair (deputy 1) in the "chair" column.
    let (s, scenario) = call(
        &app,
        "POST",
        &format!("/diogenes/projects/{project_id}/scenarios"),
        Some(&editor),
        Some(json!({"name": "Base"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let scenario_id = id(&scenario);
    let (_, sc) = call(
        &app,
        "GET",
        &format!("/diogenes/scenarios/{scenario_id}"),
        Some(&viewer),
        None,
    )
    .await;
    assert_eq!(sc["data"]["counts"]["chair"], 1);
    assert_eq!(sc["data"]["counts"]["undecided"], 9);
    assert_eq!(sc["data"]["outcome"]["state"], "open");

    // Moves: 6 yes reach the threshold of 6.
    let moves: Vec<Value> = (2..=7)
        .map(|d| json!({"camara_id": d, "column_id": col("yes")}))
        .collect();
    let (s, _) = call(
        &app,
        "PUT",
        &format!("/diogenes/scenarios/{scenario_id}/positions"),
        Some(&viewer),
        Some(json!({"moves": moves.clone()})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "a viewer cannot move");
    let (s, _) = call(
        &app,
        "PUT",
        &format!("/diogenes/scenarios/{scenario_id}/positions"),
        Some(&editor),
        Some(json!({"moves": moves})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, sc) = call(
        &app,
        "GET",
        &format!("/diogenes/scenarios/{scenario_id}"),
        Some(&viewer),
        None,
    )
    .await;
    assert_eq!(sc["data"]["counts"]["yes"], 6);
    assert_eq!(
        sc["data"]["outcome"],
        json!({"state": "approved", "margin": 0})
    );

    // A column of another project is rejected.
    let (s, _) = call(
        &app,
        "PUT",
        &format!("/diogenes/scenarios/{scenario_id}/positions"),
        Some(&editor),
        Some(json!({"moves": [{"camara_id": 8, "column_id": Uuid::now_v7()}]})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Custom column; delete it: its deputies become undecided.
    let (_, custom) = call(
        &app,
        "POST",
        &format!("/diogenes/projects/{project_id}/columns"),
        Some(&editor),
        Some(json!({"name": "Indeciso PSD", "color": "#123456"})),
    )
    .await;
    let custom_id = custom["data"]["id"].as_str().unwrap().to_owned();
    call(
        &app,
        "PUT",
        &format!("/diogenes/scenarios/{scenario_id}/positions"),
        Some(&editor),
        Some(json!({"moves": [{"camara_id": 2, "column_id": custom_id}]})),
    )
    .await;
    let (s, _) = call(
        &app,
        "DELETE",
        &format!("/diogenes/columns/{}", col("yes")),
        Some(&editor),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "fixed columns stay");
    call(
        &app,
        "DELETE",
        &format!("/diogenes/columns/{custom_id}"),
        Some(&editor),
        None,
    )
    .await;
    let (_, sc) = call(
        &app,
        "GET",
        &format!("/diogenes/scenarios/{scenario_id}"),
        Some(&viewer),
        None,
    )
    .await;
    assert_eq!(sc["data"]["counts"]["yes"], 5);
    assert_eq!(sc["data"]["counts"]["undecided"], 4);

    // Copy a scenario.
    let (_, copy) = call(
        &app,
        "POST",
        &format!("/diogenes/projects/{project_id}/scenarios"),
        Some(&editor),
        Some(json!({"name": "Cópia", "copy_from": scenario_id})),
    )
    .await;
    let (_, sc2) = call(
        &app,
        "GET",
        &format!("/diogenes/scenarios/{}", id(&copy)),
        Some(&viewer),
        None,
    )
    .await;
    assert_eq!(sc2["data"]["counts"]["yes"], 5);

    // Notes: a private note is invisible to other members.
    call(
        &app,
        "POST",
        &format!("/diogenes/projects/{project_id}/notes"),
        Some(&editor),
        Some(json!({"camara_id": 3, "body": "ligar amanhã", "private": true})),
    )
    .await;
    call(
        &app,
        "POST",
        &format!("/diogenes/projects/{project_id}/notes"),
        Some(&editor),
        Some(json!({"camara_id": 3, "body": "apoio confirmado"})),
    )
    .await;
    let (_, as_viewer) = call(
        &app,
        "GET",
        &format!("/diogenes/projects/{project_id}"),
        Some(&viewer),
        None,
    )
    .await;
    let (_, as_editor) = call(
        &app,
        "GET",
        &format!("/diogenes/projects/{project_id}"),
        Some(&editor),
        None,
    )
    .await;
    assert_eq!(as_viewer["data"]["notes"].as_array().unwrap().len(), 1);
    assert_eq!(as_editor["data"]["notes"].as_array().unwrap().len(), 2);

    // Priorities.
    let (s, _) = call(
        &app,
        "PUT",
        &format!("/diogenes/projects/{project_id}/priorities"),
        Some(&editor),
        Some(json!({"camara_ids": [4, 5], "value": true})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, d) = call(
        &app,
        "GET",
        &format!("/diogenes/projects/{project_id}"),
        Some(&viewer),
        None,
    )
    .await;
    assert_eq!(d["data"]["priorities"], json!([4, 5]));

    // Audit: every change is there, and the private note text is not.
    let (_, log) = call(
        &app,
        "GET",
        &format!("/diogenes/rooms/{room_id}/audit?limit=200"),
        Some(&viewer),
        None,
    )
    .await;
    let log_text = log.to_string();
    assert!(log_text.contains("position.move"));
    assert!(log_text.contains("note.create"));
    assert!(!log_text.contains("ligar amanhã"));
    // A private note leaves no trace of its deputy in the audit table.
    let private_rows: Vec<&Value> = log["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["action"] == "note.create" && r["payload"]["visibility"] == "author")
        .collect();
    assert_eq!(private_rows.len(), 1);
    assert!(private_rows[0]["payload"].get("camara_id").is_none());

    // The last owner cannot leave.
    let (s, _) = call(
        &app,
        "DELETE",
        &format!("/diogenes/rooms/{room_id}/members/{}", owner.id),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // A member can leave.
    let (s, _) = call(
        &app,
        "DELETE",
        &format!("/diogenes/rooms/{room_id}/members/{}", viewer.id),
        Some(&viewer),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/diogenes/rooms/{room_id}"),
            Some(&viewer),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn module_off_hides_rooms() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    let org = Uuid::now_v7();
    let user = User::new(org, "Ana", "ana", true);
    let app = app(db, &[&user], vec![org]);
    let (s, body) = call(&app, "GET", "/diogenes/rooms", Some(&user), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "module_disabled");
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn scenario_from_roll_call_maps_votes_and_dissidents() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    seed_deputies(&db, 8).await;
    sqlx::raw_sql(
        "INSERT INTO diogenes_roll_call (id, voted_on, body, description) VALUES ('1-1', '2026-11-23', 'PLEN', 'Sim: 3; Não: 1');
         INSERT INTO diogenes_roll_call_vote (roll_call_id, camara_id, vote, party, uf) VALUES
           ('1-1', 1, 'Artigo 17', 'PL', 'SP'), ('1-1', 2, 'Sim', 'PT', 'SP'), ('1-1', 3, 'Sim', 'PL', 'SP'),
           ('1-1', 4, 'Não', 'PT', 'SP'), ('1-1', 5, 'Sim', 'PL', 'SP'), ('1-1', 6, 'Abstenção', 'PT', 'SP'),
           ('1-1', 7, 'Obstrução', 'PL', 'SP');
         INSERT INTO diogenes_roll_call_orientation (roll_call_id, bloc, orientation) VALUES
           ('1-1', 'PT', 'Sim'), ('1-1', 'PL', 'Não');",
    )
    .execute(&db)
    .await
    .unwrap();
    let org = Uuid::now_v7();
    let user = User::new(org, "Ana", "ana", true);
    let app = app(db, &[&user], vec![]);

    // Public read: the roll call and its dissidents.
    let (s, rc) = call(&app, "GET", "/diogenes/roll-calls/1-1", None, None).await;
    assert_eq!(s, StatusCode::OK);
    let mut dissidents: Vec<i64> = rc["data"]["dissidents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect();
    dissidents.sort_unstable();
    // PT oriented Sim: deputy 4 voted Não. PL oriented Não: deputies 3 and 5 voted Sim.
    assert_eq!(dissidents, vec![3, 4, 5]);

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
        &format!("/diogenes/rooms/{}/projects", id(&room)),
        Some(&user),
        Some(json!({"name": "P"})),
    )
    .await;
    let (s, scenario) = call(
        &app,
        "POST",
        &format!("/diogenes/projects/{}/scenarios", id(&project)),
        Some(&user),
        Some(json!({"name": "Votação real", "roll_call": "1-1"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{scenario}");
    let (_, sc) = call(
        &app,
        "GET",
        &format!("/diogenes/scenarios/{}", id(&scenario)),
        Some(&user),
        None,
    )
    .await;
    let c = &sc["data"]["counts"];
    assert_eq!(
        (
            c["yes"].as_i64(),
            c["no"].as_i64(),
            c["chair"].as_i64(),
            c["absent"].as_i64()
        ),
        (Some(3), Some(1), Some(1), Some(1))
    );
    // Abstention and obstruction became custom columns: 2 deputies, counted as undecided.
    assert_eq!(c["undecided"], 2);
    let names: Vec<&str> = sc["data"]["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"Abstenção") && names.contains(&"Obstrução"));
    assert_eq!(sc["data"]["dissidents"].as_array().unwrap().len(), 3);

    // A roll call with no votes cannot make a scenario.
    let (s, _) = call(
        &app,
        "POST",
        &format!("/diogenes/projects/{}/scenarios", id(&project)),
        Some(&user),
        Some(json!({"name": "X", "roll_call": "nope"})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn migrations_are_idempotent() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    assert_eq!(diogenes::migrate(&db).await.unwrap(), 0);
    // An operator applies the file again with psql: no error.
    sqlx::raw_sql(include_str!("../migrations/0001_init.sql"))
        .execute(&db)
        .await
        .unwrap();
}
