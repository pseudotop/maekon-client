//! Event-driven LLM analysis helper (context analyzer) + the shared
//! suggestion surfacing funnel.

use std::sync::Arc;

use tracing::{debug, info, warn};

use maekon_core::config_manager::ConfigManager;
use maekon_core::error::CoreError;
use maekon_core::models::suggestion::{Priority, Suggestion};
use maekon_core::models::tiered_memory::Regime;
use maekon_core::ports::consent_manager::{ConsentGate, ConsentManagerPort};
use maekon_core::ports::storage::StorageService;
use maekon_core::ports::vision::{CaptureAccess, CapturePermit};

use crate::notification_manager::NotificationManager;

/// Learned + static relevance gates applied UNIFORMLY at the shared surfacing
/// seam, so every LOCAL suggestion producer (rule, periodic-LLM, event-driven,
/// focus/idle-resume) runs the SAME decision function (#7914).
///
/// Before #7914 the three gates were scattered: `FeedbackScorer::adjust` ran
/// only on the server SSE path (`SuggestionReceiver::handle_suggestion`), the
/// learned per-regime acceptance gate only on the event-driven producer, and
/// the periodic-LLM/rule/idle-resume producers applied only the static
/// Deep-Focus `filter_by_regime`. Rejecting a rule-based nudge 10× therefore
/// never suppressed it locally even though the identical signal suppressed
/// server suggestions. Bundling the inputs here and consuming them in
/// [`enqueue_and_surface`] closes that gap.
///
/// Every field is "no signal ⇒ pass-through": an unwired producer, an OSS build
/// without a scorer, or a regime with too few learned samples behaves exactly
/// as before. This mirrors the `SchedulerRequiredDeps`/two-payload taste (named
/// fields, no `Default` escape hatch beyond the explicit [`RelevanceGates::pass_through`]).
pub(crate) struct RelevanceGates<'a> {
    /// FeedbackScorer-driven per (`suggestion_type`, `source`) relevance
    /// adjustment + suppression — the SAME `FeedbackScorer::adjust` the server
    /// SSE path applies, so the scorer decision lives in ONE place across both
    /// streams. `None` ⇒ no scorer wired (kept usable from tests / builds
    /// without the suggestion pipeline). #7913 (T2.1) will back this handle
    /// with persisted state; the plumbing here is deliberately
    /// persistence-agnostic so that swap is transparent.
    pub scorer: Option<&'a Arc<tokio::sync::Mutex<maekon_suggestion::scorer::FeedbackScorer>>>,
    /// Current activity regime for the static Deep-Focus filter
    /// (`filter_by_regime`). `None` ⇒ pass-through. This is a UX posture, NOT a
    /// learned gate — the learned gates below compose WITH it, never replace it.
    pub regime: Option<&'a Regime>,
    /// Learned per-regime acceptance rate (`RegimeClassifier::acceptance_rate`)
    /// for `regime`. `None` ⇒ pass-through (reader unwired, or below the
    /// classifier's minimum sample floor). See
    /// [`maekon_analysis::apply_regime_acceptance_gate`].
    pub regime_acceptance_rate: Option<f64>,
}

impl RelevanceGates<'_> {
    /// Pass-through gates — no scorer, no regime signal; every field is the
    /// "no signal ⇒ no suppression" default. Test-only: production producers
    /// always build real gates via [`relevance_gates`], so gating this
    /// `#[cfg(test)]` keeps it out of the shipped dead-code surface.
    #[cfg(test)]
    pub(crate) fn pass_through() -> Self {
        Self {
            scorer: None,
            regime: None,
            regime_acceptance_rate: None,
        }
    }
}

/// Look up the learned per-regime acceptance rate for `regime` from the
/// adaptive trigger state's `RegimeClassifier`. `None` when there is no active
/// regime, no trigger state (e.g. analysis-disabled builds), or the classifier
/// has fewer than its minimum reaction samples for this regime. Read-only: the
/// `parking_lot` lock is taken and released synchronously with no `.await`
/// held, so it is safe to call anywhere on the scheduler hot path.
pub(crate) fn regime_acceptance_rate(
    regime: Option<&Regime>,
    adaptive_trigger_state: Option<&crate::scheduler::AdaptiveTriggerState>,
) -> Option<f64> {
    let regime = regime?;
    adaptive_trigger_state?
        .regime_classifier
        .lock()
        .acceptance_rate(&regime.regime_id)
}

/// Build [`RelevanceGates`] for a monitor-loop producer path in one call —
/// keeps the acceptance-rate lookup out of `spawn_monitor_loop`'s LOC-capped
/// body (put plumbing in helpers, not inline expansions). The returned gates
/// borrow `scorer`/`regime`; `adaptive_trigger_state` is only read to compute
/// the owned acceptance rate, so its lifetime is independent.
pub(crate) fn relevance_gates<'a>(
    scorer: Option<&'a Arc<tokio::sync::Mutex<maekon_suggestion::scorer::FeedbackScorer>>>,
    regime: Option<&'a Regime>,
    adaptive_trigger_state: Option<&crate::scheduler::AdaptiveTriggerState>,
) -> RelevanceGates<'a> {
    RelevanceGates {
        scorer,
        regime,
        regime_acceptance_rate: regime_acceptance_rate(regime, adaptive_trigger_state),
    }
}

/// Apply the learned + static relevance gates to `suggestions` in the fixed
/// compose order (#7914). Each gate can only SHRINK the set — none reorders or
/// re-admits — so the order is a pure conjunction. Suppression is made
/// OBSERVABLE (groundwork for T1.6 "quieted because you rejected N similar"):
/// every gate that drops anything emits a `debug!` line naming the reason
/// (`focus` / `regime` / `scorer`). No UI is built here.
async fn apply_relevance_gates(
    mut suggestions: Vec<Suggestion>,
    gates: &RelevanceGates<'_>,
    original: Option<&CapturePermit>,
) -> Result<Vec<Suggestion>, CoreError> {
    // 1. Static Deep-Focus regime filter (UX posture, NOT a learned gate). The
    //    learned gates below compose WITH it.
    let before = suggestions.len();
    maekon_analysis::filter_by_regime(&mut suggestions, gates.regime);
    if suggestions.len() < before {
        debug!(
            dropped = before - suggestions.len(),
            gate = "focus",
            "local suggestions suppressed by Deep-Focus regime filter"
        );
    }

    // 2. Learned per-regime acceptance gate (#7600) — quiets regimes the user
    //    has historically rejected suggestions in.
    let before = suggestions.len();
    maekon_analysis::apply_regime_acceptance_gate(&mut suggestions, gates.regime_acceptance_rate);
    if suggestions.len() < before {
        debug!(
            dropped = before - suggestions.len(),
            gate = "regime",
            acceptance_rate = ?gates.regime_acceptance_rate,
            "local suggestions suppressed by learned per-regime acceptance gate"
        );
    }

    // 3. Learned per (type, source) FeedbackScorer gate (#7914 headline). Shares
    //    `FeedbackScorer::adjust` with the server SSE path so the scorer logic
    //    lives in ONE place. In OSS/default (no `server`) builds this is the
    //    first time the scorer READS on a local path — before #7914 it was
    //    write-only off the SSE stream.
    if let Some(scorer) = gates.scorer {
        let scorer = scorer.lock().await;
        check_capture_origin(original)?;
        suggestions.retain_mut(|s| {
            let keep = scorer.adjust(&s.suggestion_type, &s.source, &mut s.relevance_score);
            if !keep {
                debug!(
                    suggestion_id = %s.suggestion_id,
                    relevance = s.relevance_score,
                    gate = "scorer",
                    "local suggestion suppressed — learned relevance below threshold"
                );
            }
            keep
        });
    }

    Ok(suggestions)
}

/// Shared surfacing funnel (#5694): apply the learned + static relevance gates
/// (#7914), then push survivors into the live review queue (fingerprint dedup)
/// and make them DISCOVERABLE — emit the `overlay:suggestions-changed` refresh
/// via `on_changed` when anything was accepted, and toast the highest-priority
/// accepted suggestion (High and above) unless focus mode is active (A4 gate).
///
/// Gating (`gates`) is applied HERE, at the single seam, rather than at each
/// producer call site, so the periodic analysis loop, the event-driven path,
/// the rule producer, and the focus/idle-resume path all inherit ONE identical
/// decision function ([`apply_relevance_gates`]) — see [`RelevanceGates`].
/// Notification policy (cooldown / Critical bypass / master switch) lives in
/// [`NotificationManager::notify_suggestion`], so every producer inherits one
/// consistent policy there too.
pub(crate) async fn enqueue_and_surface(
    queue: &Arc<tokio::sync::Mutex<maekon_suggestion::queue::SuggestionQueue>>,
    to_enqueue: Vec<Suggestion>,
    gates: RelevanceGates<'_>,
    on_changed: Option<&(dyn Fn(usize) + Send + Sync)>,
    notifier: Option<&Arc<NotificationManager>>,
    focus_active: bool,
) -> usize {
    // None preserves the legacy funnel; only an original permit can deny it.
    enqueue_and_surface_with_capture_origin(
        queue,
        to_enqueue,
        gates,
        on_changed,
        notifier,
        focus_active,
        None,
    )
    .await
    .unwrap_or_default()
}

/// Preserve the original capture authority through every surfacing await.
/// Effects already committed before revocation are not rolled back.
pub(crate) async fn enqueue_and_surface_with_capture_origin(
    queue: &Arc<tokio::sync::Mutex<maekon_suggestion::queue::SuggestionQueue>>,
    to_enqueue: Vec<Suggestion>,
    gates: RelevanceGates<'_>,
    on_changed: Option<&(dyn Fn(usize) + Send + Sync)>,
    notifier: Option<&Arc<NotificationManager>>,
    focus_active: bool,
    original: Option<&CapturePermit>,
) -> Result<usize, CoreError> {
    check_capture_origin(original)?;
    // #7914: uniform learned + static relevance gating for every LOCAL producer.
    let to_enqueue = apply_relevance_gates(to_enqueue, &gates, original).await?;
    if to_enqueue.is_empty() {
        return Ok(0);
    }

    let mut accepted = 0usize;
    let mut top: Option<Suggestion> = None;
    let count = {
        let mut q = queue.lock().await;
        check_capture_origin(original)?;
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
        return Ok(0);
    }
    check_capture_origin(original)?;
    if let Some(cb) = on_changed {
        cb(count);
    }
    check_capture_origin(original)?;
    if let (Some(nm), Some(t)) = (notifier, top) {
        if t.priority >= Priority::High && !focus_active {
            nm.notify_suggestion(&t).await;
            check_capture_origin(original)?;
        }
    }
    Ok(accepted)
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
    consent_manager: Option<&Arc<dyn ConsentManagerPort>>,
    config_manager: Option<&ConfigManager>,
    fallback_analysis_enabled: bool,
    // E20-24 (#4816): live suggestion queue (the manager's Arc). `None` when the
    // local-suggestions pipeline is absent; `Some` enqueues for live review.
    suggestion_queue: Option<&Arc<tokio::sync::Mutex<maekon_suggestion::queue::SuggestionQueue>>>,
    // #7914: learned + static relevance gates (regime Deep-Focus filter +
    // learned per-regime acceptance + FeedbackScorer). Applied inside
    // `enqueue_and_surface`, the same seam every other LOCAL producer flows
    // through. `RelevanceGates::pass_through()` disables all gating.
    gates: RelevanceGates<'_>,
    // #5694: overlay auto-refresh + desktop toast for accepted suggestions.
    on_changed: Option<&(dyn Fn(usize) + Send + Sync)>,
    notifier: Option<&Arc<NotificationManager>>,
    focus_active: bool,
) -> crate::local_analysis_status::LocalAnalysisStatus {
    handle_event_analysis_with_capture_origin(
        analyzer,
        storage,
        app_name,
        window_title,
        ocr_hint,
        consent_manager,
        config_manager,
        fallback_analysis_enabled,
        suggestion_queue,
        gates,
        on_changed,
        notifier,
        focus_active,
        None,
    )
    .await
}

/// Capture-derived results keep the permit issued with their original frame.
/// This precursor does not install a runtime capture authority or caller.
#[tracing::instrument(skip_all)]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_event_analysis_with_capture_origin(
    analyzer: &Option<Arc<maekon_analysis::ContextAnalyzer>>,
    storage: &Arc<dyn StorageService>,
    app_name: &str,
    window_title: &str,
    ocr_hint: Option<&str>,
    consent_manager: Option<&Arc<dyn ConsentManagerPort>>,
    config_manager: Option<&ConfigManager>,
    fallback_analysis_enabled: bool,
    // E20-24 (#4816): live suggestion queue (the manager's Arc). `None` when the
    // local-suggestions pipeline is absent; `Some` enqueues for live review.
    suggestion_queue: Option<&Arc<tokio::sync::Mutex<maekon_suggestion::queue::SuggestionQueue>>>,
    // #7914: learned + static relevance gates (regime Deep-Focus filter +
    // learned per-regime acceptance + FeedbackScorer). Applied inside
    // `enqueue_and_surface`, the same seam every other LOCAL producer flows
    // through. `RelevanceGates::pass_through()` disables all gating.
    gates: RelevanceGates<'_>,
    // #5694: overlay auto-refresh + desktop toast for accepted suggestions.
    on_changed: Option<&(dyn Fn(usize) + Send + Sync)>,
    notifier: Option<&Arc<NotificationManager>>,
    focus_active: bool,
    original: Option<&CapturePermit>,
) -> crate::local_analysis_status::LocalAnalysisStatus {
    use crate::local_analysis_status::{
        missing_local_analysis_permissions, LocalAnalysisProducer, LocalAnalysisStatus,
        LocalAnalysisStatusKind,
    };
    use maekon_analysis::AnalysisRunOutcome;

    let producer = LocalAnalysisProducer::AppSwitch;
    let initial_queue_count = match suggestion_queue {
        Some(queue) => queue.lock().await.len(),
        None => 0,
    };
    let denied =
        |error: CoreError| LocalAnalysisStatus::from_error(&error, producer, initial_queue_count);
    if let Err(error) = check_capture_origin(original) {
        return denied(error);
    }
    // Re-read the user-controlled feature switch at the same final boundary as
    // consent. The monitor tick performs several awaits before reaching this
    // helper, so its initial config snapshot is not current authorization.
    let live_config = super::super::analysis_invocation_guard::analysis_config_now(
        config_manager,
        fallback_analysis_enabled,
    );
    if !live_config.enabled {
        return LocalAnalysisStatus::new(
            LocalAnalysisStatusKind::PolicyBlocked,
            "analysis_disabled",
            producer,
            0,
            initial_queue_count,
        );
    }
    // Read the authority after the queue-count await and immediately before
    // analyzer invocation. A tick-level snapshot can outlive a consent
    // withdrawal while OCR is still being processed (#11737).
    let permissions = ConsentGate::from_ref(consent_manager).permissions_snapshot();
    let missing = missing_local_analysis_permissions(&permissions);
    if !missing.is_empty() {
        return LocalAnalysisStatus::consent_required(producer, missing, initial_queue_count);
    }
    let Some(analyzer) = analyzer else {
        return LocalAnalysisStatus::new(
            LocalAnalysisStatusKind::ProviderOffline,
            "provider_unavailable",
            producer,
            0,
            initial_queue_count,
        );
    };

    let outcome = match original {
        Some(permit) => {
            analyzer
                .on_significant_event_with_original_capture_outcome(
                    app_name,
                    window_title,
                    ocr_hint,
                    permit,
                )
                .await
        }
        None => {
            analyzer
                .on_significant_event_with_outcome(app_name, window_title, ocr_hint)
                .await
        }
    };
    if let Err(error) = check_capture_origin(original) {
        return denied(error);
    }
    match outcome {
        Ok(AnalysisRunOutcome::Generated(suggestions)) => {
            let candidate_count = suggestions.len();
            for s in &suggestions {
                if let Err(error) = check_capture_origin(original) {
                    return denied(error);
                }
                info!(
                    id = %s.suggestion_id,
                    priority = ?s.priority,
                    content = %maekon_monitor::log_privacy::content_digest(&s.content),
                    "event-driven suggestion produced"
                );
                if let Err(e) = storage.save_suggestion(s).await {
                    warn!("suggestion save failure: {e}");
                }
                // A save already entered may have committed; only stop later effects.
                if let Err(error) = check_capture_origin(original) {
                    return denied(error);
                }
            }
            // E20-24 (#4816): enqueue for live review (same producer wire as the
            // periodic analysis loop). Dedup via queue fingerprint (push -> bool).
            // #7914: regime + learned per-regime acceptance + FeedbackScorer
            // gating now all happen inside `enqueue_and_surface` (one seam,
            // one decision function), so this path applies the SAME gates as
            // every other LOCAL producer instead of a bespoke subset.
            let Some(queue) = suggestion_queue else {
                return LocalAnalysisStatus::new(
                    LocalAnalysisStatusKind::PolicyBlocked,
                    "suggestion_queue_unavailable",
                    producer,
                    candidate_count,
                    0,
                );
            };
            let admitted = enqueue_and_surface_with_capture_origin(
                queue,
                suggestions,
                gates,
                on_changed,
                notifier,
                focus_active,
                original,
            )
            .await;
            let admitted = match admitted {
                Ok(admitted) => admitted,
                Err(error) => return denied(error),
            };
            let queue_count = queue.lock().await.len();
            if let Err(error) = check_capture_origin(original) {
                return LocalAnalysisStatus::from_error(&error, producer, queue_count);
            }
            if admitted == 0 {
                LocalAnalysisStatus::new(
                    LocalAnalysisStatusKind::NoCandidate,
                    "not_admitted",
                    producer,
                    candidate_count,
                    queue_count,
                )
            } else {
                LocalAnalysisStatus::new(
                    LocalAnalysisStatusKind::Generated,
                    "generated",
                    producer,
                    candidate_count,
                    queue_count,
                )
            }
        }
        Ok(AnalysisRunOutcome::NoCandidate) => LocalAnalysisStatus::new(
            LocalAnalysisStatusKind::NoCandidate,
            "no_valid_candidate",
            producer,
            0,
            initial_queue_count,
        ),
        Ok(AnalysisRunOutcome::Throttled) => LocalAnalysisStatus::new(
            LocalAnalysisStatusKind::Throttled,
            "analysis_throttled",
            producer,
            0,
            initial_queue_count,
        ),
        Ok(AnalysisRunOutcome::NoInput) => LocalAnalysisStatus::new(
            LocalAnalysisStatusKind::NoCandidate,
            "no_input",
            producer,
            0,
            initial_queue_count,
        ),
        Ok(AnalysisRunOutcome::Unchanged) => LocalAnalysisStatus::new(
            LocalAnalysisStatusKind::NoCandidate,
            "context_unchanged",
            producer,
            0,
            initial_queue_count,
        ),
        Err(error) => {
            let core: maekon_core::error::CoreError = error.into();
            debug!(err.code = %core.code(), "event analysis unavailable");
            LocalAnalysisStatus::from_error(&core, producer, initial_queue_count)
        }
    }
}

/// A fresh grant cannot authorize capture data from a revoked epoch.
fn check_capture_origin(original: Option<&CapturePermit>) -> Result<(), CoreError> {
    if let Some(original) = original {
        original.check(CaptureAccess::Retain)?;
        original.check(CaptureAccess::Ocr)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
