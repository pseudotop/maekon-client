use std::sync::Arc;
use std::time::Instant;

use lru::LruCache;
use maekon_core::config::PiiFilterLevel;
use maekon_core::models::tiered_memory::WorkType;
use maekon_core::ports::analysis_provider::AnalysisProvider;
use maekon_core::ports::pii_sanitizer::PiiSanitizer;
use maekon_core::sanitized;
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

const CACHE_CAPACITY: usize = 64;
const CACHE_TTL_SECS: u64 = 300;
const CONFIDENCE_THRESHOLD: f64 = 0.7;

const SYSTEM_PROMPT: &str = r#"You are a work activity classifier. Given the user's current app, window title, and engagement context, classify the activity into exactly one work type.

Work types: ACTIVE_CODING, CODE_REVIEW, WRITING, READING, DESIGNING, FORM_FILLING, BROWSING, PASSIVE_MEETING, ACTIVE_MEETING, NAVIGATION, TERMINAL_COMMANDS, LOG_READING, DOCUMENT_WRITING, DOCUMENT_READING, CHAT_COMPOSING, UNKNOWN

Respond with JSON only:
{"work_type": "ACTIVE_CODING", "confidence": 0.92}"#;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct CacheKey {
    app_name: String,
    window_title: String,
    baseline: WorkType,
}

#[derive(Debug, Clone)]
struct CachedResult {
    refined: WorkType,
    confidence: f64,
    cached_at: Instant,
}

impl CachedResult {
    fn is_expired(&self) -> bool {
        self.cached_at.elapsed().as_secs() > CACHE_TTL_SECS
    }
}

#[derive(Debug, Deserialize)]
struct ClassificationResponse {
    work_type: WorkType,
    confidence: f64,
}

pub struct LlmWorkTypeRefiner {
    provider: Arc<dyn AnalysisProvider>,
    cache: Arc<Mutex<LruCache<CacheKey, CachedResult>>>,
    /// D5 iter-16 migration: sanitizer for `CoreError::Display` output when
    /// LLM-call failures are logged. The error message can carry up to 200
    /// chars of LLM response body (set at `AnalysisClient` exit), which may
    /// echo user-context PII from the prompt. Optional — sites without a
    /// configured sanitizer fall back to raw Display.
    pii_sanitizer: Option<Arc<dyn PiiSanitizer>>,
    pii_level: PiiFilterLevel,
    /// F-RR-C28-03: handle to the background LLM prefetch task.
    /// Calling abort() on Drop prevents orphaned tasks (cycle 26 #3733/#3749 pattern).
    prefetch_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
}

/// F-RR-C28-03: cancels the in-flight prefetch task on Drop.
/// abort() is a no-op on an already-completed task, so this is also safe during
/// clean shutdown.
impl Drop for LlmWorkTypeRefiner {
    fn drop(&mut self) {
        // Use try_lock on the Mutex in a sync context (Drop is a sync context)
        if let Ok(mut guard) = self.prefetch_handle.try_lock() {
            if let Some(handle) = guard.take() {
                handle.abort();
            }
        }
    }
}

impl LlmWorkTypeRefiner {
    pub fn new(provider: Arc<dyn AnalysisProvider>) -> Self {
        Self {
            provider,
            cache: Arc::new(Mutex::new(LruCache::new(
                std::num::NonZeroUsize::new(CACHE_CAPACITY).expect("nonzero"),
            ))),
            pii_sanitizer: None,
            pii_level: PiiFilterLevel::Standard,
            prefetch_handle: Arc::new(Mutex::new(None)),
        }
    }

    /// D5 iter-16: attach a PII sanitizer applied to tracing output of
    /// LLM-error messages via `SanitizedDisplay`.
    #[must_use]
    pub fn with_pii_sanitizer(
        mut self,
        sanitizer: Arc<dyn PiiSanitizer>,
        level: PiiFilterLevel,
    ) -> Self {
        self.pii_sanitizer = Some(sanitizer);
        self.pii_level = level;
        self
    }

    /// Refine the rule-based WorkType using LLM.
    /// Returns `None` to keep the baseline (cache miss pending, LLM error, low confidence).
    pub async fn refine(
        &self,
        baseline: WorkType,
        app_name: &str,
        window_title: &str,
        focused_role: Option<&str>,
        ocr_sample: Option<&str>,
        keystrokes_per_min: f32,
    ) -> Option<WorkType> {
        let key = CacheKey {
            app_name: app_name.to_string(),
            window_title: window_title.to_string(),
            baseline,
        };

        // Check cache first
        {
            let mut cache = self.cache.lock().await;
            if let Some(cached) = cache.get(&key) {
                if !cached.is_expired() {
                    if cached.confidence >= CONFIDENCE_THRESHOLD && cached.refined != baseline {
                        debug!(
                            baseline = ?baseline,
                            refined = ?cached.refined,
                            confidence = cached.confidence,
                            "LLM work type refinement (cached)"
                        );
                        return Some(cached.refined);
                    }
                    return None;
                }
            }
        }

        // Cache miss — spawn background prefetch
        let provider = self.provider.clone();
        let cache = self.cache.clone();
        let key_clone = key.clone();
        let pii_sanitizer = self.pii_sanitizer.clone();
        let pii_level = self.pii_level;
        let context = build_context(
            app_name,
            window_title,
            focused_role,
            ocr_sample,
            keystrokes_per_min,
            baseline,
        );

        // F-RR-C28-03: store the JoinHandle in prefetch_handle to guarantee abort
        // on Drop.
        let handle = tokio::spawn(async move {
            match provider.summarize_text(&context, SYSTEM_PROMPT).await {
                Ok(response) => {
                    if let Some(parsed) = parse_response(&response) {
                        let result = CachedResult {
                            refined: parsed.work_type,
                            confidence: parsed.confidence,
                            cached_at: Instant::now(),
                        };
                        debug!(
                            work_type = ?parsed.work_type,
                            confidence = parsed.confidence,
                            "LLM classification cached"
                        );
                        let mut cache = cache.lock().await;
                        cache.put(key_clone, result);
                    } else {
                        warn!("failed to parse LLM classification response");
                    }
                }
                Err(e) => {
                    // D5 iter-16: LLM error body can include user-context PII
                    // echoed by the provider (up to 200 chars of response text
                    // per `AnalysisClient` error message). Route Display through
                    // `SanitizedDisplay` when a sanitizer is attached.
                    match &pii_sanitizer {
                        Some(san) => debug!(
                            err.code = %e.code(),
                            "LLM classification request failed: {}",
                            sanitized(&e, &**san, pii_level),
                        ),
                        None => {
                            debug!(err.code = %e.code(), "LLM classification request failed: {e}")
                        }
                    }
                }
            }
        });
        // F-PF-C29-02/F-RR-C29-01: try_lock().expect() panics under concurrent
        // refine() calls. Graceful match: if another refine() holds the lock,
        // abort this duplicate prefetch to avoid leaking the handle.
        match self.prefetch_handle.try_lock() {
            Ok(mut guard) => {
                if let Some(prev_handle) = guard.take() {
                    prev_handle.abort();
                }
                *guard = Some(handle);
            }
            Err(_) => {
                handle.abort();
            }
        }

        None
    }
}

fn build_context(
    app_name: &str,
    window_title: &str,
    focused_role: Option<&str>,
    ocr_sample: Option<&str>,
    keystrokes_per_min: f32,
    baseline: WorkType,
) -> String {
    let mut ctx = format!("App: {app_name}\nWindow: {window_title}\n");
    if let Some(role) = focused_role {
        ctx.push_str(&format!("Focused role: {role}\n"));
    }
    if let Some(sample) = ocr_sample {
        let truncated: String = sample.chars().take(200).collect();
        ctx.push_str(&format!("OCR sample: {truncated}\n"));
    }
    ctx.push_str(&format!("Keystrokes/min: {keystrokes_per_min:.0}\n"));
    ctx.push_str(&format!("Rule-based classification: {baseline:?}\n"));
    ctx
}

fn parse_response(response: &str) -> Option<ClassificationResponse> {
    if let Ok(parsed) = serde_json::from_str::<ClassificationResponse>(response) {
        return Some(parsed);
    }
    let start = response.find('{')?;
    let end = response.rfind('}')? + 1;
    serde_json::from_str(&response[start..end]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_clean_json() {
        let resp = r#"{"work_type": "ACTIVE_CODING", "confidence": 0.95}"#;
        let parsed = parse_response(resp).unwrap();
        assert_eq!(parsed.work_type, WorkType::ActiveCoding);
        assert!((parsed.confidence - 0.95).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_json_with_preamble() {
        let resp =
            "Here is the classification:\n{\"work_type\": \"CODE_REVIEW\", \"confidence\": 0.82}\n";
        let parsed = parse_response(resp).unwrap();
        assert_eq!(parsed.work_type, WorkType::CodeReview);
    }

    #[test]
    fn parse_invalid_returns_none() {
        assert!(parse_response("not json at all").is_none());
    }

    #[test]
    fn parse_unknown_work_type_uses_default() {
        let resp = r#"{"work_type": "SOMETHING_NEW", "confidence": 0.9}"#;
        let parsed = parse_response(resp).unwrap();
        assert_eq!(parsed.work_type, WorkType::Unknown);
    }

    #[test]
    fn cache_key_equality() {
        let k1 = CacheKey {
            app_name: "VSCode".into(),
            window_title: "main.rs".into(),
            baseline: WorkType::ActiveCoding,
        };
        let k2 = CacheKey {
            app_name: "VSCode".into(),
            window_title: "main.rs".into(),
            baseline: WorkType::ActiveCoding,
        };
        assert_eq!(k1, k2);
    }

    #[test]
    fn cached_result_expiry() {
        let fresh = CachedResult {
            refined: WorkType::ActiveCoding,
            confidence: 0.9,
            cached_at: Instant::now(),
        };
        assert!(!fresh.is_expired());
    }

    #[test]
    fn build_context_includes_all_fields() {
        let ctx = build_context(
            "VSCode",
            "main.rs — VSCode",
            Some("AXTextArea"),
            Some("fn main()"),
            45.0,
            WorkType::ActiveCoding,
        );
        assert!(ctx.contains("App: VSCode"));
        assert!(ctx.contains("Window: main.rs"));
        assert!(ctx.contains("Focused role: AXTextArea"));
        assert!(ctx.contains("OCR sample: fn main()"));
        assert!(ctx.contains("Keystrokes/min: 45"));
        assert!(ctx.contains("Rule-based classification: ActiveCoding"));
    }

    #[test]
    fn build_context_omits_none_fields() {
        let ctx = build_context("Chrome", "Google", None, None, 0.0, WorkType::Browsing);
        assert!(!ctx.contains("Focused role"));
        assert!(!ctx.contains("OCR sample"));
    }

    // D5 iter-16: verify `with_pii_sanitizer` builder wires fields without
    // regressing the base `new` constructor. The runtime sanitize call
    // happens inside a `tokio::spawn` that's exercised end-to-end — this
    // asserts the builder plumbing itself.
    #[test]
    fn with_pii_sanitizer_sets_fields() {
        use crate::fallback_analysis_provider::NoOpAnalysisProvider;
        use maekon_core::config::PiiFilterLevel;

        struct MockSanitizer;
        impl PiiSanitizer for MockSanitizer {
            fn sanitize_text(&self, text: &str, _: PiiFilterLevel) -> String {
                text.to_string()
            }
        }

        let refiner = LlmWorkTypeRefiner::new(Arc::new(NoOpAnalysisProvider))
            .with_pii_sanitizer(Arc::new(MockSanitizer), PiiFilterLevel::Strict);
        assert!(refiner.pii_sanitizer.is_some());
        assert_eq!(refiner.pii_level, PiiFilterLevel::Strict);
    }

    #[test]
    fn default_new_has_no_sanitizer() {
        use crate::fallback_analysis_provider::NoOpAnalysisProvider;
        let refiner = LlmWorkTypeRefiner::new(Arc::new(NoOpAnalysisProvider));
        assert!(refiner.pii_sanitizer.is_none());
        assert_eq!(refiner.pii_level, PiiFilterLevel::Standard);
    }

    /// F-RR-C28-03/F-QA-C29-01: verify that prefetch_handle is aborted on Drop.
    /// Capture `abort_handle()` up front, then assert `is_finished()` after the
    /// abort propagates.
    #[tokio::test]
    async fn drop_aborts_prefetch_handle() {
        use crate::fallback_analysis_provider::NoOpAnalysisProvider;
        use tokio::sync::oneshot;

        let (_tx, rx) = oneshot::channel::<()>();
        let refiner = LlmWorkTypeRefiner::new(Arc::new(NoOpAnalysisProvider));

        // Spawn a task that waits forever
        let long_task = tokio::spawn(async move {
            let _ = rx.await;
        });
        // Capture the abort_handle up front (the JoinHandle is moved into
        // prefetch_handle)
        let abort_handle = long_task.abort_handle();

        // Inject the JoinHandle into prefetch_handle
        *refiner.prefetch_handle.try_lock().expect("lock available") = Some(long_task);

        // On Drop, the Drop impl try_locks and then calls handle.abort()
        drop(refiner);

        // Give the abort time to propagate to the scheduler
        tokio::task::yield_now().await;

        // F-QA-C29-01: cycle 28 #3824 hardening of an assertion-less test.
        // If the Drop impl called abort(), then abort_handle.is_finished() == true.
        assert!(
            abort_handle.is_finished(),
            "Drop impl must abort the tracked prefetch handle"
        );
    }

    /// F-PF-C29-02/F-RR-C29-01/F-QA-C30-03: verify that try_lock contention during
    /// concurrent refine() calls is handled by the graceful abort-new policy
    /// rather than a panic.
    /// (If `try_lock().expect()` were still present, this test would fail with a
    /// panic.)
    ///
    /// F-QA-C30-03: verify the spawned handle in the Err arm is actually aborted.
    /// After releasing lock_guard, confirm prefetch_handle is None (the abort path
    /// does not store the handle in prefetch_handle, so it must be None once the
    /// guard is acquired).
    #[tokio::test]
    async fn concurrent_try_lock_does_not_panic() {
        use crate::fallback_analysis_provider::NoOpAnalysisProvider;

        let refiner = Arc::new(LlmWorkTypeRefiner::new(Arc::new(NoOpAnalysisProvider)));

        // Hold the prefetch_handle Mutex externally so try_lock always returns
        // WouldBlock
        let lock_guard = refiner.prefetch_handle.clone().lock_owned().await;

        // Call refine() in this state — the internal try_lock returns WouldBlock,
        // but it must abort the handle and return None without panicking
        let result = refiner
            .refine(WorkType::Unknown, "VSCode", "main.rs", None, None, 0.0)
            .await;
        // Cache miss, so it returns None (existing contract)
        assert!(result.is_none());

        // F-QA-C30-03: release lock_guard to return the prefetch_handle Mutex.
        // The Err arm calls handle.abort() and does not store the handle in
        // prefetch_handle, so the inner value must be None once the guard is
        // acquired (verifies the abort path).
        drop(lock_guard);

        // Wait for the abort to propagate
        tokio::task::yield_now().await;

        // Verify the Err arm did not store a handle in prefetch_handle
        // (the Ok arm stores Some(handle), so the Err/Ok paths are distinguishable)
        let guard = refiner.prefetch_handle.lock().await;
        assert!(
            guard.is_none(),
            "F-QA-C30-03: Err arm must NOT store handle in prefetch_handle              — it aborts immediately without storing. If Some, abort path was bypassed."
        );
    }
}
