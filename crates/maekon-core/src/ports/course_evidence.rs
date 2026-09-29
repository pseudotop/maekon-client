//! Local-only, request-scoped evidence access. Implementations must recheck live access.

use async_trait::async_trait;

use crate::models::course_evidence::{
    CourseAccessDecision, CourseByteSpan, CourseEvidenceError, CourseEvidenceReference,
    CourseEvidenceRequest, CourseExcerpt,
};

/// Supplied by the backend access owner; registration/selection alone is not authorization.
#[async_trait]
pub trait CourseEvidenceAccess: Send + Sync {
    async fn check(&self, request: &CourseEvidenceRequest) -> CourseAccessDecision;

    /// Nonblocking delivery fence backed by current request consent/source access state.
    /// The owner must update this view before acknowledging revocation or an epoch change.
    /// Never reuse an earlier async decision or perform I/O here; return Unknown when the
    /// live view cannot be established. Readers call this after their last await.
    fn check_current(&self, request: &CourseEvidenceRequest) -> CourseAccessDecision;
}

/// No caller-supplied paths, URLs, parsers, or executable tools cross this port.
#[async_trait]
pub trait CourseEvidenceReader: Send + Sync {
    async fn read(
        &self,
        request: &CourseEvidenceRequest,
        span: CourseByteSpan,
    ) -> Result<CourseExcerpt, CourseEvidenceError>;

    async fn search(
        &self,
        request: &CourseEvidenceRequest,
        query: &str,
    ) -> Result<Vec<CourseExcerpt>, CourseEvidenceError>;

    async fn dereference(
        &self,
        reference: &CourseEvidenceReference,
    ) -> Result<CourseExcerpt, CourseEvidenceError>;
}
