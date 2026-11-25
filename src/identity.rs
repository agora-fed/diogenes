//! The identity boundary. The host implements [`Identity`]. DIOGENES never reads
//! cookies, tokens or host tables.

use async_trait::async_trait;
use axum::http::request::Parts;
use serde::Serialize;
use uuid::Uuid;

/// The authenticated caller of a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    /// The host user id.
    pub user_id: Uuid,
    /// The host organization (tenant) of the caller.
    pub org_id: Uuid,
    /// The public name of the caller.
    pub display_name: String,
    /// True when the caller can make new rooms.
    pub can_create_rooms: bool,
    /// True when the caller can operate the data sync.
    pub can_operate_sync: bool,
}

/// A host user that a room owner can add to a room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UserRef {
    /// The host user id.
    pub user_id: Uuid,
    /// The public name of the user.
    pub display_name: String,
}

/// A notice to the host: a room owner added a new member to a room.
///
/// DIOGENES sends it after the change is saved. It sends it only for a new member. It does
/// not send it for a role change or when an owner adds themself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberAdded {
    /// The organization of the room.
    pub org_id: Uuid,
    /// The room.
    pub room_id: Uuid,
    /// The name of the room.
    pub room_name: String,
    /// The new member.
    pub user_id: Uuid,
    /// The role of the new member: `viewer`, `editor` or `owner`.
    pub role: String,
    /// The owner who added the member.
    pub added_by: Uuid,
    /// The public name of that owner.
    pub added_by_name: String,
}

/// The host adapter for identity.
#[async_trait]
pub trait Identity: Send + Sync + 'static {
    /// Return the caller of the request, or `None` when nobody is signed in.
    async fn actor(&self, parts: &Parts) -> Option<Actor>;

    /// Find a user of the same organization by a public handle.
    /// Return `None` when no user has that handle.
    async fn find_user(&self, org_id: Uuid, handle: &str) -> Option<UserRef>;

    /// Return true when the DIOGENES module is active for the organization.
    /// The default is true. A host with per-organization modules overrides it.
    async fn module_active(&self, _org_id: Uuid) -> bool {
        true
    }

    /// Tell the new member about the room (for example, by e-mail).
    /// The default does nothing. The host must not block for a long time and must not fail:
    /// the member is already saved.
    async fn member_added(&self, _notice: MemberAdded) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Minimal;

    #[async_trait]
    impl Identity for Minimal {
        async fn actor(&self, _parts: &Parts) -> Option<Actor> {
            None
        }

        async fn find_user(&self, _org_id: Uuid, _handle: &str) -> Option<UserRef> {
            None
        }
    }

    #[tokio::test]
    async fn module_is_active_unless_the_host_overrides_it() {
        assert!(Minimal.module_active(Uuid::now_v7()).await);
    }

    #[tokio::test]
    async fn member_notice_does_nothing_unless_the_host_overrides_it() {
        let id = Uuid::now_v7();
        Minimal
            .member_added(MemberAdded {
                org_id: id,
                room_id: id,
                room_name: "Sala".into(),
                user_id: id,
                role: "viewer".into(),
                added_by: id,
                added_by_name: "Ana".into(),
            })
            .await;
    }
}
