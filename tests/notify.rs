//! The host gets a notice when a room owner adds a new member.
//!
//! Set `DATABASE_URL` and run `cargo test -- --include-ignored`.

mod common;

use axum::http::StatusCode;
use common::{app_with_notices, call, db, User, SERIAL};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
#[ignore = "needs DATABASE_URL"]
async fn a_new_member_gets_one_notice_and_a_role_change_gets_none() {
    let _guard = SERIAL.lock().await;
    let db = db().await;
    let org = Uuid::now_v7();
    let owner = User::new(org, "Ana", "ana", true);
    let bia = User::new(org, "Bia", "bia", false);
    let (app, notices) = app_with_notices(db, &[&owner, &bia]);

    let (s, room) = call(
        &app,
        "POST",
        "/diogenes/rooms",
        Some(&owner),
        Some(json!({"name": "PL 1"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let room_id: Uuid = room["data"]["id"].as_str().unwrap().parse().unwrap();
    // The owner of a new room is a member, but nobody added the owner: no notice.
    assert!(notices.lock().unwrap().is_empty());

    let add = |role: &'static str| {
        let app = app.clone();
        let owner = owner.clone();
        async move {
            call(
                &app,
                "POST",
                &format!("/diogenes/rooms/{room_id}/members"),
                Some(&owner),
                Some(json!({"handle": "@bia", "role": role})),
            )
            .await
            .0
        }
    };
    assert_eq!(add("editor").await, StatusCode::OK);
    {
        let got = notices.lock().unwrap();
        assert_eq!(got.len(), 1);
        let n = &got[0];
        assert_eq!(n.org_id, org);
        assert_eq!(n.room_id, room_id);
        assert_eq!(n.room_name, "PL 1");
        assert_eq!(n.user_id, bia.id);
        assert_eq!(n.role, "editor");
        assert_eq!(n.added_by, owner.id);
        assert_eq!(n.added_by_name, "Ana");
    }

    // A role change of a member is not a new membership.
    assert_eq!(add("viewer").await, StatusCode::OK);
    assert_eq!(notices.lock().unwrap().len(), 1);

    // An owner who adds themself again gets no notice.
    let (s, _) = call(
        &app,
        "POST",
        &format!("/diogenes/rooms/{room_id}/members"),
        Some(&owner),
        Some(json!({"handle": "ana", "role": "owner"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(notices.lock().unwrap().len(), 1);
}
