//! Context-home snapshot wire types (#9625, WD-02.2a).
//!
//! Mirrors the server contract `context-home.v1` (#9610). The authoritative
//! fixture is `api/fixtures/context-home.v1.json`, which the server generates
//! from its own DTOs and byte-compares in CI — so a server-side field change
//! turns that fixture red first, and `context_home_fixture_parses` here turns
//! red second. Neither side can drift silently.
//!
//! ## Why these are plain data with no accessors
//!
//! This module is the transport boundary's payload, not a domain model. The UI
//! (#9611) renders it and nothing else interprets it, so adding behaviour here
//! would put display logic below the port. Sections carry a `status` precisely
//! so the UI can tell "the server had nothing" from "the server could not
//! answer" — collapsing them into an empty list is the defect the server
//! contract was designed to prevent.
//!
//! ## Unknown enum values are kept, not rejected
//!
//! `participant_kind`, `channel_kind`, and `status` are modelled as owned
//! `String`s rather than Rust enums. A server that adds a new section status or
//! participant kind must not make an otherwise-valid snapshot fail to parse on
//! an older client — the whole home would go blank over one unknown token. The
//! UI matches on the values it knows and falls back for the rest.

use serde::{Deserialize, Serialize};

/// Contract version this client is written against.
///
/// Compared against the server's `contract_version` so a major server change is
/// visible as a typed mismatch instead of silently-missing fields.
pub const CONTEXT_HOME_CONTRACT_VERSION: &str = "context-home.v1";

/// A thread participant — internal employee or external counterparty contact.
///
/// `kind` is load-bearing: the two ID spaces are disjoint and an external
/// contact is **not** a tenant member or an authenticated actor. Rendering one
/// as the other is the misrepresentation the server contract guards against, so
/// the field is never inferred here — it is whatever the server said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextHomeParticipant {
    pub participant_id: String,
    pub kind: String,
    pub display_label: String,
    #[serde(default)]
    pub role_label: Option<String>,
    /// Tenant org for internal members, counterparty id for external contacts —
    /// **different axes**, so this must not be compared without `kind`.
    #[serde(default)]
    pub affiliation_id: Option<String>,
    #[serde(default)]
    pub affiliation_label: Option<String>,
}

/// One mail or messenger thread.
///
/// ⚠️ There is deliberately no field for a message body. The server projects
/// only `last_message_preview` (bounded) and never the stored full text; giving
/// this struct a `body` would invite a future server change to start filling it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextHomeThread {
    pub thread_id: String,
    pub channel_kind: String,
    pub subject: String,
    #[serde(default)]
    pub message_count: u32,
    #[serde(default)]
    pub has_external_participants: bool,
    #[serde(default)]
    pub last_message_at: Option<String>,
    #[serde(default)]
    pub last_message_preview: String,
    #[serde(default)]
    pub last_message_sender_id: Option<String>,
    #[serde(default)]
    pub last_message_sender_kind: Option<String>,
    #[serde(default)]
    pub participants: Vec<ContextHomeParticipant>,
    #[serde(default)]
    pub participant_count: u32,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub project_label: Option<String>,
}

/// A project the actor works on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextHomeProject {
    pub project_id: String,
    pub name: String,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub project_kind: Option<String>,
    #[serde(default)]
    pub my_role: Option<String>,
    #[serde(default)]
    pub member_count: Option<u32>,
    #[serde(default)]
    pub started_on: Option<String>,
    #[serde(default)]
    pub target_end_on: Option<String>,
    #[serde(default)]
    pub counterparty_id: Option<String>,
    #[serde(default)]
    pub counterparty_label: Option<String>,
}

/// Mail or messenger section.
///
/// `status` is `ready | empty | unavailable`. `unavailable` means the server
/// could not answer for this section while the rest of the snapshot is valid —
/// the UI must not draw it as "you have no mail".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextHomeThreadSection {
    pub status: String,
    #[serde(default)]
    pub items: Vec<ContextHomeThread>,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub next_cursor: Option<String>,
    #[serde(default)]
    pub unavailable_reason: Option<String>,
}

/// Participating-projects section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextHomeProjectSection {
    pub status: String,
    #[serde(default)]
    pub items: Vec<ContextHomeProject>,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub next_cursor: Option<String>,
    #[serde(default)]
    pub unavailable_reason: Option<String>,
}

/// Who the **server** resolved the request as.
///
/// Echoed back from the JWT, not from anything the client sent — the endpoint
/// accepts no identity parameters. Surfacing it lets the UI show which account
/// the data belongs to without the client ever choosing that account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextHomeActor {
    pub actor_id: String,
    pub organization_id: String,
}

/// Where the returned items came from.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ContextHomeProvenance {
    #[serde(default)]
    pub synthetic_only: bool,
    #[serde(default)]
    pub seed_namespaces: Vec<String>,
    #[serde(default)]
    pub seed_revisions: Vec<String>,
}

/// The whole snapshot — this endpoint's response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextHomeSnapshot {
    pub contract_version: String,
    pub snapshot_id: String,
    pub as_of: String,
    /// IANA zone the server intends the dates to be read in. Explicit because
    /// the demo's day boundaries are KST and the client's local zone is not
    /// guaranteed to match.
    pub timezone: String,
    pub actor: ContextHomeActor,
    #[serde(default)]
    pub synthetic: bool,
    #[serde(default)]
    pub provenance: ContextHomeProvenance,
    pub mail: ContextHomeThreadSection,
    pub messenger: ContextHomeThreadSection,
    pub projects: ContextHomeProjectSection,
}

impl ContextHomeSnapshot {
    /// True when the server's contract version is the one this client parses.
    ///
    /// Not enforced at parse time on purpose: a mismatch with all expected
    /// fields still present is more useful shown than refused, and the caller
    /// can decide. Refusing here would black out the home on a version bump
    /// that changed nothing this client reads.
    pub fn is_expected_contract(&self) -> bool {
        self.contract_version == CONTEXT_HOME_CONTRACT_VERSION
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed fixture is generated from the server DTOs and byte-compared
    /// in server CI. Parsing it here is what makes "client and server agree" a
    /// checked claim rather than a hand-copied example that rots.
    #[test]
    fn context_home_fixture_parses() {
        let fixture = include_str!("../../../../api/fixtures/context-home.v1.json");
        let snapshot: ContextHomeSnapshot =
            serde_json::from_str(fixture).expect("committed fixture must parse");

        assert!(snapshot.is_expected_contract());
        assert_eq!(snapshot.actor.actor_id, "wd-brk-024");
        assert_eq!(snapshot.actor.organization_id, "org-wd-brokerage");
        assert_eq!(snapshot.timezone, "Asia/Seoul");

        // The fixture deliberately is not all happy path — the three sections
        // carry different statuses.
        assert_eq!(snapshot.mail.status, "ready");
        assert_eq!(snapshot.messenger.status, "ready");
        assert_eq!(snapshot.projects.status, "unavailable");
        assert_eq!(
            snapshot.projects.unavailable_reason.as_deref(),
            Some("backend_unavailable")
        );
        assert!(snapshot.projects.items.is_empty());
    }

    #[test]
    fn fixture_carries_both_participant_kinds() {
        let fixture = include_str!("../../../../api/fixtures/context-home.v1.json");
        let snapshot: ContextHomeSnapshot = serde_json::from_str(fixture).unwrap();
        let kinds: Vec<&str> = snapshot.mail.items[0]
            .participants
            .iter()
            .map(|p| p.kind.as_str())
            .collect();

        assert!(kinds.contains(&"internal_member"));
        assert!(kinds.contains(&"external_counterparty_contact"));
    }

    #[test]
    fn external_participant_affiliation_is_not_the_tenant_org() {
        // An external contact's affiliation lives on the counterparty axis. If it
        // ever equals the tenant org, the client draws an outsider as a member.
        let fixture = include_str!("../../../../api/fixtures/context-home.v1.json");
        let snapshot: ContextHomeSnapshot = serde_json::from_str(fixture).unwrap();
        let tenant = &snapshot.actor.organization_id;

        for p in &snapshot.mail.items[0].participants {
            if p.kind == "external_counterparty_contact" {
                assert_ne!(p.affiliation_id.as_ref(), Some(tenant));
                assert_ne!(&p.participant_id, &snapshot.actor.actor_id);
            }
        }
    }

    #[test]
    fn thread_has_no_field_that_could_carry_a_message_body() {
        // The client type upholds the same contract the server does — preview only.
        // No body-shaped key may appear in the serialized key set.
        let thread = ContextHomeThread {
            thread_id: "t".into(),
            channel_kind: "mail".into(),
            subject: "s".into(),
            message_count: 1,
            has_external_participants: false,
            last_message_at: None,
            last_message_preview: "p".into(),
            last_message_sender_id: None,
            last_message_sender_kind: None,
            participants: vec![],
            participant_count: 0,
            project_id: None,
            project_label: None,
        };
        let value = serde_json::to_value(&thread).unwrap();
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        for forbidden in ["body", "payload_json", "payload", "content", "full_text"] {
            assert!(
                !keys.iter().any(|k| k.as_str() == forbidden),
                "ContextHomeThread must not expose `{forbidden}`"
            );
        }
    }

    #[test]
    fn unknown_enum_tokens_do_not_fail_the_whole_snapshot() {
        // A new server-side status/kind must not blank an older client's whole
        // home — which is why these fields are `String` rather than Rust enums.
        let fixture = include_str!("../../../../api/fixtures/context-home.v1.json");
        let mutated = fixture
            .replace("\"internal_member\"", "\"some_future_kind\"")
            .replace("\"ready\"", "\"degraded\"");

        let snapshot: ContextHomeSnapshot =
            serde_json::from_str(&mutated).expect("unknown tokens must still parse");
        assert_eq!(snapshot.mail.status, "degraded");
    }
}
