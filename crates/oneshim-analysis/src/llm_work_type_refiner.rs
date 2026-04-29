use std::sync::Arc;
use std::time::Instant;

use lru::LruCache;
use oneshim_core::config::PiiFilterLevel;
use oneshim_core::models::tiered_memory::WorkType;
use oneshim_core::ports::analysis_provider::AnalysisProvider;
use oneshim_core::ports::pii_sanitizer::PiiSanitizer;
use oneshim_core::sanitized;
use serde::Deserialize;
use tokio::sync::Mutex;
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

        tokio::spawn(async move {
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
        use oneshim_core::config::PiiFilterLevel;

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
}
