//! Feature performance buffer + flush pipeline (#5069 — client half of #5057).
//!
//! [`FeaturePerfUploader`] implements [`FeaturePerfRecorder`] (the cheap,
//! bounded, non-blocking enqueue seam the scheduler call sites push into) and
//! exposes [`flush`](FeaturePerfUploader::flush) (drained by a periodic
//! scheduler loop): consent-gate → drain → chunk ≤1000 → POST → audit, with
//! transient re-buffer / permanent drop per contract §5.
//!
//! Public privacy and payload behavior is summarized in
//! `docs/guides/telemetry.md#feature-performance-samples`.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use chrono::Utc;
use parking_lot::Mutex;
use serde::Serialize;
use tracing::warn;

use maekon_core::error::CoreError;
use maekon_core::models::feature_performance::FeaturePerfSample;
use maekon_core::models::storage_records::EgressLedgerRecord;
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::egress_ledger::EgressLedgerSink;
use maekon_core::ports::feature_perf::{FeaturePerfRecorder, FeaturePerfSink};

/// Egress ledger destination for feature-performance flushes (#4803 audit).
pub const EGRESS_DESTINATION_FEATURE_PERF: &str = "server.feature_performance";

/// Server-enforced max samples per POST (contract §5). Chunks larger than this
/// are split across requests.
const SERVER_MAX_BATCH: usize = 1000;

/// Default per-`feature_key` buffer cap (bounded-collections guardrail).
/// On overflow the **oldest** sample is dropped so a stuck backlog can never
/// grow unbounded or starve fresh data.
const DEFAULT_MAX_PER_KEY: usize = 10_000;

/// Outcome of one [`FeaturePerfUploader::flush`] cycle (for logging / tests).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FlushReport {
    /// Samples successfully shipped to the server.
    pub uploaded: usize,
    /// Samples re-buffered after a transient (5xx/timeout/429/transport) failure.
    pub requeued: usize,
    /// Samples dropped after a permanent (404/403/422) failure.
    pub dropped: usize,
    /// Samples discarded because telemetry consent was off (never egressed).
    pub blocked: usize,
}

/// Body shape for `POST /api/v1/system/features/{feature_key}/performance`.
/// `{"samples": [...]}` — the org is derived by the server from the bearer token
/// and MUST NOT appear here; samples carry only contract-allowed fields.
#[derive(Serialize)]
pub(crate) struct PerfUploadBody<'a> {
    pub samples: &'a [FeaturePerfSample],
}

/// True for failures worth retrying later (re-buffer); false for permanent
/// failures (drop). Mirrors contract §5 (5xx backoff vs 4xx drop) and the
/// `NetworkError → CoreError` mapping (`error.rs`).
fn is_transient(err: &CoreError) -> bool {
    matches!(
        err,
        CoreError::Network { .. }
            | CoreError::RequestTimeout { .. }
            | CoreError::RateLimit { .. }
            | CoreError::ServiceUnavailable { .. }
    )
}

pub struct FeaturePerfUploader {
    buffer: Mutex<HashMap<&'static str, VecDeque<FeaturePerfSample>>>,
    sink: Arc<dyn FeaturePerfSink>,
    consent: Arc<dyn ConsentManagerPort>,
    egress: Option<Arc<dyn EgressLedgerSink>>,
    /// Cached telemetry-consent gate for the cheap `record()` fast-path.
    /// Fail-closed: initialised from the current consent state and refreshed at
    /// the top of every `flush()`.
    consent_allowed: AtomicBool,
    max_per_key: usize,
    max_batch: usize,
}

impl FeaturePerfUploader {
    /// Construct with default limits.
    pub fn new(
        sink: Arc<dyn FeaturePerfSink>,
        consent: Arc<dyn ConsentManagerPort>,
        egress: Option<Arc<dyn EgressLedgerSink>>,
    ) -> Self {
        let initial = consent.telemetry_permitted();
        Self {
            buffer: Mutex::new(HashMap::new()),
            sink,
            consent,
            egress,
            consent_allowed: AtomicBool::new(initial),
            max_per_key: DEFAULT_MAX_PER_KEY,
            max_batch: SERVER_MAX_BATCH,
        }
    }

    /// Override the per-key buffer cap and per-POST batch cap (tests).
    pub fn with_limits(mut self, max_per_key: usize, max_batch: usize) -> Self {
        self.max_per_key = max_per_key.max(1);
        self.max_batch = max_batch.max(1);
        self
    }

    /// Total buffered samples across all keys (tests/diagnostics).
    pub fn buffered_len(&self) -> usize {
        self.buffer.lock().values().map(VecDeque::len).sum()
    }

    /// Drain the buffer, ship samples per `feature_key`, and audit each outcome.
    ///
    /// Never holds the buffer lock across `.await`: the whole buffer is drained
    /// under a short lock, the network I/O runs lock-free, and transient
    /// failures are re-buffered afterwards.
    pub async fn flush(&self) -> FlushReport {
        // 1. Refresh the consent gate (also updates the record() fast-path).
        let allowed = self.consent.telemetry_permitted();
        self.consent_allowed.store(allowed, Ordering::Relaxed);

        // 2. Drain everything under a short lock.
        let drained: Vec<(&'static str, Vec<FeaturePerfSample>)> = {
            let mut buf = self.buffer.lock();
            buf.drain()
                .map(|(k, v)| (k, v.into_iter().collect::<Vec<_>>()))
                .filter(|(_, v)| !v.is_empty())
                .collect()
        };

        let mut report = FlushReport::default();

        // 3. Consent off ⇒ nothing egresses. Record one `blocked` audit row per
        //    non-empty key and discard the samples (they never leave the device).
        if !allowed {
            for (key, samples) in &drained {
                report.blocked += samples.len();
                self.record_egress_row(key, samples, "blocked", false);
            }
            return report;
        }

        // 4. Per key, chunk to the server cap and ship.
        for (key, samples) in drained {
            for chunk in samples.chunks(self.max_batch) {
                match self.sink.upload_feature_performance(key, chunk).await {
                    Ok(()) => {
                        report.uploaded += chunk.len();
                        self.record_egress_row(key, chunk, "uploaded", true);
                    }
                    Err(e) if is_transient(&e) => {
                        report.requeued += chunk.len();
                        warn!(
                            err.code = %e.code(),
                            feature_key = key,
                            count = chunk.len(),
                            "feature-perf flush transient failure — re-buffering: {e}"
                        );
                        self.requeue(key, chunk);
                    }
                    Err(e) => {
                        report.dropped += chunk.len();
                        warn!(
                            err.code = %e.code(),
                            feature_key = key,
                            count = chunk.len(),
                            "feature-perf flush permanent failure — dropping batch: {e}"
                        );
                        // Permanent: data did NOT leave the device → no egress row.
                    }
                }
            }
        }

        report
    }

    /// Push a transiently-failed chunk back to the FRONT of its queue (these are
    /// older than any concurrently-recorded samples) under the bounded cap.
    fn requeue(&self, key: &'static str, chunk: &[FeaturePerfSample]) {
        let mut buf = self.buffer.lock();
        let q = buf.entry(key).or_default();
        for sample in chunk.iter().rev() {
            if q.len() >= self.max_per_key {
                // Buffer already full of fresher samples — prefer those; the
                // stale re-queued chunk is what gets dropped.
                break;
            }
            q.push_front(sample.clone());
        }
    }

    fn record_egress_row(
        &self,
        key: &str,
        samples: &[FeaturePerfSample],
        disposition: &str,
        consent_allowed: bool,
    ) {
        let Some(egress) = &self.egress else {
            return;
        };
        // Plaintext byte count of the body that would be / was sent.
        let byte_count = serde_json::to_vec(&PerfUploadBody { samples })
            .map(|v| v.len() as i64)
            .unwrap_or(0);
        let record = EgressLedgerRecord {
            record_id: uuid::Uuid::new_v4().to_string(),
            event_type: format!("feature_performance:{key}"),
            event_id: None,
            byte_count,
            // One server endpoint → 1 recipient.
            recipient_count: 1,
            destination: EGRESS_DESTINATION_FEATURE_PERF.to_string(),
            disposition: disposition.to_string(),
            consent_state: if consent_allowed {
                "telemetry:granted"
            } else {
                "telemetry:denied"
            }
            .to_string(),
            occurred_at: Utc::now().to_rfc3339(),
        };
        if let Err(e) = egress.record_egress(&record) {
            warn!(err.code = %e.code(), "feature-perf egress ledger 기록 실패: {e}");
        }
    }
}

impl FeaturePerfRecorder for FeaturePerfUploader {
    fn record(&self, feature_key: &'static str, sample: FeaturePerfSample) {
        // Fail-closed fast path: with telemetry consent off we never even buffer
        // operational timings (refreshed each flush; initialised in `new`).
        if !self.consent_allowed.load(Ordering::Relaxed) {
            return;
        }
        let mut buf = self.buffer.lock();
        let q = buf.entry(feature_key).or_default();
        if q.len() >= self.max_per_key {
            q.pop_front(); // drop oldest
        }
        q.push_back(sample);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use maekon_core::consent::{ConsentManager, ConsentPermissions};
    use std::sync::Mutex as StdMutex;

    fn sample(ms: f64) -> FeaturePerfSample {
        FeaturePerfSample {
            response_time_ms: ms,
            timestamp: None,
            total_requests: Some(1),
            error_count: Some(0),
        }
    }

    /// Consent manager with telemetry granted/denied, backed by a temp file.
    fn consent_with_telemetry(granted: bool) -> Arc<dyn ConsentManagerPort> {
        let dir = tempfile::tempdir().unwrap();
        // The tempdir may drop after this fn returns: `ConsentManager` keeps its
        // permission state in memory (RwLock), and these tests only ever *read*
        // consent (`telemetry_permitted`) — never write — after construction.
        let path = dir.path().join("consent.json");
        let mgr = ConsentManager::new(path);
        if granted {
            mgr.grant_consent(
                ConsentPermissions {
                    telemetry: true,
                    ..Default::default()
                },
                30,
            )
            .unwrap();
        }
        Arc::new(mgr)
    }

    /// Records every upload call; returns a queue of pre-programmed outcomes.
    #[derive(Default)]
    struct ScriptedSink {
        calls: StdMutex<Vec<(String, usize)>>,
        outcomes: StdMutex<VecDeque<Result<(), CoreError>>>,
    }
    impl ScriptedSink {
        fn with_outcomes(outcomes: Vec<Result<(), CoreError>>) -> Arc<Self> {
            Arc::new(Self {
                calls: StdMutex::new(Vec::new()),
                outcomes: StdMutex::new(outcomes.into()),
            })
        }
    }
    #[async_trait]
    impl FeaturePerfSink for ScriptedSink {
        async fn upload_feature_performance(
            &self,
            feature_key: &str,
            samples: &[FeaturePerfSample],
        ) -> Result<(), CoreError> {
            self.calls
                .lock()
                .unwrap()
                .push((feature_key.to_string(), samples.len()));
            self.outcomes.lock().unwrap().pop_front().unwrap_or(Ok(()))
        }
    }

    #[derive(Default)]
    struct CapturingEgress {
        rows: StdMutex<Vec<EgressLedgerRecord>>,
    }
    impl EgressLedgerSink for CapturingEgress {
        fn record_egress(&self, record: &EgressLedgerRecord) -> Result<(), CoreError> {
            self.rows.lock().unwrap().push(record.clone());
            Ok(())
        }
    }

    fn permanent_err() -> CoreError {
        CoreError::NotFound {
            code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
            resource_type: "API".into(),
            id: "missing feature".into(),
        }
    }
    fn transient_err() -> CoreError {
        CoreError::ServiceUnavailable {
            code: maekon_core::error_codes::ServiceCode::Unavailable,
            message: "503".into(),
        }
    }

    #[tokio::test]
    async fn flush_uploads_and_audits_when_consent_granted() {
        let sink = ScriptedSink::with_outcomes(vec![Ok(())]);
        let egress = Arc::new(CapturingEgress::default());
        let up = FeaturePerfUploader::new(
            sink.clone(),
            consent_with_telemetry(true),
            Some(egress.clone()),
        );
        up.record("ocr", sample(10.0));
        up.record("ocr", sample(20.0));

        let report = up.flush().await;
        assert_eq!(report.uploaded, 2);
        assert_eq!(report.blocked, 0);
        assert_eq!(up.buffered_len(), 0);

        // One POST for the single key, 2 samples.
        let calls = sink.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], ("ocr".to_string(), 2));

        // One `uploaded` egress row, recipient_count=1, plaintext byte_count>0.
        let rows = egress.rows.lock().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].disposition, "uploaded");
        assert_eq!(rows[0].recipient_count, 1);
        assert_eq!(rows[0].destination, EGRESS_DESTINATION_FEATURE_PERF);
        assert!(rows[0].byte_count > 0);
        assert_eq!(rows[0].consent_state, "telemetry:granted");
    }

    #[tokio::test]
    async fn consent_off_blocks_record_and_flush_never_egresses_data() {
        let sink = ScriptedSink::with_outcomes(vec![]);
        let egress = Arc::new(CapturingEgress::default());
        let up = FeaturePerfUploader::new(
            sink.clone(),
            consent_with_telemetry(false),
            Some(egress.clone()),
        );
        // record() fast-path is fail-closed: nothing buffers.
        up.record("ocr", sample(10.0));
        assert_eq!(up.buffered_len(), 0);

        let report = up.flush().await;
        assert_eq!(report.uploaded, 0);
        assert_eq!(report.blocked, 0); // nothing was buffered to block
                                       // No upload attempted.
        assert!(sink.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn consent_revoked_after_buffering_drops_and_audits_blocked() {
        // Granted at first so records buffer, then revoke before flush.
        // `dir` stays alive to end-of-test so revoke's consent file write lands.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mgr = ConsentManager::new(path);
        mgr.grant_consent(
            ConsentPermissions {
                telemetry: true,
                ..Default::default()
            },
            30,
        )
        .unwrap();
        let consent: Arc<dyn ConsentManagerPort> = Arc::new(mgr);

        let sink = ScriptedSink::with_outcomes(vec![]);
        let egress = Arc::new(CapturingEgress::default());
        let up = FeaturePerfUploader::new(sink.clone(), consent.clone(), Some(egress.clone()));

        up.record("sync", sample(5.0));
        up.record("sync", sample(6.0));
        assert_eq!(up.buffered_len(), 2);

        consent.revoke_consent().unwrap();
        let report = up.flush().await;

        assert_eq!(report.uploaded, 0);
        assert_eq!(report.blocked, 2);
        assert_eq!(up.buffered_len(), 0, "buffer cleared — nothing egresses");
        assert!(
            sink.calls.lock().unwrap().is_empty(),
            "no POST after revoke"
        );

        let rows = egress.rows.lock().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].disposition, "blocked");
        assert_eq!(rows[0].consent_state, "telemetry:denied");
    }

    #[tokio::test]
    async fn transient_failure_requeues_permanent_drops() {
        // First flush: transient → re-buffer. Second flush: permanent → drop.
        let sink = ScriptedSink::with_outcomes(vec![Err(transient_err()), Err(permanent_err())]);
        let egress = Arc::new(CapturingEgress::default());
        let up = FeaturePerfUploader::new(
            sink.clone(),
            consent_with_telemetry(true),
            Some(egress.clone()),
        );
        up.record("embedding", sample(1.0));

        let r1 = up.flush().await;
        assert_eq!(r1.requeued, 1);
        assert_eq!(up.buffered_len(), 1, "transient failure re-buffers");

        let r2 = up.flush().await;
        assert_eq!(r2.dropped, 1);
        assert_eq!(up.buffered_len(), 0, "permanent failure drops");

        // No `uploaded` egress row was written (data never left the device).
        let rows = egress.rows.lock().unwrap();
        assert!(rows.iter().all(|r| r.disposition != "uploaded"));
    }

    #[tokio::test]
    async fn batches_split_at_server_cap() {
        let sink = ScriptedSink::with_outcomes(vec![Ok(()), Ok(())]);
        let up = FeaturePerfUploader::new(sink.clone(), consent_with_telemetry(true), None)
            .with_limits(100, 3); // cap 100/key, 3/POST
        for i in 0..5 {
            up.record("local-suggestions", sample(i as f64));
        }
        let report = up.flush().await;
        assert_eq!(report.uploaded, 5);
        // 5 samples / 3 per POST → 2 POSTs (3 + 2).
        let calls = sink.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].1, 3);
        assert_eq!(calls[1].1, 2);
    }

    #[tokio::test]
    async fn bounded_buffer_drops_oldest() {
        let sink = ScriptedSink::with_outcomes(vec![]);
        let up =
            FeaturePerfUploader::new(sink, consent_with_telemetry(true), None).with_limits(3, 1000); // cap 3 per key
        for i in 0..10 {
            up.record("screen-capture", sample(i as f64));
        }
        assert_eq!(up.buffered_len(), 3, "bounded at max_per_key");
    }
}
