use super::*;
use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU8};
use std::sync::Mutex as StdMutex;
use std::task::Poll;

use maekon_core::error_codes::PermissionCode;
use maekon_core::ports::vision::{capture_denied, CaptureGuard, CapturePauseState};
use tokio::sync::Notify;

const OCR_MARKER: &str = "ORIGINAL_CAPTURE_OCR_MARKER";

struct Origin {
    pause: Arc<CapturePauseState>,
    denied: Arc<AtomicU8>,
}

impl Origin {
    fn new() -> Self {
        Self {
            pause: Arc::new(CapturePauseState::new(Arc::new(AtomicBool::new(false)))),
            denied: Arc::new(AtomicU8::new(0)),
        }
    }

    fn permit(&self) -> CapturePermit {
        CapturePermit::new(Arc::new(OriginalGuard {
            pause: self.pause.clone(),
            epoch: self.pause.snapshot().unwrap(),
            denied: self.denied.clone(),
        }))
    }

    fn revoke_aba(&self) {
        assert!(self.pause.toggle().unwrap());
        assert!(!self.pause.toggle().unwrap());
    }
}

struct OriginalGuard {
    pause: Arc<CapturePauseState>,
    epoch: u64,
    denied: Arc<AtomicU8>,
}

impl CaptureGuard for OriginalGuard {
    fn check(&self, access: CaptureAccess) -> Result<(), CoreError> {
        self.pause.check(self.epoch)?;
        let bit = match access {
            CaptureAccess::Retain => 1,
            CaptureAccess::Ocr => 2,
            _ => 0,
        };
        if self.denied.load(Ordering::SeqCst) & bit != 0 {
            return Err(capture_denied("test capture access revoked"));
        }
        Ok(())
    }
}

#[derive(Default)]
struct Gate {
    release: Notify,
    entered: AtomicBool,
}

impl Gate {
    async fn once(&self) {
        if !self.entered.swap(true, Ordering::SeqCst) {
            self.release.notified().await;
        }
    }
}

struct WaitingStorage {
    inner: MockStorage,
    gate: Arc<Gate>,
}

#[async_trait]
impl StorageService for WaitingStorage {
    async fn save_event(&self, event: &Event) -> Result<(), CoreError> {
        self.inner.save_event(event).await
    }

    async fn get_events(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<Event>, CoreError> {
        self.gate.once().await;
        self.inner.get_events(from, to, limit).await
    }

    async fn get_pending_events(&self, limit: usize) -> Result<Vec<Event>, CoreError> {
        self.inner.get_pending_events(limit).await
    }

    async fn mark_as_sent(&self, ids: &[String]) -> Result<(), CoreError> {
        self.inner.mark_as_sent(ids).await
    }

    async fn enforce_retention(&self) -> Result<usize, CoreError> {
        self.inner.enforce_retention().await
    }

    async fn save_suggestion(&self, suggestion: &Suggestion) -> Result<(), CoreError> {
        self.inner.save_suggestion(suggestion).await
    }

    async fn save_activity_segment(
        &self,
        summary: &maekon_core::models::tiered_memory::SegmentSummary,
    ) -> Result<(), CoreError> {
        self.inner.save_activity_segment(summary).await
    }

    async fn update_segment_llm_summary(&self, id: &str, summary: &str) -> Result<(), CoreError> {
        self.inner.update_segment_llm_summary(id, summary).await
    }
}

struct ObservedProvider {
    inner: MockAnalysisProvider,
    inputs: StdMutex<Vec<String>>,
    gate: Option<Arc<Gate>>,
}

#[async_trait]
impl AnalysisProvider for ObservedProvider {
    async fn analyze(&self, context: &str, prompt: &str) -> Result<Vec<Suggestion>, CoreError> {
        self.inputs.lock().unwrap().push(context.to_owned());
        if let Some(gate) = &self.gate {
            gate.once().await;
        }
        self.inner.analyze(context, prompt).await
    }

    fn provider_name(&self) -> &str {
        "observed-existing-mock"
    }
}

fn fixture(
    storage_gate: Option<Arc<Gate>>,
    provider_gate: Option<Arc<Gate>>,
) -> (ContextAnalyzer, Arc<ObservedProvider>) {
    let mut analyzer = make_analyzer_with_throttle(3600);
    if let Some(gate) = storage_gate {
        analyzer.storage = Arc::new(WaitingStorage {
            inner: MockStorage::new(make_events(2)),
            gate,
        });
    }
    let provider = Arc::new(ObservedProvider {
        inner: MockAnalysisProvider::new(vec![make_suggestion("capture suggestion", 0.9)]),
        inputs: StdMutex::new(Vec::new()),
        gate: provider_gate,
    });
    analyzer.analysis_provider = provider.clone();
    (analyzer, provider)
}

async fn run(
    analyzer: &ContextAnalyzer,
    origin: &CapturePermit,
) -> Result<AnalysisRunOutcome, AnalysisError> {
    analyzer
        .on_significant_event_with_original_capture_outcome(
            "VSCode",
            "source.rs",
            Some(OCR_MARKER),
            origin,
        )
        .await
}

// Poll the real pipeline to its deliberately held boundary without sleeps.
async fn blocked<F: Future>(mut future: Pin<&mut F>) {
    poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
}

fn assert_denied(result: Result<AnalysisRunOutcome, AnalysisError>) {
    let AnalysisError::Core(CoreError::PermissionDenied { code, message }) = result.unwrap_err()
    else {
        panic!("original capture denial must remain a typed policy error");
    };
    assert_eq!(code, PermissionCode::PrivacyDenied);
    assert!(message.starts_with("capture denied: "));
}

fn assert_generated(result: Result<AnalysisRunOutcome, AnalysisError>) {
    let AnalysisRunOutcome::Generated(suggestions) = result.unwrap() else {
        panic!("fresh original capture must produce a result, not be throttled");
    };
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0].content, "capture suggestion");
}

async fn set_previous(analyzer: &ContextAnalyzer) -> DateTime<Utc> {
    let previous = Utc::now() - Duration::hours(2);
    analyzer.analysis_claim.lock().await.last_analysis_at = Some(previous);
    previous
}

async fn assert_restored(analyzer: &ContextAnalyzer, previous: DateTime<Utc>) {
    let state = analyzer.analysis_claim.lock().await;
    assert_eq!(state.last_analysis_at, Some(previous));
    assert!(!state.in_flight);
    assert_eq!(state.generation, 1);
}

#[tokio::test]
async fn events_await_rejects_old_epoch_and_fresh_capture_retries_immediately() {
    let gate = Arc::new(Gate::default());
    let (analyzer, provider) = fixture(Some(gate.clone()), None);
    let previous = set_previous(&analyzer).await;
    let origin = Origin::new();
    let permit = origin.permit();
    let mut pending = Box::pin(run(&analyzer, &permit));
    blocked(pending.as_mut()).await;
    assert!(gate.entered.load(Ordering::SeqCst));
    origin.revoke_aba();
    gate.release.notify_one();
    assert_denied(pending.await);
    assert!(provider.inputs.lock().unwrap().is_empty());
    assert_restored(&analyzer, previous).await;
    assert_generated(run(&analyzer, &origin.permit()).await);
    let inputs = provider.inputs.lock().unwrap();
    assert_eq!(inputs.len(), 1);
    assert!(inputs[0].contains(OCR_MARKER));
}

#[tokio::test]
async fn segment_lock_revocation_never_reaches_the_provider() {
    let (analyzer, provider) = fixture(None, None);
    let previous = set_previous(&analyzer).await;
    let origin = Origin::new();
    let permit = origin.permit();
    let held = analyzer.segment_stats.write().await;
    let mut pending = Box::pin(run(&analyzer, &permit));
    blocked(pending.as_mut()).await;
    origin.revoke_aba();
    drop(held);
    assert_denied(pending.await);
    assert!(provider.inputs.lock().unwrap().is_empty());
    assert_restored(&analyzer, previous).await;
    assert_generated(run(&analyzer, &origin.permit()).await);
}

#[tokio::test]
async fn provider_completion_discards_revoked_capture_results() {
    let gate = Arc::new(Gate::default());
    let (analyzer, provider) = fixture(None, Some(gate.clone()));
    let previous = set_previous(&analyzer).await;
    let origin = Origin::new();
    let permit = origin.permit();
    let mut pending = Box::pin(run(&analyzer, &permit));
    blocked(pending.as_mut()).await;
    assert!(gate.entered.load(Ordering::SeqCst));
    assert!(provider.inputs.lock().unwrap()[0].contains(OCR_MARKER));
    origin.revoke_aba();
    gate.release.notify_one();
    assert_denied(pending.await);
    assert_restored(&analyzer, previous).await;
    assert_generated(run(&analyzer, &origin.permit()).await);
    assert_eq!(provider.inputs.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn throttle_completion_await_cannot_return_revoked_results() {
    let gate = Arc::new(Gate::default());
    let (analyzer, provider) = fixture(None, Some(gate.clone()));
    let previous = set_previous(&analyzer).await;
    let origin = Origin::new();
    let permit = origin.permit();
    let mut pending = Box::pin(run(&analyzer, &permit));
    blocked(pending.as_mut()).await;
    assert!(gate.entered.load(Ordering::SeqCst));
    let held = analyzer.analysis_claim.lock().await;
    gate.release.notify_one();
    // Provider has returned and its permit check passed; only completion blocks.
    blocked(pending.as_mut()).await;
    origin.revoke_aba();
    drop(held);
    assert_denied(pending.await);
    assert_restored(&analyzer, previous).await;
    assert_eq!(provider.inputs.lock().unwrap().len(), 1);
    assert_generated(run(&analyzer, &origin.permit()).await);
}

#[tokio::test]
async fn revoked_old_run_does_not_clobber_a_newer_claim_generation() {
    let gate = Arc::new(Gate::default());
    let (analyzer, provider) = fixture(Some(gate.clone()), None);
    let origin = Origin::new();
    let permit = origin.permit();
    let mut pending = Box::pin(run(&analyzer, &permit));
    blocked(pending.as_mut()).await;
    let newer_timestamp = Utc::now() + Duration::seconds(3);
    {
        let mut state = analyzer.analysis_claim.lock().await;
        state.generation = 7;
        state.last_analysis_at = Some(newer_timestamp);
        state.in_flight = true;
    }
    origin.revoke_aba();
    gate.release.notify_one();
    assert_denied(pending.await);
    let state = analyzer.analysis_claim.lock().await;
    assert_eq!(state.generation, 7);
    assert_eq!(state.last_analysis_at, Some(newer_timestamp));
    assert!(state.in_flight);
    assert!(provider.inputs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn retain_and_ocr_are_independent_required_permissions() {
    for denied_bit in [1, 2] {
        let (analyzer, provider) = fixture(None, None);
        let previous = set_previous(&analyzer).await;
        let origin = Origin::new();
        origin.denied.store(denied_bit, Ordering::SeqCst);
        assert_denied(run(&analyzer, &origin.permit()).await);
        assert!(provider.inputs.lock().unwrap().is_empty());
        assert_restored(&analyzer, previous).await;
        origin.denied.store(0, Ordering::SeqCst);
        assert_generated(run(&analyzer, &origin.permit()).await);
    }
}

#[tokio::test]
async fn legacy_wrappers_preserve_provider_input_and_vec_results() {
    let (mut analyzer, provider) = fixture(None, None);
    analyzer.config.throttle_secs = 0;
    assert_generated(
        analyzer
            .on_significant_event_with_outcome("VSCode", "source.rs", Some(OCR_MARKER))
            .await,
    );
    let suggestions = analyzer
        .on_significant_event("VSCode", "source.rs", Some(OCR_MARKER))
        .await
        .unwrap();
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0].content, "capture suggestion");
    let origin = Origin::new();
    assert_generated(run(&analyzer, &origin.permit()).await);
    let inputs = provider.inputs.lock().unwrap();
    assert_eq!(inputs.len(), 3);
    for input in inputs.iter() {
        assert!(input.contains(OCR_MARKER));
    }
}
