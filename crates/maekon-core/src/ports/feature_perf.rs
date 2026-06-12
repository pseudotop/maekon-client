//! Feature performance emitter ports (#5069 — client half of #5057).
//!
//! Two object-safe ports plus a timing helper:
//! - [`FeaturePerfRecorder`] — the **sync, non-blocking** enqueue seam the
//!   scheduler call sites push samples into (like a metrics counter).
//! - [`FeaturePerfSink`] — the **async** transport that ships a batch of one
//!   `feature_key`'s samples to the server.
//! - [`time_feature`] — wraps a feature call in a wall-clock span and records a
//!   sample built from the measured `Duration` (§4 anti-theater: the span IS the
//!   feature execution, not flag-eval or process cpu/mem).
//!
//! Public privacy and payload behavior is summarized in
//! `docs/guides/telemetry.md#feature-performance-samples`.

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::feature_performance::FeaturePerfSample;

/// Object-safe, **synchronous** recorder seam: enqueue one measured sample for a
/// `feature_key`. Implementations MUST be cheap and non-blocking (a bounded
/// buffer push) so wrapping a hot call site adds negligible overhead and never
/// colours it async. Consent/transport happen later, at flush.
pub trait FeaturePerfRecorder: Send + Sync {
    /// Enqueue one sample. Bounded buffers drop the oldest sample on overflow;
    /// this call never blocks and never fails the audited operation.
    fn record(&self, feature_key: &'static str, sample: FeaturePerfSample);
}

/// Object-safe **async** transport for a batch of one `feature_key`'s samples.
///
/// Implemented by `HttpApiClient` (POST
/// `/api/v1/system/features/{feature_key}/performance`). The returned
/// `CoreError` MUST distinguish transient (5xx/timeout/429/transport — caller
/// re-buffers) from permanent (404 missing / 403 forbidden / 422 invalid —
/// caller drops) failures so the flush honours contract §5.
#[async_trait]
pub trait FeaturePerfSink: Send + Sync {
    /// Ship `samples` (already chunked to the server's ≤1000 cap) for
    /// `feature_key`. The org is derived by the server from the bearer token —
    /// callers MUST NOT include it (or any forbidden field) in the body.
    ///
    /// # Errors
    /// Returns `CoreError` on any non-2xx; the variant encodes transient vs
    /// permanent (see trait docs).
    async fn upload_feature_performance(
        &self,
        feature_key: &str,
        samples: &[FeaturePerfSample],
    ) -> Result<(), CoreError>;
}

/// Time a feature call and record a sample built from the measured wall-clock.
///
/// `None` recorder ⇒ the future runs with zero timing overhead (emitter
/// un-wired). The result is returned untouched; `error_count` is `1` iff the
/// call returned `Err`. The `timestamp` is stamped at completion.
pub async fn time_feature<F, T, E>(
    recorder: Option<&Arc<dyn FeaturePerfRecorder>>,
    feature_key: &'static str,
    fut: F,
) -> Result<T, E>
where
    F: std::future::Future<Output = Result<T, E>>,
{
    let Some(rec) = recorder else {
        return fut.await;
    };
    let start = std::time::Instant::now();
    let result = fut.await;
    let sample =
        FeaturePerfSample::from_invocation(start.elapsed(), result.is_err(), Some(now_rfc3339()));
    rec.record(feature_key, sample);
    result
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct CapturingRecorder {
        samples: Mutex<Vec<(&'static str, FeaturePerfSample)>>,
    }
    impl FeaturePerfRecorder for CapturingRecorder {
        fn record(&self, feature_key: &'static str, sample: FeaturePerfSample) {
            self.samples.lock().unwrap().push((feature_key, sample));
        }
    }

    #[tokio::test]
    async fn time_feature_records_success_sample() {
        // Keep the concrete Arc to inspect; coerce a clone (same allocation) to
        // `dyn` for the call so the recorded samples are visible here.
        let rec = Arc::new(CapturingRecorder::default());
        let dyn_rec: Arc<dyn FeaturePerfRecorder> = rec.clone();
        let out: Result<u32, ()> =
            time_feature(Some(&dyn_rec), "screen-capture", async { Ok(7) }).await;
        assert_eq!(out, Ok(7));

        let cap = rec.samples.lock().unwrap();
        assert_eq!(cap.len(), 1);
        assert_eq!(cap[0].0, "screen-capture");
        assert_eq!(cap[0].1.error_count, Some(0));
        assert!(cap[0].1.timestamp.is_some());
        assert!(cap[0].1.response_time_ms >= 0.0);
    }

    #[tokio::test]
    async fn time_feature_records_error_sample_and_propagates_err() {
        let rec = Arc::new(CapturingRecorder::default());
        let dyn_rec: Arc<dyn FeaturePerfRecorder> = rec.clone();
        let out: Result<(), &str> =
            time_feature(Some(&dyn_rec), "ocr", async { Err("boom") }).await;
        assert_eq!(out, Err("boom"));

        let cap = rec.samples.lock().unwrap();
        assert_eq!(cap.len(), 1);
        assert_eq!(cap[0].0, "ocr");
        assert_eq!(cap[0].1.error_count, Some(1));
    }

    #[tokio::test]
    async fn time_feature_none_recorder_is_passthrough() {
        let out: Result<u32, ()> = time_feature(None, "sync", async { Ok(42) }).await;
        assert_eq!(out, Ok(42));
    }
}
