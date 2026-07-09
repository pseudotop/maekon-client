use std::collections::HashMap;
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
    content_bucket: String,
    baseline: WorkType,
}

impl CacheKey {
    fn new(app_name: &str, window_title: &str, baseline: WorkType) -> Self {
        Self {
            app_name: app_name.to_string(),
            content_bucket: coarse_content_bucket(window_title),
            baseline,
        }
    }
}

fn coarse_content_bucket(window_title: &str) -> String {
    let mut normalized = String::new();
    let mut last_was_space = false;

    for ch in window_title.chars() {
        let mapped = if ch.is_ascii_digit() {
            '#'
        } else if ch.is_ascii_alphabetic() {
            ch.to_ascii_lowercase()
        } else if ch.is_ascii_alphanumeric() {
            ch
        } else {
            ' '
        };

        if mapped.is_whitespace() {
            if !last_was_space {
                normalized.push(' ');
                last_was_space = true;
            }
        } else {
            normalized.push(mapped);
            last_was_space = false;
        }
    }

    let bucket = normalized
        .split_whitespace()
        .take(6)
        .collect::<Vec<_>>()
        .join(" ");

    if bucket.is_empty() {
        "untitled".to_string()
    } else {
        bucket
    }
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
    /// F-RR-C28-03/#7485: handles to background LLM prefetch tasks keyed by a
    /// stable context bucket. Calling abort() on Drop prevents orphaned tasks.
    prefetch_handles: Arc<Mutex<HashMap<CacheKey, Option<JoinHandle<()>>>>>,
}

/// F-RR-C28-03: cancels in-flight prefetch tasks on Drop.
/// abort() is a no-op on an already-completed task, so this is also safe during
/// clean shutdown.
impl Drop for LlmWorkTypeRefiner {
    fn drop(&mut self) {
        // Use try_lock on the Mutex in a sync context (Drop is a sync context)
        if let Ok(mut guard) = self.prefetch_handles.try_lock() {
            for (_key, handle) in guard.drain() {
                let Some(handle) = handle else {
                    continue;
                };
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
                std::num::NonZeroUsize::new(CACHE_CAPACITY).unwrap_or_else(|| panic!("nonzero")),
            ))),
            pii_sanitizer: None,
            pii_level: PiiFilterLevel::Standard,
            prefetch_handles: Arc::new(Mutex::new(HashMap::new())),
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
        let key = CacheKey::new(app_name, window_title, baseline);

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

        // Cache miss — spawn one background prefetch per stable key. A second
        // tick for the same key keeps the first task alive so it can warm cache.
        {
            let mut handles = self.prefetch_handles.lock().await;
            if handles.contains_key(&key) {
                return None;
            }
            handles.insert(key.clone(), None);
        }

        let provider = self.provider.clone();
        let cache = self.cache.clone();
        let key_clone = key.clone();
        let cleanup_key = key.clone();
        let prefetch_handles = self.prefetch_handles.clone();
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
            let mut handles = prefetch_handles.lock().await;
            handles.remove(&cleanup_key);
        });

        let mut handles = self.prefetch_handles.lock().await;
        if let Some(slot) = handles.get_mut(&key) {
            *slot = Some(handle);
        } else {
            handle.abort();
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
    // #6926: guard start <= end before slicing. A model-controlled response with a
    // closing brace BEFORE an opening one (e.g. "} ... {", reachable when the LLM
    // echoes screen-captured window_title/ocr_sample embedded in the prompt) gives
    // start > end and `&response[start..end]` PANICS. The three sibling JSON
    // extractors all guard this (daily_insight_generator / ai_llm_client::parsers /
    // ai_ocr_client::parsers — the #6194 reversed-range panic class).
    if start >= end {
        return None;
    }
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

    /// #6926 regression guard: a model-controlled response with a closing brace
    /// BEFORE an opening one yields start > end. Pre-fix `&response[start..end]`
    /// PANICKED; the guard must return None instead (no panic). brace-free input
    /// hits the earlier `?` and never reaches the slice, so these inputs MUST
    /// contain `}` before `{` to exercise the reversed-range path.
    #[test]
    fn parse_reversed_braces_returns_none_not_panic() {
        // start=index_of('{'), end=rindex_of('}')+1; here '}' precedes '{'.
        assert!(parse_response("} {").is_none());
        assert!(parse_response("Sorry I can't } classify this {").is_none());
        // A closing brace with a later opening brace and trailing text.
        assert!(parse_response("garbage } more { junk").is_none());
    }

    #[test]
    fn parse_unknown_work_type_uses_default() {
        let resp = r#"{"work_type": "SOMETHING_NEW", "confidence": 0.9}"#;
        let parsed = parse_response(resp).unwrap();
        assert_eq!(parsed.work_type, WorkType::Unknown);
    }

    #[test]
    fn cache_key_equality() {
        let k1 = CacheKey::new("VSCode", "main.rs", WorkType::ActiveCoding);
        let k2 = CacheKey::new("VSCode", "main.rs", WorkType::ActiveCoding);
        assert_eq!(k1, k2);
    }

    #[test]
    fn cache_key_ignores_volatile_title_counts() {
        let k1 = CacheKey::new("Gmail", "Inbox (3) | Gmail", WorkType::Browsing);
        let k2 = CacheKey::new("Gmail", "Inbox (4) | Gmail", WorkType::Browsing);
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
        use maekon_core::ports::pii_sanitizer::FakePiiSanitizer;

        // This test only asserts builder wiring, not sanitize_text output —
        // no replacements are registered (identity passthrough).
        let refiner = LlmWorkTypeRefiner::new(Arc::new(NoOpAnalysisProvider))
            .with_pii_sanitizer(Arc::new(FakePiiSanitizer::new()), PiiFilterLevel::Strict);
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

    #[tokio::test]
    async fn refine_deduplicates_in_flight_stable_key() {
        use async_trait::async_trait;
        use maekon_core::error::CoreError;
        use maekon_core::models::suggestion::Suggestion;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::sync::Notify;

        struct BlockingProvider {
            calls: AtomicUsize,
            release: Notify,
        }

        #[async_trait]
        impl AnalysisProvider for BlockingProvider {
            async fn analyze(
                &self,
                _context_json: &str,
                _system_prompt: &str,
            ) -> Result<Vec<Suggestion>, CoreError> {
                Ok(Vec::new())
            }

            async fn summarize_text(
                &self,
                _context_json: &str,
                _system_prompt: &str,
            ) -> Result<String, CoreError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.release.notified().await;
                Ok(r#"{"work_type": "WRITING", "confidence": 0.91}"#.into())
            }

            fn provider_name(&self) -> &str {
                "blocking"
            }
        }

        let provider = Arc::new(BlockingProvider {
            calls: AtomicUsize::new(0),
            release: Notify::new(),
        });
        let refiner = LlmWorkTypeRefiner::new(provider.clone());

        assert!(refiner
            .refine(
                WorkType::Browsing,
                "Gmail",
                "Inbox (3) | Gmail",
                None,
                None,
                0.0,
            )
            .await
            .is_none());
        while provider.calls.load(Ordering::SeqCst) < 1 {
            tokio::task::yield_now().await;
        }

        assert!(refiner
            .refine(
                WorkType::Browsing,
                "Gmail",
                "Inbox (4) | Gmail",
                None,
                None,
                0.0,
            )
            .await
            .is_none());
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }

        assert_eq!(
            provider.calls.load(Ordering::SeqCst),
            1,
            "same stable key must keep one in-flight LLM prefetch"
        );
        provider.release.notify_waiters();
    }

    /// F-RR-C28-03/F-QA-C29-01: verify that tracked prefetch handles are aborted on Drop.
    /// Capture `abort_handle()` up front, then assert `is_finished()` after the
    /// abort propagates.
    #[tokio::test]
    async fn drop_aborts_prefetch_handles() {
        use crate::fallback_analysis_provider::NoOpAnalysisProvider;
        use tokio::sync::oneshot;

        let (_tx, rx) = oneshot::channel::<()>();
        let refiner = LlmWorkTypeRefiner::new(Arc::new(NoOpAnalysisProvider));

        // Spawn a task that waits forever
        let long_task = tokio::spawn(async move {
            let _ = rx.await;
        });
        // Capture the abort_handle up front (the JoinHandle is moved into
        // prefetch_handles)
        let abort_handle = long_task.abort_handle();

        // Inject the JoinHandle into prefetch_handles
        refiner
            .prefetch_handles
            .try_lock()
            .expect("lock available")
            .insert(
                CacheKey::new("VSCode", "main.rs", WorkType::ActiveCoding),
                Some(long_task),
            );

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

    /// #7485: verify that an already in-flight key is treated as a cache-warming
    /// prefetch instead of spawning and aborting a duplicate LLM request.
    #[tokio::test]
    async fn duplicate_in_flight_key_returns_without_new_handle() {
        use crate::fallback_analysis_provider::NoOpAnalysisProvider;

        let refiner = Arc::new(LlmWorkTypeRefiner::new(Arc::new(NoOpAnalysisProvider)));
        let key = CacheKey::new("VSCode", "main.rs", WorkType::Unknown);
        refiner.prefetch_handles.lock().await.insert(key, None);

        // Call refine() with the same key. It must observe the in-flight marker
        // and return without inserting a duplicate handle.
        let result = refiner
            .refine(WorkType::Unknown, "VSCode", "main.rs", None, None, 0.0)
            .await;
        assert!(result.is_none());
        assert_eq!(refiner.prefetch_handles.lock().await.len(), 1);
    }
}
