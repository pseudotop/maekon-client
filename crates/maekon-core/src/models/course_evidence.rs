//! Request-bound local course evidence (#12066). These values grant no egress rights.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const COURSE_READER_REVISION: &str = "utf8-exact-v1";
pub const MAX_COURSE_DOCUMENTS: usize = 16;
pub const MAX_CONCURRENT_COURSE_READS: usize = 2;
pub const MAX_COURSE_BYTES: usize = 262_144;
pub const MAX_COURSE_EXCERPT_BYTES: usize = 4_096;
pub const MAX_COURSE_QUERY_BYTES: usize = 256;
pub const MAX_COURSE_SEARCH_RESULTS: usize = 32;
pub const COURSE_OPERATION_TIMEOUT_MS: u64 = 500;
/// The reader retains source descriptors, never raw document/quote caches.
pub const COURSE_RAW_CACHE_TTL_SECS: u64 = 0;

/// An opaque backend-approved source, scoped to one explicit request and Part.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CourseEvidenceRequest {
    pub request_id: Uuid,
    pub source_handle: Uuid,
    pub part_id: String,
    pub access_epoch: u64,
}

impl CourseEvidenceRequest {
    pub fn validate(&self) -> Result<(), CourseEvidenceError> {
        if self.request_id.is_nil()
            || self.source_handle.is_nil()
            || self.access_epoch == 0
            || self.part_id.is_empty()
            || self.part_id.len() > 128
            || self.part_id.chars().any(char::is_control)
        {
            return Err(CourseEvidenceError::InvalidRequest);
        }
        Ok(())
    }
}

impl fmt::Debug for CourseEvidenceRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CourseEvidenceRequest")
            .field("request_id", &self.request_id)
            .field("source_handle", &self.source_handle)
            .field("access_epoch", &self.access_epoch)
            .finish_non_exhaustive()
    }
}

/// Original UTF-8 byte offsets, with an exclusive end. No normalization is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CourseByteSpan {
    pub start: usize,
    pub end: usize,
}

/// Private local provenance. Direct hashes and Part names are intentionally not Debug output.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CourseEvidenceReference {
    pub request: CourseEvidenceRequest,
    /// Fresh for each backend registration, including re-registration of identical bytes.
    pub approval_id: Uuid,
    pub source_sha256: [u8; 32],
    pub reader_revision: String,
    pub span: CourseByteSpan,
    pub quote_sha256: [u8; 32],
}

impl fmt::Debug for CourseEvidenceReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CourseEvidenceReference")
            .field("request", &self.request)
            .field("span", &self.span)
            .finish_non_exhaustive()
    }
}

/// Bounded untrusted source text. Consumers must not log, execute, or send it externally.
#[derive(Clone, PartialEq, Eq)]
pub struct CourseExcerpt {
    pub reference: CourseEvidenceReference,
    pub text: String,
}

impl fmt::Debug for CourseExcerpt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CourseExcerpt")
            .field("reference", &self.reference)
            .field("bytes", &self.text.len())
            .finish_non_exhaustive()
    }
}

/// Safe reasons only: no paths, source text, source hashes, or underlying OS errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CourseEvidenceError {
    #[error("invalid_course_request")]
    InvalidRequest,
    #[error("course_source_unavailable")]
    Unavailable,
    #[error("course_access_denied")]
    AccessDenied,
    #[error("course_access_unknown")]
    AccessUnknown,
    #[error("course_access_revoked")]
    AccessRevoked,
    #[error("course_evidence_mismatch")]
    EvidenceMismatch,
    #[error("course_source_stale")]
    Stale,
    #[error("course_format_unsupported")]
    UnsupportedFormat,
    #[error("course_path_disallowed")]
    DisallowedPath,
    #[error("course_limit_exceeded")]
    LimitExceeded,
    #[error("course_deadline_exceeded")]
    DeadlineExceeded,
}

/// The injected access owner must check current source rights and request consent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CourseAccessDecision {
    Allowed { access_epoch: u64 },
    Denied,
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> CourseEvidenceRequest {
        CourseEvidenceRequest {
            request_id: Uuid::new_v4(),
            source_handle: Uuid::new_v4(),
            part_id: "private-Part-A".to_owned(),
            access_epoch: 1,
        }
    }

    #[test]
    fn valid_scope_accepts_inclusive_byte_limit_and_unicode() {
        for part_id in ["A".to_owned(), "A".repeat(128), "한".repeat(42)] {
            let valid = CourseEvidenceRequest {
                part_id,
                ..request()
            };
            assert_eq!(valid.validate(), Ok(()));
        }
    }

    #[test]
    fn missing_identity_or_epoch_and_invalid_part_are_refused() {
        let changes: [fn(&mut CourseEvidenceRequest); 8] = [
            |r| r.request_id = Uuid::nil(),
            |r| r.source_handle = Uuid::nil(),
            |r| r.access_epoch = 0,
            |r| r.part_id.clear(),
            |r| r.part_id = "A".repeat(129),
            |r| r.part_id = "한".repeat(43),
            |r| r.part_id = "Part\nA".to_owned(),
            |r| r.part_id = "Part\0A".to_owned(),
        ];
        for change in changes {
            let mut invalid = request();
            change(&mut invalid);
            assert_eq!(invalid.validate(), Err(CourseEvidenceError::InvalidRequest));
        }
        assert_eq!(request().validate(), Ok(()));
    }

    #[test]
    fn diagnostics_keep_opaque_context_without_private_part_hash_or_quote() {
        let request = request();
        let reference = CourseEvidenceReference {
            request: request.clone(),
            approval_id: Uuid::new_v4(),
            source_sha256: [17; 32],
            reader_revision: COURSE_READER_REVISION.to_owned(),
            span: CourseByteSpan { start: 7, end: 20 },
            quote_sha256: [23; 32],
        };
        let excerpt = CourseExcerpt {
            reference: reference.clone(),
            text: "private quote".to_owned(),
        };
        let request_debug = format!("{request:?}");
        let reference_debug = format!("{reference:?}");
        let excerpt_debug = format!("{excerpt:?}");
        assert!(request_debug.contains(&request.request_id.to_string()));
        assert!(request_debug.contains(&request.source_handle.to_string()));
        assert!(reference_debug.contains("span"));
        assert!(excerpt_debug.contains("bytes"));
        for debug in [request_debug, reference_debug, excerpt_debug] {
            for private in [
                request.part_id.clone(),
                excerpt.text.clone(),
                format!("{:?}", reference.source_sha256),
                format!("{:?}", reference.quote_sha256),
            ] {
                assert!(!debug.contains(&private));
            }
        }
    }
}
