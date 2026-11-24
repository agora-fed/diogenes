//! Live updates. Each project has one broadcast channel. A change sends a small event;
//! the open pages load the changed data again.
//!
//! The hub lives in the process memory. With more than one host process, a page sees
//! only the changes of its own process until it loads again. ADR-0004 records this limit.
//!
//! Limits:
//! * A stream closes after [`STREAM_LIFETIME`]. The browser opens it again, and the server
//!   checks the access again. A removed member stops to get events after this time.
//! * One user can keep [`MAX_STREAMS_PER_USER`] open streams.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::Stream;
use serde::Serialize;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use uuid::Uuid;

const CHANNEL_CAPACITY: usize = 64;
const KEEP_ALIVE: Duration = Duration::from_secs(20);
/// The maximum life of one stream.
pub const STREAM_LIFETIME: Duration = Duration::from_secs(15 * 60);
/// The maximum number of open streams of one user.
pub const MAX_STREAMS_PER_USER: usize = 8;

/// A change in a project.
#[derive(Debug, Clone, Serialize)]
pub struct Change {
    /// What changed: `positions`, `scenario`, `columns`, `priorities`, `notes`, `project`.
    pub what: &'static str,
    /// The scenario, when the change is in one scenario.
    pub scenario_id: Option<Uuid>,
    /// The user who made the change.
    pub by: Uuid,
}

#[derive(Debug, Default)]
struct Inner {
    channels: HashMap<Uuid, broadcast::Sender<Change>>,
    streams_per_user: HashMap<Uuid, usize>,
}

/// The broadcast hub.
#[derive(Debug, Clone, Default)]
pub struct Hub {
    inner: Arc<Mutex<Inner>>,
}

/// Releases one stream slot of a user when the stream ends.
#[derive(Debug)]
struct Slot {
    hub: Hub,
    user: Uuid,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut inner = self.hub.lock();
        if let Some(n) = inner.streams_per_user.get_mut(&self.user) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                inner.streams_per_user.remove(&self.user);
            }
        }
        // Remove the channels with no receiver. This runs when a stream ends, not on publish.
        inner.channels.retain(|_, tx| tx.receiver_count() > 0);
    }
}

impl Hub {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        match self.inner.lock() {
            Ok(m) => m,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn sender(&self, project_id: Uuid) -> broadcast::Sender<Change> {
        self.lock()
            .channels
            .entry(project_id)
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0)
            .clone()
    }

    /// Send a change to the open pages of a project. No page open = no effect.
    pub fn publish(&self, project_id: Uuid, change: Change) {
        let tx = self.lock().channels.get(&project_id).cloned();
        if let Some(tx) = tx {
            let _receivers = tx.send(change);
        }
    }

    /// Open a server-sent-event stream for a project. Return `None` when the user has
    /// too many open streams.
    pub fn subscribe(
        &self,
        project_id: Uuid,
        user: Uuid,
    ) -> Option<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
        {
            let mut inner = self.lock();
            let count = inner.streams_per_user.entry(user).or_insert(0);
            if *count >= MAX_STREAMS_PER_USER {
                return None;
            }
            *count += 1;
        }
        let slot = Slot {
            hub: self.clone(),
            user,
        };
        let events = BroadcastStream::new(self.sender(project_id).subscribe()).map(move |item| {
            let _slot = &slot;
            let event = match item {
                Ok(change) => Event::default()
                    .event("change")
                    .json_data(change)
                    .unwrap_or_else(|_| Event::default().event("reload")),
                // The page missed events: it must load all data again.
                Err(_) => Event::default().event("reload").data("lagged"),
            };
            Ok(event)
        });
        // Total life, not idle time: the stream ends at the deadline, busy or not.
        let stream =
            futures_util::StreamExt::take_until(events, tokio::time::sleep(STREAM_LIFETIME));
        Some(Sse::new(stream).keep_alive(KeepAlive::new().interval(KEEP_ALIVE)))
    }

    #[cfg(test)]
    fn open_streams(&self, user: Uuid) -> usize {
        self.lock()
            .streams_per_user
            .get(&user)
            .copied()
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publish_reaches_subscribers_of_the_same_project_only() {
        let hub = Hub::default();
        let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
        let mut rx_a = hub.sender(a).subscribe();
        let mut rx_b = hub.sender(b).subscribe();
        hub.publish(
            a,
            Change {
                what: "positions",
                scenario_id: None,
                by: Uuid::nil(),
            },
        );
        assert_eq!(rx_a.recv().await.map(|c| c.what).ok(), Some("positions"));
        assert!(rx_b.try_recv().is_err());
    }

    #[tokio::test]
    async fn streams_per_user_are_limited_and_released() {
        let hub = Hub::default();
        let user = Uuid::now_v7();
        let project = Uuid::now_v7();
        let mut open = Vec::new();
        for _ in 0..MAX_STREAMS_PER_USER {
            open.push(hub.subscribe(project, user).expect("slot"));
        }
        assert!(hub.subscribe(project, user).is_none());
        assert!(
            hub.subscribe(project, Uuid::now_v7()).is_some(),
            "other users are not blocked"
        );
        drop(open);
        assert_eq!(hub.open_streams(user), 0);
        assert!(hub.subscribe(project, user).is_some());
    }
}
