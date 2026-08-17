//! Server-gated TMD mapping models for the local-first XLSX flow (#10358).
//!
//! A cached value is never authority. Only [`EffectiveMappingResolution::Effective`]
//! returned by a live server gate may authorize a write. Storage adapters may
//! retain the same fields as a revalidation candidate, but offline callers must
//! not promote that candidate back into this resolution type.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EffectiveMapping {
    pub mapping_id: String,
    pub organization_id: String,
    pub version_id: String,
    pub version_seq: i64,
    pub content_hash: String,
    pub content: String,
    pub approval_seq: i64,
    pub approved_at: String,
    pub approved_by_user_id: String,
    pub approved_template_hash: String,
    pub assignment_id: String,
    pub assignment_hash: String,
    pub source_snapshot_hash: String,
}

impl EffectiveMapping {
    #[must_use]
    pub fn hash_content(content: &str) -> String {
        let digest = Sha256::digest(content.as_bytes());
        let mut encoded = String::with_capacity(digest.len() * 2);
        for &byte in digest.iter() {
            let _ = write!(encoded, "{byte:02x}");
        }
        encoded
    }

    /// Verify the canonical JSON bytes against the server-provided digest.
    #[must_use]
    pub fn content_hash_matches(&self) -> bool {
        Self::hash_content(&self.content) == self.content_hash
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MappingResolutionReason {
    NotApproved,
    MappingHashMismatch,
    TemplateStale,
    ReceiptContractMismatch,
    ReceiptInstanceStale,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MappingResolutionRejection {
    pub reason_code: MappingResolutionReason,
    pub mapping_id: String,
    pub assignment_id: String,
    pub message: String,
    pub expected: Option<String>,
    pub actual: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EffectiveMappingResolution {
    Effective(EffectiveMapping),
    Rejected(MappingResolutionRejection),
}

/// Disk-retained mapping data that has lost live-gate authority.
///
/// This deliberately has no conversion into [`EffectiveMappingResolution`]. It
/// is available only to prefill a revalidation request and to explain offline
/// state; it cannot authorize an XLSX write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CachedEffectiveMappingCandidate {
    pub mapping: EffectiveMapping,
    pub server_validated_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_hash_is_over_exact_utf8_bytes() {
        let mut mapping = EffectiveMapping {
            mapping_id: "map-1".into(),
            organization_id: "org-1".into(),
            version_id: "ver-1".into(),
            version_seq: 1,
            content_hash: "6d0f4f6f2ee43198ff23898c3d19e66b6d7e974c84546e7fe94830acb25a3f3f".into(),
            content: "{\"한글\":true}".into(),
            approval_seq: 1,
            approved_at: "2026-08-15T00:00:00+00:00".into(),
            approved_by_user_id: "user-1".into(),
            approved_template_hash: "a".repeat(64),
            assignment_id: "assignment-1".into(),
            assignment_hash: "b".repeat(64),
            source_snapshot_hash: "c".repeat(64),
        };
        mapping.content_hash = EffectiveMapping::hash_content(&mapping.content);
        assert!(mapping.content_hash_matches());

        mapping.content.push(' ');
        assert!(!mapping.content_hash_matches());
    }
}
