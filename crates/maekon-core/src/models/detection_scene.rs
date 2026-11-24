//! Pure latest-request scheduling state for #12019; not connected to runtime.
//!
//! Revisions are opaque caller observations, not native scene identity or capture
//! permission. One serialized owner must retain this model and use its tickets
//! only with that instance. Runtime still owns capture provenance, live consent,
//! publication, synchronization, bounded retries, and provider termination.

/// A scheduled observation. The generation distinguishes repeated scene revisions
/// across invalidation; it must never be reconstructed from provider output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetectionTicket {
    generation: u64,
    revision: u64,
}

impl DetectionTicket {
    pub fn revision(self) -> u64 {
        self.revision
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectionRequest {
    Disabled,
    Duplicate,
    Queued,
    Start(DetectionTicket),
    GenerationExhausted,
}

/// Returned only when the actual running operation has terminated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetectionCompletion {
    /// Ordering eligibility at this transition, never permission to publish.
    /// The runtime must also check success, current identity and live consent.
    pub was_current: bool,
    /// Already occupies the running slot; dispatch it exactly once.
    pub next: Option<DetectionTicket>,
}

/// Bounded state: at most one running ticket and one replaceable pending ticket.
/// Starts disabled and does not spawn workers, capture pixels, or retry failures.
#[derive(Debug, Default)]
pub struct DetectionSceneState {
    enabled: bool,
    generation: u64,
    latest: Option<DetectionTicket>,
    running: Option<DetectionTicket>,
    pending: Option<DetectionTicket>,
}

impl DetectionSceneState {
    /// Every call invalidates prior observations, including re-enabling. Retain
    /// the running slot until real completion: timeout/cancel is not termination.
    pub fn reset(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.latest = None;
        self.pending = None;
    }

    /// Submit a caller observation. Repeated latest revisions are coalesced even
    /// after completion; a deliberate retry requires explicit reset/new revision.
    /// Counter exhaustion leaves all state unchanged and never reuses a ticket.
    pub fn request(&mut self, revision: u64) -> DetectionRequest {
        if !self.enabled {
            return DetectionRequest::Disabled;
        }
        if self.latest.map(|ticket| ticket.revision) == Some(revision) {
            return DetectionRequest::Duplicate;
        }
        let Some(generation) = self.generation.checked_add(1) else {
            return DetectionRequest::GenerationExhausted;
        };
        let ticket = DetectionTicket {
            generation,
            revision,
        };
        self.generation = generation;
        self.latest = Some(ticket);
        if self.running.is_some() {
            self.pending = Some(ticket);
            DetectionRequest::Queued
        } else {
            self.running = Some(ticket);
            DetectionRequest::Start(ticket)
        }
    }

    /// Report actual worker termination (success or failure), not caller timeout.
    /// A duplicate/obsolete completion cannot release another operation's slot.
    /// Taking pending and reserving the next slot is one serialized transition;
    /// this is not a proof that a future asynchronous adapter has no lost wakeup.
    pub fn complete(&mut self, ticket: DetectionTicket) -> Option<DetectionCompletion> {
        if self.running != Some(ticket) {
            return None;
        }
        let was_current = self.latest == Some(ticket);
        self.running = self.pending.take();
        Some(DetectionCompletion {
            was_current,
            next: self.running,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active() -> DetectionSceneState {
        let mut state = DetectionSceneState::default();
        state.reset(true);
        state
    }

    fn start(state: &mut DetectionSceneState, revision: u64) -> DetectionTicket {
        let DetectionRequest::Start(ticket) = state.request(revision) else {
            panic!("expected an immediately scheduled observation");
        };
        ticket
    }

    #[test]
    fn latest_pending_replaces_intermediate_scene_and_drains_without_new_request() {
        let mut state = active();
        let a = start(&mut state, 1);
        assert_eq!(state.request(2), DetectionRequest::Queued);
        assert_eq!(state.request(3), DetectionRequest::Queued);
        let completed = state.complete(a).unwrap();
        assert!(!completed.was_current);
        let c = completed.next.unwrap();
        assert_eq!(c.revision(), 3);
        assert_eq!(state.complete(a), None);
        assert_eq!(
            state.complete(c),
            Some(DetectionCompletion {
                was_current: true,
                next: None
            })
        );
    }

    #[test]
    fn single_scene_and_duplicates_do_not_reschedule() {
        let mut state = active();
        let a = start(&mut state, 10);
        assert_eq!(state.request(10), DetectionRequest::Duplicate);
        assert_eq!(
            state.complete(a),
            Some(DetectionCompletion {
                was_current: true,
                next: None
            })
        );
        assert_eq!(state.request(10), DetectionRequest::Duplicate);
        assert_eq!(state.complete(a), None);
    }

    #[test]
    fn pending_duplicate_does_not_allocate_another_generation() {
        let mut state = active();
        let a = start(&mut state, 1);
        assert_eq!(state.request(2), DetectionRequest::Queued);
        let before = state.generation;
        assert_eq!(state.request(2), DetectionRequest::Duplicate);
        assert_eq!(state.generation, before);
        assert_eq!(state.complete(a).unwrap().next.unwrap().revision(), 2);
    }

    #[test]
    fn off_on_retains_running_slot_but_drops_pending_and_old_eligibility() {
        let mut state = active();
        let old = start(&mut state, 1);
        assert_eq!(state.request(2), DetectionRequest::Queued);
        state.reset(false);
        assert_eq!(state.request(3), DetectionRequest::Disabled);
        state.reset(true);
        assert_eq!(state.request(1), DetectionRequest::Queued);
        let completed = state.complete(old).unwrap();
        assert!(!completed.was_current);
        let fresh = completed.next.unwrap();
        assert_ne!(old, fresh);
        assert_eq!(state.complete(old), None);
        assert!(state.complete(fresh).unwrap().was_current);
    }

    #[test]
    fn invalidation_clears_pending_without_claiming_worker_cancellation() {
        let mut state = active();
        let a = start(&mut state, 1);
        assert_eq!(state.request(2), DetectionRequest::Queued);
        state.reset(false);
        assert_eq!(
            state.complete(a),
            Some(DetectionCompletion {
                was_current: false,
                next: None
            })
        );
        assert_eq!(state.request(2), DetectionRequest::Disabled);
        state.reset(true);
        assert_eq!(start(&mut state, 2).revision(), 2);
    }

    #[test]
    fn completion_then_request_also_starts_latest_scene() {
        let mut state = active();
        let a = start(&mut state, 1);
        assert!(state.complete(a).unwrap().next.is_none());
        let c = start(&mut state, 3);
        assert_eq!(state.complete(a), None);
        assert!(state.complete(c).unwrap().was_current);
    }

    #[test]
    fn no_completion_signal_keeps_capacity_reserved() {
        let mut state = active();
        let a = start(&mut state, 1);
        // A timeout/cancel notification must not call complete while work lives.
        for revision in 2..=100 {
            assert_eq!(state.request(revision), DetectionRequest::Queued);
            assert_eq!(state.running, Some(a));
        }
        assert_eq!(state.complete(a).unwrap().next.unwrap().revision(), 100);
    }

    #[test]
    fn exhaustion_preserves_existing_work_and_does_not_wrap() {
        let mut state = active();
        state.generation = u64::MAX - 2;
        let a = start(&mut state, 1);
        assert_eq!(state.request(2), DetectionRequest::Queued);
        assert_eq!(state.request(3), DetectionRequest::GenerationExhausted);
        let b = state.complete(a).unwrap().next.unwrap();
        assert_eq!(b.revision(), 2);
        assert!(state.complete(b).unwrap().was_current);
        state.reset(true);
        assert_eq!(state.request(2), DetectionRequest::GenerationExhausted);
    }

    #[test]
    fn default_is_disabled() {
        let mut state = DetectionSceneState::default();
        assert_eq!(state.request(1), DetectionRequest::Disabled);
        assert_eq!(state.generation, 0);
    }

    #[test]
    fn off_on_without_new_request_never_revalidates_old_completion() {
        let mut state = active();
        let old = start(&mut state, 1);
        state.reset(false);
        state.reset(true);
        assert_eq!(
            state.complete(old),
            Some(DetectionCompletion {
                was_current: false,
                next: None,
            })
        );
    }

    #[test]
    fn returning_to_same_revision_gets_new_ticket_without_reset() {
        let mut state = active();
        let old_a = start(&mut state, 1);
        assert_eq!(state.request(2), DetectionRequest::Queued);
        assert_eq!(state.request(1), DetectionRequest::Queued);
        let completion = state.complete(old_a).unwrap();
        assert!(!completion.was_current);
        let new_a = completion.next.unwrap();
        assert_eq!(new_a.revision(), old_a.revision());
        assert_ne!(new_a, old_a);
        assert_eq!(state.complete(old_a), None);
        assert_eq!(state.running, Some(new_a));
        assert_eq!(
            state.complete(new_a),
            Some(DetectionCompletion {
                was_current: true,
                next: None,
            })
        );
    }

    #[test]
    fn repeated_enabled_reset_clears_pending_but_retains_live_worker() {
        let mut state = active();
        let running = start(&mut state, 1);
        for revision in 2..=4 {
            assert_eq!(state.request(revision), DetectionRequest::Queued);
            state.reset(true);
            assert_eq!(state.latest, None);
            assert_eq!(state.pending, None);
            assert_eq!(state.running, Some(running));
        }
        assert_eq!(
            state.complete(running),
            Some(DetectionCompletion {
                was_current: false,
                next: None,
            })
        );
        assert_eq!(start(&mut state, 4).revision(), 4);
    }
}
