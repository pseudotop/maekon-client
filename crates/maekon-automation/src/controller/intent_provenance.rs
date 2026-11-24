//! Per-controller, bounded recovery of exact intent submissions (#12060).
//!
//! Correlation tombstones survive record expiry for this controller's lifetime.
//! At their cap, new recovery mappings become unavailable instead of forgetting a
//! duplicate and attaching an old submission to a new execution. This store is
//! process memory; neither registration nor an audit enqueue proves durability.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use maekon_core::models::intent_execution::{
    IntentActionOutcome, IntentExecutionLookup, IntentExecutionObserver, IntentLookupUnknownReason,
    IntentObservation, IntentSubmission, OpaqueIntentId,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const MAX_RECORDS: usize = 256;
const MAX_CORRELATIONS: usize = 4096;
const MAX_ACTIVE_OBSERVERS: usize = 64;
const RETENTION: Duration = Duration::from_secs(15 * 60);

pub(super) struct IntentObservationLease {
    pub(super) observer: IntentExecutionObserver,
    _permit: OwnedSemaphorePermit,
}

#[derive(Clone, Copy)]
enum Correlation {
    One(OpaqueIntentId),
    Unknown(IntentLookupUnknownReason),
}

struct RetainedObservation {
    observer: IntentExecutionObserver,
    expires_at: Instant,
    submission_id: Option<OpaqueIntentId>,
}

#[derive(Default)]
struct StoreState {
    records: BTreeMap<OpaqueIntentId, RetainedObservation>,
    order: VecDeque<OpaqueIntentId>,
    correlations: BTreeMap<OpaqueIntentId, Correlation>,
}

pub(super) struct IntentObservationStore {
    state: Mutex<StoreState>,
    active: Arc<Semaphore>,
    capacity: usize,
    correlation_capacity: usize,
    retention: Duration,
}

impl Default for IntentObservationStore {
    fn default() -> Self {
        Self::with_limits(
            MAX_RECORDS,
            MAX_CORRELATIONS,
            MAX_ACTIVE_OBSERVERS,
            RETENTION,
        )
    }
}

impl IntentObservationStore {
    fn with_limits(
        capacity: usize,
        correlation_capacity: usize,
        active: usize,
        retention: Duration,
    ) -> Self {
        Self {
            state: Mutex::default(),
            active: Arc::new(Semaphore::new(active)),
            capacity,
            correlation_capacity,
            retention,
        }
    }

    pub(super) fn begin(&self, submission: IntentSubmission) -> Option<IntentObservationLease> {
        self.begin_at(submission, Instant::now())
    }

    fn begin_at(
        &self,
        mut submission: IntentSubmission,
        now: Instant,
    ) -> Option<IntentObservationLease> {
        let permit = self.active.clone().try_acquire_owned().ok();
        let mut state = self.state.lock().ok()?;
        Self::expire(&mut state, now);
        if permit.is_none() || self.capacity == 0 {
            self.record_unavailable(&mut state, submission.submission_id);
            return None;
        }
        let permit = permit?;
        // A client reference is only a link when this exact execution is known.
        submission.retry_of = submission.retry_of.filter(|id| {
            state.records.get(id).is_some_and(|record| {
                let receipt = record.observer.snapshot();
                receipt.action_outcome != IntentActionOutcome::Unknown
                    && receipt.observation != IntentObservation::Partial
            })
        });
        while state.records.len() >= self.capacity {
            let oldest = state.order.pop_front()?;
            Self::remove_record(&mut state, oldest);
        }
        let observer = IntentExecutionObserver::new(submission);
        let id = observer.execution_id();
        if let Some(submission_id) = submission.submission_id {
            if let Some(existing) = state.correlations.get_mut(&submission_id) {
                *existing = Correlation::Unknown(IntentLookupUnknownReason::Ambiguous);
            } else if state.correlations.len() < self.correlation_capacity {
                state
                    .correlations
                    .insert(submission_id, Correlation::One(id));
            } else {
                observer.mark_partial();
            }
        }
        state.order.push_back(id);
        state.records.insert(
            id,
            RetainedObservation {
                observer: observer.clone(),
                expires_at: now + self.retention,
                submission_id: submission.submission_id,
            },
        );
        Some(IntentObservationLease {
            observer,
            _permit: permit,
        })
    }

    fn record_unavailable(&self, state: &mut StoreState, submission: Option<OpaqueIntentId>) {
        let Some(submission) = submission else {
            return;
        };
        if let Some(existing) = state.correlations.get_mut(&submission) {
            *existing = Correlation::Unknown(IntentLookupUnknownReason::Ambiguous);
        } else if state.correlations.len() < self.correlation_capacity {
            state.correlations.insert(
                submission,
                Correlation::Unknown(IntentLookupUnknownReason::Unavailable),
            );
        }
    }

    pub(super) fn lookup(&self, submission: OpaqueIntentId) -> IntentExecutionLookup {
        self.lookup_at(submission, Instant::now())
    }

    fn lookup_at(&self, submission: OpaqueIntentId, now: Instant) -> IntentExecutionLookup {
        let Ok(mut state) = self.state.lock() else {
            return IntentExecutionLookup::Unknown(IntentLookupUnknownReason::Unavailable);
        };
        Self::expire(&mut state, now);
        match state.correlations.get(&submission) {
            Some(Correlation::One(id)) => match state.records.get(id) {
                Some(record) => IntentExecutionLookup::Found(Box::new(record.observer.snapshot())),
                None => IntentExecutionLookup::Unknown(IntentLookupUnknownReason::Expired),
            },
            Some(Correlation::Unknown(reason)) => IntentExecutionLookup::Unknown(*reason),
            None => IntentExecutionLookup::Unknown(IntentLookupUnknownReason::Missing),
        }
    }

    fn expire(state: &mut StoreState, now: Instant) {
        // Concurrent callers can acquire the mutex in a different order from
        // their clock observations; expiry must not rely on insertion order.
        let expired: Vec<_> = state
            .records
            .iter()
            .filter(|(_, record)| record.expires_at <= now)
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            Self::remove_record(state, id);
        }
        let StoreState { records, order, .. } = state;
        order.retain(|id| records.contains_key(id));
    }

    fn remove_record(state: &mut StoreState, id: OpaqueIntentId) {
        let Some(record) = state.records.remove(&id) else {
            return;
        };
        record.observer.mark_partial();
        if let Some(submission) = record.submission_id {
            if matches!(state.correlations.get(&submission), Some(Correlation::One(found)) if *found == id)
            {
                state.correlations.insert(
                    submission,
                    Correlation::Unknown(IntentLookupUnknownReason::Expired),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn submitted(id: OpaqueIntentId) -> IntentSubmission {
        IntentSubmission {
            submission_id: Some(id),
            retry_of: None,
        }
    }

    #[test]
    fn duplicate_submission_never_selects_a_candidate_even_after_expiry() {
        let store = IntentObservationStore::with_limits(4, 8, 4, Duration::from_secs(2));
        let now = Instant::now();
        let id = OpaqueIntentId::new();
        let first = store.begin_at(submitted(id), now).unwrap();
        let second = store
            .begin_at(submitted(id), now + Duration::from_secs(1))
            .unwrap();
        assert_ne!(
            first.observer.execution_id(),
            second.observer.execution_id()
        );
        assert_eq!(
            store.lookup_at(id, now + Duration::from_secs(2)),
            IntentExecutionLookup::Unknown(IntentLookupUnknownReason::Ambiguous)
        );
        let third = store
            .begin_at(submitted(id), now + Duration::from_secs(4))
            .unwrap();
        assert_ne!(first.observer.execution_id(), third.observer.execution_id());
        assert_eq!(
            store.lookup_at(id, now + Duration::from_secs(4)),
            IntentExecutionLookup::Unknown(IntentLookupUnknownReason::Ambiguous)
        );
    }

    #[test]
    fn expiry_capacity_and_saturated_tombstones_do_not_rebind_old_submissions() {
        let store = IntentObservationStore::with_limits(1, 2, 3, Duration::from_secs(2));
        let now = Instant::now();
        let a = OpaqueIntentId::new();
        let b = OpaqueIntentId::new();
        let c = OpaqueIntentId::new();
        let first = store.begin_at(submitted(a), now).unwrap();
        first.observer.finish(IntentActionOutcome::Completed, None);
        let _second = store.begin_at(submitted(b), now).unwrap();
        assert_eq!(
            store.lookup_at(a, now),
            IntentExecutionLookup::Unknown(IntentLookupUnknownReason::Expired)
        );
        let third = store.begin_at(submitted(c), now).unwrap();
        third.observer.finish(IntentActionOutcome::Completed, None);
        assert_eq!(
            third.observer.snapshot().observation,
            IntentObservation::Partial
        );
        assert!(!matches!(
            store.lookup_at(c, now),
            IntentExecutionLookup::Found(_)
        ));
        assert_eq!(
            store.lookup_at(b, now + Duration::from_secs(3)),
            IntentExecutionLookup::Unknown(IntentLookupUnknownReason::Expired)
        );
        let state = store.state.lock().unwrap();
        assert!(state.records.len() <= 1);
        assert_eq!(state.correlations.len(), 2);
    }

    #[test]
    fn active_limit_and_interrupted_finalization_are_observable_without_replay() {
        let store = IntentObservationStore::with_limits(2, 4, 1, Duration::from_secs(10));
        let id = OpaqueIntentId::new();
        let first = store.begin(submitted(id)).unwrap();
        let unavailable = OpaqueIntentId::new();
        assert!(store.begin(submitted(unavailable)).is_none());
        assert_eq!(
            store.lookup(unavailable),
            IntentExecutionLookup::Unknown(IntentLookupUnknownReason::Unavailable)
        );
        first
            .observer
            .record_outcome(IntentActionOutcome::Completed, None);
        drop(first); // No async finalization in Drop.
        let IntentExecutionLookup::Found(receipt) = store.lookup(id) else {
            panic!("retained partial observation");
        };
        assert_eq!(receipt.observation, IntentObservation::Partial);
        assert!(store.begin(submitted(OpaqueIntentId::new())).is_some());
    }

    #[test]
    fn default_recovery_expires_at_fifteen_minutes_without_sleeping() {
        let store = IntentObservationStore::default();
        let now = Instant::now();
        let id = OpaqueIntentId::new();
        let lease = store.begin_at(submitted(id), now).unwrap();
        lease.observer.finish(IntentActionOutcome::Completed, None);
        assert_eq!(
            store.lookup_at(id, now + Duration::from_secs(899)),
            IntentExecutionLookup::Found(Box::new(lease.observer.snapshot()))
        );
        assert_eq!(
            store.lookup_at(id, now + Duration::from_secs(900)),
            IntentExecutionLookup::Unknown(IntentLookupUnknownReason::Expired)
        );
        assert_eq!(
            lease.observer.snapshot().observation,
            IntentObservation::Partial
        );
    }

    #[test]
    fn unavailable_correlations_remain_bounded_and_never_rebind_duplicates() {
        let store = IntentObservationStore::with_limits(0, 2, 1, Duration::from_secs(10));
        let first = OpaqueIntentId::new();
        let second = OpaqueIntentId::new();
        let excess = OpaqueIntentId::new();
        assert!(store.begin(submitted(first)).is_none());
        assert_eq!(
            store.lookup(first),
            IntentExecutionLookup::Unknown(IntentLookupUnknownReason::Unavailable)
        );
        assert!(store.begin(submitted(first)).is_none());
        assert_eq!(
            store.lookup(first),
            IntentExecutionLookup::Unknown(IntentLookupUnknownReason::Ambiguous)
        );
        assert!(store.begin(submitted(second)).is_none());
        assert_eq!(
            store.lookup(second),
            IntentExecutionLookup::Unknown(IntentLookupUnknownReason::Unavailable)
        );
        assert!(store.begin(submitted(excess)).is_none());
        assert_eq!(
            store.lookup(excess),
            IntentExecutionLookup::Unknown(IntentLookupUnknownReason::Missing)
        );
        let state = store.state.lock().unwrap();
        assert!(state.records.is_empty());
        assert_eq!(state.correlations.len(), 2);
    }

    #[test]
    fn retry_links_require_both_known_outcome_and_completed_observation() {
        for (outcome, finalized) in [
            (IntentActionOutcome::Unknown, true),
            (IntentActionOutcome::Completed, false),
        ] {
            let store = IntentObservationStore::default();
            let first = store.begin(submitted(OpaqueIntentId::new())).unwrap();
            first.observer.record_outcome(outcome, None);
            if finalized {
                first.observer.finalize();
            }
            let retry = store
                .begin(IntentSubmission {
                    submission_id: Some(OpaqueIntentId::new()),
                    retry_of: Some(first.observer.execution_id()),
                })
                .unwrap();
            assert_eq!(retry.observer.snapshot().retry_of, None);
        }
    }

    #[test]
    fn two_controller_scopes_cannot_read_each_others_execution_or_retry_link() {
        let one = IntentObservationStore::default();
        let two = IntentObservationStore::default();
        let submission = OpaqueIntentId::new();
        let first = one.begin(submitted(submission)).unwrap();
        first.observer.finish(IntentActionOutcome::Completed, None);
        assert_eq!(
            two.lookup(submission),
            IntentExecutionLookup::Unknown(IntentLookupUnknownReason::Missing)
        );
        let retry = two
            .begin(IntentSubmission {
                submission_id: Some(OpaqueIntentId::new()),
                retry_of: Some(first.observer.execution_id()),
            })
            .unwrap();
        assert_eq!(retry.observer.snapshot().retry_of, None);
        let own_retry = one
            .begin(IntentSubmission {
                submission_id: Some(OpaqueIntentId::new()),
                retry_of: Some(first.observer.execution_id()),
            })
            .unwrap();
        assert_eq!(
            own_retry.observer.snapshot().retry_of,
            Some(first.observer.execution_id())
        );
    }
}
