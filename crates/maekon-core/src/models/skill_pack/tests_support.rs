//! Shared fixtures for Skill Pack tests.
//!
//! Lives in the crate (behind `cfg(test)`) so the resolver tests and the
//! prompt-assembly tests build the same "known good" world and can each mutate
//! one axis away from it.

use std::collections::BTreeMap;

use chrono::{TimeZone, Utc};

use super::{EffectiveGrants, SkillPackEntry};
use crate::models::extension::{
    AccountAuthentication, Availability, CapabilityGrant, ContributionKind, Enablement,
    ExtensionInstall, ExtensionProvenance, Health, InstallationState, SignatureState, SourceKind,
    UpdateState,
};

/// The verified body every fixture agrees on.
pub(crate) const GOOD_BODY: &str = "Summarize the diff. Never run shell commands.";

/// An install record that clears every non-availability gate.
pub(crate) fn granted_install(version: &str) -> ExtensionInstall {
    let t = Utc.with_ymd_and_hms(2026, 7, 21, 0, 0, 0).unwrap();
    ExtensionInstall {
        install_id: "inst_1".to_string(),
        extension_id: "com.maekon.review".to_string(),
        version: version.to_string(),
        provenance: ExtensionProvenance::Bundled,
        source_kind: SourceKind::AppBundle,
        signature_state: SignatureState::AppBundleTrusted,
        installation: InstallationState::Installed,
        enablement: Enablement::Enabled,
        authentication: AccountAuthentication::NotRequired,
        grant: CapabilityGrant::Granted,
        update: UpdateState::Current,
        health: Health::Healthy,
        previous_version: None,
        revision: 1,
        created_at: t,
        updated_at: t,
    }
}

/// The available verdict.
pub(crate) fn available() -> Availability {
    Availability::Available
}

/// Build an effective-grant map from `(capability, grant)` pairs.
pub(crate) fn grants(pairs: &[(&str, CapabilityGrant)]) -> EffectiveGrants {
    let mut map = BTreeMap::new();
    for (cap, grant) in pairs {
        map.insert((*cap).to_string(), *grant);
    }
    map
}

/// A catalog entry for [`GOOD_BODY`] with no capabilities and no references.
pub(crate) fn entry() -> SkillPackEntry {
    SkillPackEntry {
        skill_id: "sk.review".to_string(),
        install_id: "inst_1".to_string(),
        extension_id: "com.maekon.review".to_string(),
        contribution_id: "review.pack".to_string(),
        contribution_kind: ContributionKind::SkillPack,
        version: "1.0.0".to_string(),
        publisher_id: "maekon".to_string(),
        body_sha256: super::body_digest(GOOD_BODY),
        required_capabilities: vec![],
        optional_capabilities: vec![],
        references: vec![],
    }
}
