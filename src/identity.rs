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
}
