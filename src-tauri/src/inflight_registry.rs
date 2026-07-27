//! #8057 (P3): registry of abortable in-flight AI-session drain tasks.
//!
//! `send_session_message` spawns a background task per turn that drains the
//! provider stream and consumes BYOK tokens. Backends WITHOUT a native
//! in-flight-turn interrupt (HTTP/Ollama) have no way to stop that consumption
//! when the user hits "stop". This registry stores each task's [`AbortHandle`]
//! (keyed by session id) so `interrupt_session_turn` can cancel it — dropping
//! the drain task drops the stream, which cancels the underlying request. A
//! monotonic token guards deregistration so a slow-finishing task never evicts a
//! newer same-session registration.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use tokio::task::AbortHandle;

/// Registry of abortable in-flight drain tasks (see module docs).
#[derive(Default)]
pub(crate) struct InflightRegistry {
    next_token: AtomicU64,
    tasks: Mutex<HashMap<String, (u64, AbortHandle)>>,
}

impl InflightRegistry {
    /// Reserve a fresh monotonic token for a drain task BEFORE it is spawned, so
    /// the token can be moved into the task body (which presents it to
    /// [`deregister`](Self::deregister)) while [`bind`](Self::bind) stores the
    /// task's abort handle under the same token after spawn.
    pub(crate) fn reserve_token(&self) -> u64 {
        self.next_token.fetch_add(1, Ordering::Relaxed)
    }

    /// Store a drain task's abort handle under `token`, replacing any prior entry
    /// for `session_id`.
    pub(crate) fn bind(&self, session_id: String, token: u64, handle: AbortHandle) {
        self.tasks.lock().insert(session_id, (token, handle));
    }

    /// Deregister a task's own entry on completion — a no-op if a newer turn has
    /// already replaced it (token mismatch), preventing a slow-finishing task
    /// from evicting a newer same-session registration.
    pub(crate) fn deregister(&self, session_id: &str, token: u64) {
        let mut tasks = self.tasks.lock();
        if tasks.get(session_id).is_some_and(|(t, _)| *t == token) {
            tasks.remove(session_id);
        }
    }

    /// Abort the in-flight drain task for `session_id`, if any. Returns true when
    /// a task was aborted.
    pub(crate) fn abort(&self, session_id: &str) -> bool {
        if let Some((_, handle)) = self.tasks.lock().remove(session_id) {
            handle.abort();
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn token_guards_deregister_and_abort_cancels() {
        // A stale deregister (superseded token) must NOT evict a newer
        // same-session registration; a matching-token deregister removes it;
        // abort cancels + removes.
        fn spawn_forever() -> tokio::task::JoinHandle<()> {
            tokio::spawn(async {
                loop {
                    tokio::task::yield_now().await;
                }
            })
        }
        let reg = InflightRegistry::default();

        let first = spawn_forever();
        let stale_token = reg.reserve_token();
        reg.bind("s1".to_string(), stale_token, first.abort_handle());

        // A newer turn replaces the entry.
        let newer = spawn_forever();
        let newer_token = reg.reserve_token();
        reg.bind("s1".to_string(), newer_token, newer.abort_handle());

        // The first task finishing late must NOT drop the newer registration.
        reg.deregister("s1", stale_token);
        assert!(
            reg.abort("s1"),
            "newer registration must still be abortable"
        );
        assert!(!reg.abort("s1"), "a second abort finds nothing");

        // A matching-token deregister removes the entry (natural completion path).
        let solo = spawn_forever();
        let solo_token = reg.reserve_token();
        reg.bind("s2".to_string(), solo_token, solo.abort_handle());
        reg.deregister("s2", solo_token);
        assert!(!reg.abort("s2"), "a deregistered entry is gone");

        first.abort();
        newer.abort();
        solo.abort();
    }
}
