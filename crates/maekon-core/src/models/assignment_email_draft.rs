//! Receipt-backed assignment email draft wire model (#9627).
//!
//! This is data returned by the authenticated server transport. It is not a
//! provider message and carries no send capability. The only egress boundary is
//! the separate OS-handoff command, whose Rust policy validates the final URL.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssignmentEmailRecipient {
    pub contact_id: String,
    pub address: String,
    pub contact_label: String,
    pub counterparty_id: String,
    pub organization_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssignmentEmailSyntheticProvenance {
    pub synthetic: bool,
    pub source_kind: String,
    pub project_id: String,
    pub counterparty_id: String,
    pub notice: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssignmentEmailDraft {
    pub draft_id: String,
    pub draft_hash: String,
    pub organization_id: String,
    pub recipient: AssignmentEmailRecipient,
    pub subject: String,
    pub body: String,
    pub assignment_receipt_id: String,
    pub assignment_id: String,
    pub assignment_hash: String,
    pub wbs_item_id: String,
    pub revision: u32,
    pub created_at: String,
    #[serde(default)]
    pub stale: bool,
    #[serde(default)]
    pub stale_reason: Option<String>,
    pub template_id: String,
    pub template_version: String,
    pub template_hash: String,
    pub synthetic_provenance: AssignmentEmailSyntheticProvenance,
}

impl AssignmentEmailDraft {
    /// The server and OS handoff both require a non-deliverable synthetic
    /// recipient. Keeping this check here prevents an invalid server response
    /// from becoming editable UI state before the handoff backstop runs.
    #[must_use]
    pub fn has_reserved_synthetic_recipient(&self) -> bool {
        self.synthetic_provenance.synthetic
            && self.recipient.counterparty_id == self.synthetic_provenance.counterparty_id
            && is_reserved_mailbox(&self.recipient.address)
    }
}

fn is_reserved_mailbox(address: &str) -> bool {
    if address.chars().any(char::is_control) || address.contains(',') {
        return false;
    }
    let Some((local, domain)) = address.rsplit_once('@') else {
        return false;
    };
    if !is_conservative_local_part(local) || !is_valid_domain(domain) {
        return false;
    }
    let domain = domain.to_ascii_lowercase();
    matches!(
        domain.as_str(),
        "example.com" | "example.org" | "example.net"
    ) || domain.ends_with(".example")
        || domain.ends_with(".invalid")
        || domain.ends_with(".test")
}

fn is_conservative_local_part(local: &str) -> bool {
    !local.is_empty()
        && local.len() <= 64
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains("..")
        && local.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'.' | b'!'
                        | b'#'
                        | b'$'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'/'
                        | b'='
                        | b'?'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'{'
                        | b'|'
                        | b'}'
                        | b'~'
                )
        })
}

fn is_valid_domain(domain: &str) -> bool {
    !domain.is_empty()
        && domain.len() <= 253
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && label
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(address: &str) -> AssignmentEmailDraft {
        AssignmentEmailDraft {
            draft_id: "emd-1".into(),
            draft_hash: "d".repeat(64),
            organization_id: "org-wd-brokerage".into(),
            recipient: AssignmentEmailRecipient {
                contact_id: "wd-cpc-1".into(),
                address: address.into(),
                contact_label: "Synthetic PM".into(),
                counterparty_id: "wd-cp-yesisoft".into(),
                organization_label: "Yesi Software".into(),
            },
            subject: "Synthetic draft".into(),
            body: "Review only".into(),
            assignment_receipt_id: "ercv-1".into(),
            assignment_id: "wfa-1".into(),
            assignment_hash: "a".repeat(64),
            wbs_item_id: "wbs-1".into(),
            revision: 1,
            created_at: "2026-08-14T00:00:00Z".into(),
            stale: false,
            stale_reason: None,
            template_id: "assignment-counterparty-notice".into(),
            template_version: "1.0.0".into(),
            template_hash: "b".repeat(64),
            synthetic_provenance: AssignmentEmailSyntheticProvenance {
                synthetic: true,
                source_kind: "wd_brokerage_seed".into(),
                project_id: "wd-prj-1".into(),
                counterparty_id: "wd-cp-yesisoft".into(),
                notice: "Synthetic data".into(),
            },
        }
    }

    #[test]
    fn accepts_only_reserved_synthetic_recipient() {
        for address in [
            "pm@example.com",
            "pm@vendor.example",
            "pm@vendor.invalid",
            "pm@vendor.test",
        ] {
            assert!(
                fixture(address).has_reserved_synthetic_recipient(),
                "{address}"
            );
        }
        for address in [
            "pm@gmail.com",
            "pm@example.com.evil",
            "a@example.com,b@example.com",
            "a b@example.com",
            ".a@example.com",
            "a..b@example.com",
            "a@-vendor.example",
            "a@vendor..example",
        ] {
            assert!(
                !fixture(address).has_reserved_synthetic_recipient(),
                "{address}"
            );
        }
    }

    #[test]
    fn counterparty_identity_must_match_provenance() {
        let mut draft = fixture("pm@example.com");
        draft.recipient.counterparty_id = "other".into();
        assert!(!draft.has_reserved_synthetic_recipient());
    }
}
