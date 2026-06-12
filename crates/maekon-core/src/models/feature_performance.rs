//! Per-feature performance sample + canonical client feature_key registry
//! (#5069 — client half of #5057).
//!
//! Public privacy and payload behavior is summarized in
//! `docs/guides/telemetry.md#feature-performance-samples`.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Canonical client-defined `feature_key`s — one per heavy, discrete client
/// operation with a clear wall-clock boundary.
///
/// These are **client** keys. The server's features are dynamic admin-created
/// `FeatureToggle`s; a key with no matching server feature resolves to **404**,
/// which the emitter drops (contract §5). No coordination handshake is required.
/// Adding a feature = add a const here **and** an instrumentation point at the
/// operation's call site. Only wired keys ever produce samples — there are no
/// fabricated samples for un-wired keys (§4 anti-theater).
pub mod feature_keys {
    /// One frame capture + edge-process pass.
    pub const SCREEN_CAPTURE: &str = "screen-capture";
    /// One OCR text-extraction pass.
    pub const OCR: &str = "ocr";
    /// One accessibility (AX/UIA/AT-SPI) element-extraction pass.
    pub const VISION_ACCESSIBILITY: &str = "vision-accessibility";
    /// One on-device analysis / suggestion-generation pass.
    pub const LOCAL_SUGGESTIONS: &str = "local-suggestions";
    /// One embedding compute.
    pub const EMBEDDING: &str = "embedding";
    /// One speech-to-text transcription.
    pub const AUDIO_TRANSCRIPTION: &str = "audio-transcription";
    /// One sync push cycle.
    pub const SYNC: &str = "sync";

    /// All canonical keys (registry enumeration — used by tests/diagnostics).
    pub const ALL: &[&str] = &[
        SCREEN_CAPTURE,
        OCR,
        VISION_ACCESSIBILITY,
        LOCAL_SUGGESTIONS,
        EMBEDDING,
        AUDIO_TRANSCRIPTION,
        SYNC,
    ];
}

/// One real, measured performance sample for a single feature invocation.
///
/// Carries **only** measured, contract-allowed fields. It deliberately does NOT
/// carry the server-forbidden derived/health/identifier fields
/// (`success_rate`, `availability`, `error_rate`, `status`, `health_score`,
/// `metric_id`, `feature_id`, `organization_id`) — the server rejects those with
/// `extra="forbid"` → 422. `response_time_ms` is the real wall-clock of the
/// feature execution, never flag-eval latency or a host-process cpu/mem snapshot
/// (§4 anti-theater).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeaturePerfSample {
    /// Real wall-clock of the feature call, in milliseconds. Required, `>= 0.0`.
    pub response_time_ms: f64,

    /// Measurement instant (ISO 8601). Optional.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,

    /// Requests represented by this sample. One invocation ⇒ `Some(1)`. Optional.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_requests: Option<u64>,

    /// Errors among `total_requests` (`0` or `1` for a single invocation).
    /// Server constraint: `error_count <= total_requests`. Optional.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_count: Option<u64>,
}

impl FeaturePerfSample {
    /// Build a sample from one invocation's measured `elapsed` + whether it
    /// ended in error. Pure (no clock/async) so the unit math is testable
    /// without flake; the caller supplies `timestamp` (or `None`).
    ///
    /// `total_requests` is fixed at `1` (one invocation) and `error_count` is
    /// `1` iff `is_error`, so the pair always satisfies the server's
    /// `error_count <= total_requests` constraint.
    pub fn from_invocation(elapsed: Duration, is_error: bool, timestamp: Option<String>) -> Self {
        Self {
            // Sub-millisecond precision retained; `as_secs_f64()` is monotonic-safe
            // for a `Duration` (never negative).
            response_time_ms: elapsed.as_secs_f64() * 1000.0,
            timestamp,
            total_requests: Some(1),
            error_count: Some(u64::from(is_error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_invocation_converts_duration_to_millis() {
        let s = FeaturePerfSample::from_invocation(Duration::from_millis(250), false, None);
        assert!((s.response_time_ms - 250.0).abs() < 1e-6);
        assert_eq!(s.total_requests, Some(1));
        assert_eq!(s.error_count, Some(0));
        assert_eq!(s.timestamp, None);
    }

    #[test]
    fn from_invocation_marks_error() {
        let s = FeaturePerfSample::from_invocation(
            Duration::from_micros(1500),
            true,
            Some("2026-06-06T00:00:00Z".to_string()),
        );
        assert!((s.response_time_ms - 1.5).abs() < 1e-6);
        assert_eq!(s.error_count, Some(1));
        // Single invocation ⇒ error_count (1) never exceeds total_requests (1).
        assert!(s.error_count.unwrap() <= s.total_requests.unwrap());
    }

    #[test]
    fn serialized_sample_omits_forbidden_and_absent_fields() {
        let s = FeaturePerfSample::from_invocation(Duration::from_millis(10), false, None);
        let json = serde_json::to_value(&s).unwrap();
        let obj = json.as_object().unwrap();
        // Present: only the measured fields.
        assert!(obj.contains_key("response_time_ms"));
        assert!(obj.contains_key("total_requests"));
        assert!(obj.contains_key("error_count"));
        // Absent (None skipped).
        assert!(!obj.contains_key("timestamp"));
        // Never serializes server-forbidden derived/health/identifier fields.
        for forbidden in [
            "success_rate",
            "availability",
            "error_rate",
            "status",
            "health_score",
            "metric_id",
            "feature_id",
            "organization_id",
        ] {
            assert!(
                !obj.contains_key(forbidden),
                "sample must never serialize forbidden field `{forbidden}`"
            );
        }
    }

    #[test]
    fn registry_all_is_unique_and_kebab() {
        let all = feature_keys::ALL;
        let mut seen = std::collections::HashSet::new();
        for k in all {
            assert!(seen.insert(*k), "duplicate feature_key `{k}` in registry");
            assert!(
                k.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "feature_key `{k}` must be kebab-case (no slashes / path chars)"
            );
        }
        assert_eq!(seen.len(), 7);
    }
}
