//! Event-driven LLM analysis helper (context analyzer) + the shared
//! suggestion surfacing funnel.

use std::sync::Arc;

use tracing::{debug, info, warn};

use maekon_core::models::suggestion::{Priority, Suggestion};
use maekon_core::models::tiered_memory::Regime;
use maekon_core::ports::storage::StorageService;

use crate::notification_manager::NotificationManager;

/// Shared surfacing funnel (#5694): push suggestions into the live review
/// queue (fingerprint dedup), then make them DISCOVERABLE — emit the
/// `overlay:suggestions-changed` refresh via `on_changed` when anything was
/// accepted, and toast the highest-priority accepted suggestion (High and
/// above) unless focus mode is active (A4 gate). Notification policy
/// (cooldown / Critical bypass / master switch) lives in
/// [`NotificationManager::notify_suggestion`], so every producer that goes
/// through this seam — the periodic analysis loop, the event-driven path,
/// and (B1, follow-up) the rule producer — inherits one consistent policy.
pub(crate) async fn enqueue_and_surface(
    queue: &Arc<tokio::sync::Mutex<maekon_suggestion::queue::SuggestionQueue>>,
    to_enqueue: Vec<Suggestion>,
    on_changed: Option<&(dyn Fn(usize) + Send + Sync)>,
    notifier: Option<&Arc<NotificationManager>>,
    focus_active: bool,
) {
    let mut accepted = 0usize;
    let mut top: Option<Suggestion> = None;
    let count = {
        let mut q = queue.lock().await;
        for s in to_enqueue {
            let id = s.suggestion_id.clone();
            if q.push(s.clone()) {
                accepted += 1;
                debug!(suggestion_id = %id, "local suggestion enqueued for review");
                if top.as_ref().is_none_or(|t| s.priority > t.priority) {
                    top = Some(s);
                }
            }
        }
        q.len()
        // len-then-drop: the lock is released before emit/notify so UI work
        // never serializes against producers (mirrors suggestions.rs:107-110).
    };

    if accepted == 0 {
        return;
    }
    if let Some(cb) = on_changed {
        cb(count);
    }
    if let (Some(nm), Some(t)) = (notifier, top) {
        if t.priority >= Priority::High && !focus_active {
            nm.notify_suggestion(&t).await;
        }
    }
}

/// Run event-driven LLM analysis when the user switches to a new app.
/// Persists any resulting suggestions to storage.
#[tracing::instrument(skip_all)]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_event_analysis(
    analyzer: &Option<Arc<maekon_analysis::ContextAnalyzer>>,
    storage: &Arc<dyn StorageService>,
    app_name: &str,
    window_title: &str,
    ocr_hint: Option<&str>,
    // E20-24 (#4816): live suggestion queue (the manager's Arc). `None` when the
    // local-suggestions pipeline is absent; `Some` enqueues for live review.
    suggestion_queue: Option<&Arc<tokio::sync::Mutex<maekon_suggestion::queue::SuggestionQueue>>>,
    // E20-26 (#4818): current regime for context-aware gating. `None` => pass-through
    // (no filtering). In a focused regime, low/medium-priority suggestions are dropped
    // before enqueue so an app-switch does not interrupt deep-focus work.
    regime: Option<&Regime>,
    // #5694: overlay auto-refresh + desktop toast for accepted suggestions.
    on_changed: Option<&(dyn Fn(usize) + Send + Sync)>,
    notifier: Option<&Arc<NotificationManager>>,
    focus_active: bool,
) {
    if let Some(ref analyzer) = analyzer {
        match analyzer
            .on_significant_event(app_name, window_title, ocr_hint)
            .await
        {
            Ok(suggestions) => {
                for s in &suggestions {
                    info!(
                        id = %s.suggestion_id,
                        priority = ?s.priority,
                        content = %maekon_monitor::log_privacy::content_digest(&s.content),
                        "event-driven suggestion produced"
                    );
                    if let Err(e) = storage.save_suggestion(s).await {
                        warn!("suggestion save failure: {e}");
                    }
                }
                // E20-24 (#4816): enqueue for live review (same producer wire as the
                // periodic analysis loop). Dedup via queue fingerprint (push -> bool).
                if let Some(q) = suggestion_queue {
                    // E20-26 (#4818): regime/context-aware gating before enqueue.
                    // `regime: None` => pass-through (no filtering).
                    let mut to_enqueue = suggestions.clone();
                    maekon_analysis::filter_by_regime(&mut to_enqueue, regime);
                    enqueue_and_surface(q, to_enqueue, on_changed, notifier, focus_active).await;
                }
            }
            Err(e) => {
                debug!("event analysis skipped: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use maekon_core::models::suggestion::SuggestionType;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Mutex;

    fn make_suggestion(id: &str, priority: Priority) -> Suggestion {
        Suggestion {
            suggestion_id: id.to_string(),
            suggestion_type: SuggestionType::WorkGuidance,
            content: format!("suggestion {id}"),
            priority,
            confidence_score: 0.9,
            relevance_score: 0.8,
            is_actionable: true,
            created_at: Utc::now(),
            expires_at: None,
            source: Default::default(),
            reasoning: None,
            context_scope: None,
        }
    }

    #[tokio::test]
    async fn funnel_emits_on_changed_with_queue_len_once() {
        let queue = Arc::new(Mutex::new(maekon_suggestion::queue::SuggestionQueue::new(
            10,
        )));
        let calls = AtomicUsize::new(0);
        let last_count = AtomicUsize::new(0);
        let cb = |c: usize| {
            calls.fetch_add(1, Ordering::SeqCst);
            last_count.store(c, Ordering::SeqCst);
        };
        let items = vec![
            make_suggestion("s1", Priority::Medium),
            make_suggestion("s2", Priority::High),
        ];
        enqueue_and_surface(&queue, items, Some(&cb), None, false).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one emit per batch");
        assert_eq!(
            last_count.load(Ordering::SeqCst),
            2,
            "emit carries queue len"
        );
    }

    #[tokio::test]
    async fn funnel_skips_emit_when_all_duplicates() {
        let queue = Arc::new(Mutex::new(maekon_suggestion::queue::SuggestionQueue::new(
            10,
        )));
        let s = make_suggestion("dup", Priority::High);
        enqueue_and_surface(&queue, vec![s.clone()], None, None, false).await;

        let calls = AtomicUsize::new(0);
        let cb = |_c: usize| {
            calls.fetch_add(1, Ordering::SeqCst);
        };
        // Same content → fingerprint dedup rejects → no accepted → no emit.
        enqueue_and_surface(&queue, vec![s], Some(&cb), None, false).await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "duplicate-only batch must not emit a refresh"
        );
    }
}
