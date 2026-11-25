//! Integration tests: edge cases of rooms, projects, scenarios, streams and the schema runner.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{app, call, db, seed_deputies, User, SERIAL};
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

fn data_id(v: &Value) -> String {
    v["data"]
        .as_str()
        .or_else(|| v["data"]["id"].as_str())
        .unwrap()
        .to_owned()
}

/// A room with one project and one scenario. The owner is `owner`.
struct Fixture {
    app: axum::Router,
    owner: User,
    editor: User,
    viewer: User,
    room: String,
    project: String,
    scenario: String,
}

async fn fixture(db: &sqlx::PgPool) -> Fixture {
    seed_deputies(db, 8).await;
    let org = Uuid::now_v7();
    let owner = User::new(org, "Ana", "ana", true);
    let editor = User::new(org, "Bruno", "bruno", false);
    let viewer = User::new(org, "Carla", "carla", false);
    let app = app(db.clone(), &[&owner, &editor, &viewer], vec![]);
    let (_, room) = call(
        &app,
        "POST",
        "/diogenes/rooms",
        Some(&owner),
        Some(json!({"name": "Sala"})),
    )
    .await;
    let room = data_id(&room);
    for (user, role) in [(&editor, "editor"), (&viewer, "viewer")] {
        let (s, _) = call(
            &app,
            "POST",
            &format!("/diogenes/rooms/{room}/members"),
            Some(&owner),
            Some(json!({"handle": user.handle, "role": role})),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
    }
    let (_, project) = call(
        &app,
        "POST",
        &format!("/diogenes/rooms/{room}/projects"),
        Some(&owner),
        Some(json!({"name": "PL 1"})),
    )
    .await;
    let project = data_id(&project);
    let (_, scenario) = call(
        &app,
        "POST",
        &format!("/diogenes/projects/{project}/scenarios"),
        Some(&owner),
        Some(json!({"name": "Base"})),
    )
    .await;
    let scenario = data_id(&scenario);
    Fixture {
        app,
        owner,
        editor,
        viewer,
        room,
        project,
        scenario,
    }
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn room_detail_shows_members_in_role_order_and_the_projects() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    let f = fixture(&db).await;

    let (s, body) = call(
        &f.app,
        "GET",
        &format!("/diogenes/rooms/{}", f.room),
        Some(&f.viewer),
        None,
    )
    .await;

    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["name"], "Sala");
    assert_eq!(body["data"]["role"], "viewer");
    assert_eq!(body["data"]["can_create_rooms"], false);
    let members: Vec<(&str, &str)> = body["data"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["display_name"].as_str().unwrap(),
                m["role"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        members,
        vec![("Ana", "owner"), ("Bruno", "editor"), ("Carla", "viewer")]
    );
    let projects = body["data"]["projects"].as_array().unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0]["name"], "PL 1");
    assert_eq!(projects[0]["id"], f.project.as_str());
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn member_handle_and_removal_rules() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    let f = fixture(&db).await;
    let members = format!("/diogenes/rooms/{}/members", f.room);

    for handle in ["", "  @ ", &"a".repeat(101)] {
        let (s, body) = call(
            &f.app,
            "POST",
            &members,
            Some(&f.owner),
            Some(json!({"handle": handle, "role": "viewer"})),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "handle {handle:?}");
        assert_eq!(body["error"]["code"], "invalid");
    }

    // A viewer can leave, but cannot remove another member.
    let (s, _) = call(
        &f.app,
        "DELETE",
        &format!("{members}/{}", f.editor.id),
        Some(&f.viewer),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let still_there: i64 =
        sqlx::query_scalar("SELECT count(*) FROM diogenes_room_member WHERE user_id = $1")
            .bind(f.editor.id)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(still_there, 1);
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn priorities_can_be_unmarked() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    let f = fixture(&db).await;
    let path = format!("/diogenes/projects/{}/priorities", f.project);
    let marked = |ids: Vec<i32>| async {
        sqlx::query_scalar::<_, i32>(
            "SELECT camara_id FROM diogenes_priority WHERE project_id = $1::uuid ORDER BY 1",
        )
        .bind(&f.project)
        .fetch_all(&db)
        .await
        .map(|got| (got, ids))
        .unwrap()
    };

    call(
        &f.app,
        "PUT",
        &path,
        Some(&f.editor),
        Some(json!({"camara_ids": [2, 3, 4], "value": true})),
    )
    .await;
    let (s, _) = call(
        &f.app,
        "PUT",
        &path,
        Some(&f.editor),
        Some(json!({"camara_ids": [3], "value": false})),
    )
    .await;

    assert_eq!(s, StatusCode::OK);
    let (got, _) = marked(vec![]).await;
    assert_eq!(got, vec![2, 4]);
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn only_the_author_or_an_owner_can_delete_a_note() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    let f = fixture(&db).await;
    let (_, note) = call(
        &f.app,
        "POST",
        &format!("/diogenes/projects/{}/notes", f.project),
        Some(&f.editor),
        Some(json!({"camara_id": 2, "body": "conversar"})),
    )
    .await;
    let path = format!("/diogenes/notes/{}", data_id(&note));

    let (s, _) = call(&f.app, "DELETE", &path, Some(&f.viewer), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(&f.app, "DELETE", &path, Some(&f.owner), None).await;
    assert_eq!(s, StatusCode::OK, "an owner can moderate");
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM diogenes_note")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn pasted_list_over_the_limit_is_refused() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    let f = fixture(&db).await;
    let path = format!("/diogenes/projects/{}/match-list", f.project);

    let (s, body) = call(
        &f.app,
        "POST",
        &path,
        Some(&f.viewer),
        Some(json!({"text": "a".repeat(100_001)})),
    )
    .await;

    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["message"], "Texto longo demais.");
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn scenario_input_limits() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    let f = fixture(&db).await;
    let create = format!("/diogenes/projects/{}/scenarios", f.project);
    let positions = format!("/diogenes/scenarios/{}/positions", f.scenario);

    // A roll-call id that is empty or too long.
    for roll_call in [String::new(), "x".repeat(33)] {
        let (s, body) = call(
            &f.app,
            "POST",
            &create,
            Some(&f.editor),
            Some(json!({"name": "X", "roll_call": roll_call})),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["message"], "Votação inválida.");
    }

    // No moves, and too many moves.
    let too_many: Vec<Value> = (0..601)
        .map(|i| json!({"camara_id": i, "column_id": null}))
        .collect();
    for moves in [vec![], too_many] {
        let (s, body) = call(
            &f.app,
            "PUT",
            &positions,
            Some(&f.editor),
            Some(json!({ "moves": moves })),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("1 e 600"));
    }
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn moving_to_no_column_or_undecided_clears_the_position() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    let f = fixture(&db).await;
    let (_, project) = call(
        &f.app,
        "GET",
        &format!("/diogenes/projects/{}", f.project),
        Some(&f.editor),
        None,
    )
    .await;
    let column = |key: &str| -> String {
        project["data"]["columns"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["key"] == key)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let positions = format!("/diogenes/scenarios/{}/positions", f.scenario);
    let stored = || async {
        sqlx::query_scalar::<_, i32>(
            "SELECT camara_id FROM diogenes_position WHERE scenario_id = $1::uuid ORDER BY 1",
        )
        .bind(&f.scenario)
        .fetch_all(&db)
        .await
        .unwrap()
    };
    call(
        &f.app,
        "PUT",
        &positions,
        Some(&f.editor),
        Some(json!({"moves": [
            {"camara_id": 2, "column_id": column("yes")},
            {"camara_id": 3, "column_id": column("yes")},
            {"camara_id": 4, "column_id": column("no")}]})),
    )
    .await;
    // Deputy 1 is the chair: a new scenario places the chair by default.
    assert_eq!(stored().await, vec![1, 2, 3, 4]);

    let (s, _) = call(
        &f.app,
        "PUT",
        &positions,
        Some(&f.editor),
        Some(json!({"moves": [
            {"camara_id": 2, "column_id": null},
            {"camara_id": 3, "column_id": column("undecided")}]})),
    )
    .await;

    assert_eq!(s, StatusCode::OK);
    assert_eq!(stored().await, vec![1, 4]);
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn roll_call_scenario_reuses_custom_columns_and_skips_other_votes() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    let f = fixture(&db).await;
    sqlx::raw_sql(
        "INSERT INTO diogenes_roll_call (id, voted_on, body, description) VALUES ('2-1', '2026-11-23', 'PLEN', 'x');
         INSERT INTO diogenes_roll_call_vote (roll_call_id, camara_id, vote, party, uf) VALUES
           ('2-1', 2, 'Abstenção', 'PT', 'SP'), ('2-1', 3, 'Abstenção', 'PL', 'SP'),
           ('2-1', 4, 'Obstrução', 'PT', 'SP'), ('2-1', 5, 'Sim', 'PL', 'SP'),
           ('2-1', 6, 'Ausente', 'PT', 'SP');",
    )
    .execute(&db)
    .await
    .unwrap();
    let create = format!("/diogenes/projects/{}/scenarios", f.project);

    // Two scenarios from the same roll call: the second reuses the custom columns.
    for name in ["Primeira", "Segunda"] {
        let (s, body) = call(
            &f.app,
            "POST",
            &create,
            Some(&f.editor),
            Some(json!({"name": name, "roll_call": "2-1", "absent_if_missing": false})),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{body}");
    }

    let custom: Vec<(String, i64)> = sqlx::query_as(
        "SELECT name, count(*) FROM diogenes_column WHERE project_id = $1::uuid AND key IS NULL GROUP BY name ORDER BY name",
    )
    .bind(&f.project)
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!(
        custom,
        vec![("Abstenção".to_owned(), 1), ("Obstrução".to_owned(), 1)]
    );
    // The vote "Ausente" has no column, and `absent_if_missing: false` adds none.
    let placed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM diogenes_position p JOIN diogenes_scenario s ON s.id = p.scenario_id
          WHERE s.name = 'Segunda'",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(placed, 4, "deputies 2, 3, 4 and 5");
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn live_stream_is_limited_per_user_and_released() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    let f = fixture(&db).await;
    let uri = format!("/diogenes/projects/{}/events", f.project);
    let open = || {
        let req = Request::builder()
            .uri(&uri)
            .header("x-test-user", f.viewer.id.to_string())
            .body(Body::empty())
            .unwrap();
        f.app.clone().oneshot(req)
    };

    let mut streams = Vec::new();
    for _ in 0..diogenes::room::events::MAX_STREAMS_PER_USER {
        let res = open().await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        streams.push(res);
    }
    let refused = open().await.unwrap();
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);

    // Other users keep their own limit.
    let req = Request::builder()
        .uri(&uri)
        .header("x-test-user", f.editor.id.to_string())
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        f.app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::OK
    );

    // Closing one stream frees a slot.
    drop(streams.pop());
    assert_eq!(open().await.unwrap().status(), StatusCode::OK);
}

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn migrate_applies_a_missing_version_once_even_when_runners_race() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    sqlx::query("DELETE FROM diogenes_migration WHERE version = 1")
        .execute(&db)
        .await
        .unwrap();

    let results = tokio::join!(
        diogenes::migrate(&db),
        diogenes::migrate(&db),
        diogenes::migrate(&db)
    );

    let applied: usize = [results.0, results.1, results.2]
        .into_iter()
        .map(|r| r.unwrap())
        .sum();
    assert_eq!(applied, 1, "the advisory lock lets one runner apply it");
    let recorded: i64 =
        sqlx::query_scalar("SELECT count(*) FROM diogenes_migration WHERE version = 1")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(recorded, 1);
    assert_eq!(diogenes::migrate(&db).await.unwrap(), 0);
}
