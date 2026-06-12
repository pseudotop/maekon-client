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
    /// F-RR-C28-03: 백그라운드 LLM 프리페치 태스크 핸들.
    /// Drop 시 abort() 호출로 고아 태스크 방지 (cycle 26 #3733/#3749 패턴).
    prefetch_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
}

/// F-RR-C28-03: Drop 시 진행 중인 프리페치 태스크를 취소.
/// abort()는 이미 완료된 태스크에서 no-op이므로 클린 셧다운에도 안전.
impl Drop for LlmWorkTypeRefiner {
    fn drop(&mut self) {
        // Mutex는 sync context에서 try_lock 사용 (Drop은 sync 컨텍스트)
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

        // F-RR-C28-03: JoinHandle을 prefetch_handle에 저장하여 Drop 시 abort 보장.
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

    /// F-RR-C28-03/F-QA-C29-01: Drop 시 prefetch_handle이 abort되는지 검증.
    /// `abort_handle()`를 미리 확보하여 abort 전파 후 `is_finished()` 검증.
    #[tokio::test]
    async fn drop_aborts_prefetch_handle() {
        use crate::fallback_analysis_provider::NoOpAnalysisProvider;
        use tokio::sync::oneshot;

        let (_tx, rx) = oneshot::channel::<()>();
        let refiner = LlmWorkTypeRefiner::new(Arc::new(NoOpAnalysisProvider));

        // 무한 대기 태스크 스폰
        let long_task = tokio::spawn(async move {
            let _ = rx.await;
        });
        // abort_handle을 미리 확보 (JoinHandle은 prefetch_handle로 move됨)
        let abort_handle = long_task.abort_handle();

        // JoinHandle을 prefetch_handle에 주입
        *refiner.prefetch_handle.try_lock().expect("lock available") = Some(long_task);

        // Drop 시 Drop impl이 try_lock 후 handle.abort() 호출
        drop(refiner);

        // abort가 스케줄러로 전파될 시간 확보
        tokio::task::yield_now().await;

        // F-QA-C29-01: cycle 28 #3824 무-assertion 테스트 보강.
        // Drop impl이 abort()를 호출했다면 abort_handle.is_finished() == true.
        assert!(
            abort_handle.is_finished(),
            "Drop impl must abort the tracked prefetch handle"
        );
    }

    /// F-PF-C29-02/F-RR-C29-01/F-QA-C30-03: concurrent refine() 호출 시 try_lock 경합이
    /// panic이 아닌 graceful abort-new 정책으로 처리되는지 검증.
    /// (try_lock().expect() 가 살아있다면 이 테스트가 panic으로 실패함)
    ///
    /// F-QA-C30-03: Err arm 에서 spawned handle 이 실제로 abort 되었는지 검증.
    /// lock_guard 해제 후 prefetch_handle 이 None 인지 확인 (abort path 는 handle 을
    /// prefetch_handle 에 저장하지 않으므로 guard 취득 후 None 이어야 함).
    #[tokio::test]
    async fn concurrent_try_lock_does_not_panic() {
        use crate::fallback_analysis_provider::NoOpAnalysisProvider;

        let refiner = Arc::new(LlmWorkTypeRefiner::new(Arc::new(NoOpAnalysisProvider)));

        // prefetch_handle Mutex를 외부에서 잡아 try_lock이 항상 WouldBlock 반환하게 만듦
        let lock_guard = refiner.prefetch_handle.clone().lock_owned().await;

        // 이 상태에서 refine() 호출 — 내부 try_lock이 WouldBlock이지만 panic 없이
        // handle을 abort하고 None을 반환해야 함
        let result = refiner
            .refine(WorkType::Unknown, "VSCode", "main.rs", None, None, 0.0)
            .await;
        // 캐시 미스라 None 반환 (기존 contract)
        assert!(result.is_none());

        // F-QA-C30-03: lock_guard 를 해제하여 prefetch_handle Mutex 를 돌려준다.
        // Err arm 은 handle.abort() 를 호출하고 prefetch_handle 에 저장하지 않으므로
        // guard 취득 후 내부 값이 None 이어야 한다 (abort path 검증).
        drop(lock_guard);

        // abort 전파 대기
        tokio::task::yield_now().await;

        // Err arm 이 prefetch_handle 에 handle 을 저장하지 않았음을 검증
        // (Ok arm 은 Some(handle) 을 저장하므로 Err/Ok 경로가 구별됨)
        let guard = refiner.prefetch_handle.lock().await;
        assert!(
            guard.is_none(),
            "F-QA-C30-03: Err arm must NOT store handle in prefetch_handle              — it aborts immediately without storing. If Some, abort path was bypassed."
        );
    }
}
