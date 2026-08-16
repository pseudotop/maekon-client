//! PII-free Maekon→Console pending handoff receipt (#9628).
//!
//! Actor, organization, bearer and target URL are deliberately absent. The
//! server binds the record to its JWT scope; this payload only lets Maekon show
//! which synthetic run/source snapshot it asked Console to continue.

use serde::{Deserialize, Serialize};

pub const CONSOLE_HANDOFF_CONTRACT_VERSION: &str = "console-handoff.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleHandoffProvenance {
    pub seed_namespaces: Vec<String>,
    #[serde(default)]
    pub seed_revisions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleHandoffIssue {
    pub contract_version: String,
    pub run_id: String,
    pub source_snapshot_id: String,
    pub issued_at: String,
    pub expires_at: String,
    pub synthetic: bool,
    pub source_provenance: ConsoleHandoffProvenance,
}

impl ConsoleHandoffIssue {
    #[must_use]
    pub fn is_valid_contract(&self) -> bool {
        self.contract_version == CONSOLE_HANDOFF_CONTRACT_VERSION
            && self.synthetic
            && !self.run_id.trim().is_empty()
            && !self.source_snapshot_id.trim().is_empty()
            && !self.source_provenance.seed_namespaces.is_empty()
            && self
                .source_provenance
                .seed_namespaces
                .iter()
                .all(|namespace| !namespace.trim().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipt_has_no_identity_token_or_url_field() {
        let receipt = ConsoleHandoffIssue {
            contract_version: CONSOLE_HANDOFF_CONTRACT_VERSION.to_string(),
            run_id: "cor_01K2M6R8J9K0M1N2P3Q4R5S6T7".to_string(),
            source_snapshot_id: "context-snapshot-01".to_string(),
            issued_at: "2026-08-14T09:00:00Z".to_string(),
            expires_at: "2026-08-14T09:05:00Z".to_string(),
            synthetic: true,
            source_provenance: ConsoleHandoffProvenance {
                seed_namespaces: vec!["wd-brokerage".to_string()],
                seed_revisions: vec!["wd-01.2".to_string()],
            },
        };

        assert!(receipt.is_valid_contract());
        let wire = serde_json::to_string(&receipt).unwrap();
        for forbidden in [
            "actor_id",
            "organization_id",
            "token",
            "bearer",
            "url",
            "href",
        ] {
            assert!(
                !wire.contains(forbidden),
                "wire must not contain {forbidden}"
            );
        }
    }

    #[test]
    fn missing_synthetic_provenance_fails_contract_validation() {
        let receipt: ConsoleHandoffIssue = serde_json::from_str(
            r#"{
              "contract_version":"console-handoff.v1",
              "run_id":"cor_test",
              "source_snapshot_id":"snapshot",
              "issued_at":"2026-08-14T09:00:00Z",
              "expires_at":"2026-08-14T09:05:00Z",
              "synthetic":true,
              "source_provenance":{"seed_namespaces":[],"seed_revisions":[]}
            }"#,
        )
        .unwrap();

        assert!(!receipt.is_valid_contract());
    }
}
