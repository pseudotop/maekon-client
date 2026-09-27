//! Metadata-only status contract for the local OCR-derived suggestion path.

use maekon_core::consent::ConsentPermissions;
use maekon_core::error::CoreError;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalAnalysisStatusKind {
    Generated,
    NoCandidate,
    Throttled,
    ProviderOffline,
    PolicyBlocked,
    ConsentRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalAnalysisProducer {
    Periodic,
    AppSwitch,
}

/// Safe product/telemetry projection. It intentionally contains no prompt,
/// OCR text, window title, provider response, or provider error message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalAnalysisStatus {
    pub status: LocalAnalysisStatusKind,
    pub reason: &'static str,
    pub producer: LocalAnalysisProducer,
    pub source: &'static str,
    pub observed_at: String,
    pub candidate_count: u32,
    pub queue_count: u32,
    pub missing_permissions: Vec<&'static str>,
}

impl LocalAnalysisStatus {
    /// Crate-visible because scheduler producers construct the transport-safe
    /// projection while the type itself is serialized by suggestion queries.
    pub(crate) fn new(
        status: LocalAnalysisStatusKind,
        reason: &'static str,
        producer: LocalAnalysisProducer,
        candidate_count: usize,
        queue_count: usize,
    ) -> Self {
        Self {
            status,
            reason,
            producer,
            source: "llm_local",
            observed_at: chrono::Utc::now().to_rfc3339(),
            candidate_count: u32::try_from(candidate_count).unwrap_or(u32::MAX),
            queue_count: u32::try_from(queue_count).unwrap_or(u32::MAX),
            missing_permissions: Vec::new(),
        }
    }

    /// Crate-visible constructor for the independently revocable consent gate.
    pub(crate) fn consent_required(
        producer: LocalAnalysisProducer,
        missing_permissions: Vec<&'static str>,
        queue_count: usize,
    ) -> Self {
        let mut status = Self::new(
            LocalAnalysisStatusKind::ConsentRequired,
            "consent_required",
            producer,
            0,
            queue_count,
        );
        status.missing_permissions = missing_permissions;
        status
    }

    /// Crate-visible adapter from internal failures to bounded product states.
    pub(crate) fn from_error(
        error: &CoreError,
        producer: LocalAnalysisProducer,
        queue_count: usize,
    ) -> Self {
        let (status, reason) = match error {
            CoreError::ConsentRequired { .. } | CoreError::ConsentExpired { .. } => (
                LocalAnalysisStatusKind::ConsentRequired,
                "provider_consent_required",
            ),
            CoreError::PolicyDenied { .. }
            | CoreError::PrivacyDenied { .. }
            | CoreError::PermissionDenied { .. } => (
                LocalAnalysisStatusKind::PolicyBlocked,
                "provider_policy_blocked",
            ),
            _ => (
                LocalAnalysisStatusKind::ProviderOffline,
                "provider_unavailable",
            ),
        };
        Self::new(status, reason, producer, 0, queue_count)
    }
}

/// Independent invocation gate for OCR-derived analysis. The capture composite
/// gate is necessary but insufficient: OCR processing and activity-pattern
/// learning remain separately revocable authorities (#11737).
pub(crate) fn missing_local_analysis_permissions(
    permissions: &ConsentPermissions,
) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if !permissions.screen_capture {
        missing.push("screen_capture");
    }
    if !permissions.ocr_processing {
        missing.push("ocr_processing");
    }
    if !permissions.activity_pattern_learning {
        missing.push("activity_pattern_learning");
    }
    missing
}

#[cfg(feature = "local-suggestions")]
pub(crate) async fn record_periodic_status(
    manager: Option<&std::sync::Arc<crate::suggestion_manager::SuggestionManager>>,
    status: LocalAnalysisStatusKind,
    reason: &'static str,
    candidate_count: usize,
) {
    if let Some(manager) = manager {
        let queue_count = manager.queue().lock().await.len();
        manager
            .record_local_analysis(LocalAnalysisStatus::new(
                status,
                reason,
                LocalAnalysisProducer::Periodic,
                candidate_count,
                queue_count,
            ))
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::error_codes::{ConsentCode, PermissionCode, PolicyCode, ProviderCode};

    #[test]
    fn each_independent_consent_mutation_closes_the_gate() {
        let ready = ConsentPermissions {
            screen_capture: true,
            ocr_processing: true,
            activity_pattern_learning: true,
            ..Default::default()
        };
        assert!(missing_local_analysis_permissions(&ready).is_empty());

        for (field, mutate) in [
            (
                "screen_capture",
                (|p: &mut ConsentPermissions| p.screen_capture = false)
                    as fn(&mut ConsentPermissions),
            ),
            (
                "ocr_processing",
                (|p: &mut ConsentPermissions| p.ocr_processing = false)
                    as fn(&mut ConsentPermissions),
            ),
            (
                "activity_pattern_learning",
                (|p: &mut ConsentPermissions| p.activity_pattern_learning = false)
                    as fn(&mut ConsentPermissions),
            ),
        ] {
            let mut mutated = ready.clone();
            mutate(&mut mutated);
            assert_eq!(missing_local_analysis_permissions(&mutated), vec![field]);
        }
    }

    #[test]
    fn provider_failure_is_distinct_and_does_not_serialize_error_content() {
        let failure = CoreError::Analysis {
            code: ProviderCode::AnalysisFailed,
            message: "sensitive provider payload".to_string(),
        };

        let status = LocalAnalysisStatus::from_error(&failure, LocalAnalysisProducer::AppSwitch, 4);
        let wire = serde_json::to_string(&status).expect("serialize safe status");

        assert_eq!(status.status, LocalAnalysisStatusKind::ProviderOffline);
        assert_eq!(status.reason, "provider_unavailable");
        assert_eq!(status.queue_count, 4);
        assert!(!wire.contains("sensitive provider payload"));
    }

    #[test]
    fn consent_failures_preserve_the_consent_required_product_state() {
        let errors = [
            CoreError::ConsentRequired {
                code: ConsentCode::Required,
                message: "missing OCR consent".to_string(),
            },
            CoreError::ConsentExpired {
                code: ConsentCode::Expired,
            },
        ];

        for error in errors {
            let status =
                LocalAnalysisStatus::from_error(&error, LocalAnalysisProducer::Periodic, 2);
            assert_eq!(status.status, LocalAnalysisStatusKind::ConsentRequired);
            assert_eq!(status.reason, "provider_consent_required");
            assert_eq!(status.queue_count, 2);
        }
    }

    #[test]
    fn policy_and_permission_failures_preserve_the_policy_blocked_product_state() {
        let errors = [
            CoreError::PolicyDenied {
                code: PolicyCode::Denied,
                message: "policy denied".to_string(),
            },
            CoreError::PrivacyDenied {
                code: PermissionCode::PrivacyDenied,
                message: "privacy denied".to_string(),
            },
            CoreError::PermissionDenied {
                code: PermissionCode::PermissionDenied,
                message: "permission denied".to_string(),
            },
        ];

        for error in errors {
            let status =
                LocalAnalysisStatus::from_error(&error, LocalAnalysisProducer::AppSwitch, 3);
            assert_eq!(status.status, LocalAnalysisStatusKind::PolicyBlocked);
            assert_eq!(status.reason, "provider_policy_blocked");
            assert_eq!(status.queue_count, 3);
        }
    }

    #[test]
    fn wire_counts_preserve_small_values_and_saturate_at_u32_max() {
        let exact = LocalAnalysisStatus::new(
            LocalAnalysisStatusKind::Generated,
            "generated",
            LocalAnalysisProducer::Periodic,
            7,
            9,
        );
        assert_eq!(exact.candidate_count, 7);
        assert_eq!(exact.queue_count, 9);

        let saturated = LocalAnalysisStatus::new(
            LocalAnalysisStatusKind::Generated,
            "generated",
            LocalAnalysisProducer::Periodic,
            usize::MAX,
            usize::MAX,
        );
        assert_eq!(saturated.candidate_count, u32::MAX);
        assert_eq!(saturated.queue_count, u32::MAX);
    }
}
