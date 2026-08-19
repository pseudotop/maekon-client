//! Composition-root adapter resolving the trusted Skill Pack activation (#8588).
//!
//! Wires three existing owners together without merging them:
//!
//! * the **Extension registry** supplies install/enable/grant truth and the
//!   manifest whose §10 availability gates decide package trust;
//! * the **Skill Pack catalog** supplies the activation, the expected body
//!   digest, and the declared capabilities;
//! * `FileSkillLoader` supplies *bytes only*. ADR-029 §4 defers `FileSkillLoader`
//!   precisely because Markdown discovery carries no publisher, signature, or
//!   update guarantee — so it is used here as a byte source whose output is
//!   re-hashed against the catalog digest, never as a trust authority.
//!
//! Every call re-derives the decision from live state. Nothing is cached, which
//! is what makes restart, update, rollback, disable, and revoke take effect on
//! the next planning run rather than at some later refresh (ADR-029 §6).

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tracing::{debug, warn};

use maekon_core::error::CoreError;
use maekon_core::id_generation::generate_id;
use maekon_core::models::extension::{
    Availability, CapabilityGrant, ContributionKind, Enablement, InstallationState,
    UnavailableReason,
};
use maekon_core::models::skill_pack::{
    body_digest, EffectiveGrants, SkillActivationOutcome, SkillActivationRequest,
    SkillBlockedReason, SkillPackEntry, SkillSelectionKind, StoredActivation,
    MAX_ACTIVATION_LIFETIME_SECS,
};
use maekon_core::ports::extension_registry::ExtensionRegistryQueryPort;
use maekon_core::ports::skill_loader::SkillLoader;
use maekon_core::ports::skill_pack_registry::{
    ActiveSkillResolverPort, RecordActivationRequest, RegisterSkillPackRequest,
    SkillPackCatalogCommandPort, SkillPackCatalogQueryPort, SkillPackOutcome,
};

/// Resolves the active skill from registry + catalog + on-disk body.
pub struct RegistryActiveSkillResolver {
    catalog: Arc<dyn SkillPackCatalogQueryPort>,
    catalog_writer: Arc<dyn SkillPackCatalogCommandPort>,
    registry: Arc<dyn ExtensionRegistryQueryPort>,
    bodies: Arc<dyn SkillLoader>,
}

impl RegistryActiveSkillResolver {
    /// Assemble the resolver.
    pub fn new(
        catalog: Arc<dyn SkillPackCatalogQueryPort>,
        catalog_writer: Arc<dyn SkillPackCatalogCommandPort>,
        registry: Arc<dyn ExtensionRegistryQueryPort>,
        bodies: Arc<dyn SkillLoader>,
    ) -> Self {
        Self {
            catalog,
            catalog_writer,
            registry,
            bodies,
        }
    }

    /// Effective capability grants for a catalog entry (ADR-029 §6).
    ///
    /// Phase-1 approximation, stated plainly: the Extension registry stores one
    /// `capability_grant` axis per installation, not one per capability, so
    /// every capability the skill declares is evaluated against that single
    /// install-level grant. This is conservative — one denial withholds
    /// everything — but it is NOT per-capability granularity. Real per-capability
    /// and per-account scope arrives with the authorization broker (#8584 /
    /// ADR-031); until then a skill declaring two capabilities cannot have one
    /// granted and the other denied.
    fn effective_grants(entry: &SkillPackEntry, install_grant: CapabilityGrant) -> EffectiveGrants {
        let mut grants = EffectiveGrants::new();
        for capability in entry
            .required_capabilities
            .iter()
            .chain(entry.optional_capabilities.iter())
        {
            grants.insert(capability.clone(), install_grant);
        }
        grants
    }

    /// Register a Skill Pack catalog entry from a verified, enabled install and
    /// the on-disk body — the producer half of #8588 (explicit-selection path).
    ///
    /// Fail-closed by construction: the owning install must be `Installed` +
    /// `Enabled` and its manifest must clear every §10 availability gate before
    /// anything is written, and the body is read from the SAME `SkillLoader` key
    /// (`skill_id`) the resolver later fetches by — so a registered skill is
    /// actually resolvable (this closes the reviewer's body-on-disk key concern).
    /// `body_sha256` is derived from disk here, never supplied by the caller
    /// (WebView), which is what keeps provenance honest.
    pub async fn register_pack_from_disk(
        &self,
        install_id: &str,
        skill_id: &str,
    ) -> Result<SkillPackEntry, CoreError> {
        let install = self
            .registry
            .get_install(install_id)
            .await?
            .ok_or_else(|| not_found("ExtensionInstall", install_id))?;
        if install.installation != InstallationState::Installed {
            return Err(fail_closed(format!(
                "install {install_id} is not installed ({})",
                install.installation.as_sql_str()
            )));
        }
        if install.enablement != Enablement::Enabled {
            return Err(fail_closed(format!("install {install_id} is disabled")));
        }
        // Availability is re-derived from the stored manifest, never cached.
        let manifest = self
            .registry
            .get_manifest(install_id)
            .await?
            .ok_or_else(|| not_found("ExtensionManifest", install_id))?;
        if let Availability::Unavailable { reason } = manifest.evaluate_availability() {
            return Err(fail_closed(format!(
                "install {install_id} is unavailable ({})",
                reason.as_str()
            )));
        }
        // Bytes only — from the same key the resolver will fetch by. A missing
        // file is a hard failure, not an empty body.
        let skill = self.bodies.get_skill(skill_id)?;
        let entry = SkillPackEntry {
            skill_id: skill_id.to_string(),
            install_id: install.install_id.clone(),
            extension_id: install.extension_id.clone(),
            contribution_id: format!("{skill_id}.skill_pack"),
            contribution_kind: ContributionKind::SkillPack,
            version: install.version.clone(),
            publisher_id: manifest.publisher_id.clone(),
            body_sha256: body_digest(&skill.body),
            required_capabilities: Vec::new(),
            optional_capabilities: Vec::new(),
            references: Vec::new(),
        };
        self.catalog_writer
            .register_skill_pack(RegisterSkillPackRequest {
                entry: entry.clone(),
                now: Utc::now(),
            })
            .await?;
        Ok(entry)
    }

    /// Explicit user activation: register the pack from disk (idempotent) and
    /// record the activation as an `ExplicitUserSelection`. Returns the entry so
    /// callers can surface identity without exposing the body. `version` and
    /// `body_sha256` come from the freshly-registered catalog entry, never the
    /// caller.
    pub async fn activate_explicit(
        &self,
        install_id: &str,
        skill_id: &str,
    ) -> Result<SkillPackEntry, CoreError> {
        let entry = self.register_pack_from_disk(install_id, skill_id).await?;
        let outcome = self
            .catalog_writer
            .record_activation(RecordActivationRequest {
                activation_id: generate_id("skact"),
                skill_id: entry.skill_id.clone(),
                version: entry.version.clone(),
                body_sha256: entry.body_sha256.clone(),
                selection: SkillSelectionKind::ExplicitUserSelection,
                now: Utc::now(),
                lifetime_secs: MAX_ACTIVATION_LIFETIME_SECS,
            })
            .await?;
        if matches!(outcome, SkillPackOutcome::NotFound) {
            // The entry we just registered vanished — an uninstall raced us.
            return Err(fail_closed(format!(
                "skill {skill_id} is no longer registered"
            )));
        }
        Ok(entry)
    }

    /// Clear the current activation (explicit deselection). Idempotent.
    pub async fn clear_active_skill(&self) -> Result<(), CoreError> {
        self.catalog_writer.clear_activation().await?;
        Ok(())
    }

    /// Record the decision. Audit failures never change the decision itself —
    /// a blocked activation stays blocked even if the audit write fails.
    async fn audit(&self, outcome: &SkillActivationOutcome, skill_id: &str) {
        let record = outcome.to_audit(generate_id("skact"), skill_id, Utc::now());
        if let Err(e) = self.catalog_writer.record_activation_audit(record).await {
            warn!(error = %e, "skill pack activation audit write failed");
        }
    }
}

#[async_trait]
impl ActiveSkillResolverPort for RegistryActiveSkillResolver {
    async fn resolve_active_skill(&self) -> Result<SkillActivationOutcome, CoreError> {
        // No stored selection is the benign default: run with no skill.
        let Some(stored) = self.catalog.get_activation().await? else {
            return Ok(blocked(SkillBlockedReason::NoSelection));
        };

        let Some(entry) = self.catalog.get_skill_pack(&stored.skill_id).await? else {
            // The catalog row vanished under the activation — an uninstall that
            // raced this read. Fail closed rather than trusting the stale row.
            let outcome = blocked(SkillBlockedReason::PackageNotInstalled {
                installation: "UNINSTALLED".to_string(),
            });
            self.audit(&outcome, &stored.skill_id).await;
            return Ok(outcome);
        };

        let Some(install) = self.registry.get_install(&entry.install_id).await? else {
            let outcome = blocked(SkillBlockedReason::PackageNotInstalled {
                installation: "UNINSTALLED".to_string(),
            });
            self.audit(&outcome, &stored.skill_id).await;
            return Ok(outcome);
        };

        // Availability is re-derived from the stored manifest every time, never
        // cached — a manifest that became incompatible must start failing now.
        let availability = match self.registry.get_manifest(&entry.install_id).await? {
            Some(manifest) => manifest.evaluate_availability(),
            None => Availability::Unavailable {
                reason: UnavailableReason::SignatureUnverified,
            },
        };

        // Bytes only. A missing file is a block, not an empty body.
        let body = match self.bodies.get_skill(&entry.skill_id) {
            Ok(skill) => skill.body,
            Err(e) => {
                debug!(skill_id = %entry.skill_id, error = %e, "skill body unavailable");
                let outcome = blocked(SkillBlockedReason::BodyHashMismatch {
                    expected: entry.body_sha256.clone(),
                    actual: "absent".to_string(),
                });
                self.audit(&outcome, &stored.skill_id).await;
                return Ok(outcome);
            }
        };

        let grants = Self::effective_grants(&entry, install.grant);
        let graph = self.catalog.reference_graph().await?;
        let selection = stored.selection.clone();

        let outcome = StoredActivation::revalidate(
            &stored,
            SkillActivationRequest {
                entry: &entry,
                install: &install,
                availability: &availability,
                presented_body: &body,
                selection: Some(&selection),
                effective_grants: &grants,
                reference_graph: &graph,
                now: Utc::now(),
                lifetime_secs: MAX_ACTIVATION_LIFETIME_SECS,
            },
        );

        self.audit(&outcome, &stored.skill_id).await;
        Ok(outcome)
    }
}

fn blocked(reason: SkillBlockedReason) -> SkillActivationOutcome {
    SkillActivationOutcome::Blocked {
        reason,
        decisions: Vec::new(),
    }
}

/// A missing registry/catalog row is a not-found, not a silent success.
fn not_found(resource_type: &str, id: &str) -> CoreError {
    CoreError::NotFound {
        code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
        resource_type: resource_type.to_string(),
        id: id.to_string(),
    }
}

/// A refused precondition (disabled/unavailable/not-installed package). The
/// producer never registers or activates against such an install.
fn fail_closed(message: String) -> CoreError {
    CoreError::PermissionDenied {
        code: maekon_core::error_codes::PermissionCode::PermissionDenied,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::models::extension::ContributionKind;

    fn entry_with(required: &[&str], optional: &[&str]) -> SkillPackEntry {
        SkillPackEntry {
            skill_id: "sk.a".to_string(),
            install_id: "inst_1".to_string(),
            extension_id: "com.maekon.a".to_string(),
            contribution_id: "a.pack".to_string(),
            contribution_kind: ContributionKind::SkillPack,
            version: "1.0.0".to_string(),
            publisher_id: "maekon".to_string(),
            body_sha256: "sha256:aa".to_string(),
            required_capabilities: required.iter().map(|s| s.to_string()).collect(),
            optional_capabilities: optional.iter().map(|s| s.to_string()).collect(),
            references: vec![],
        }
    }

    #[test]
    fn a_granted_install_grants_every_declared_capability() {
        let e = entry_with(&["cal.read"], &["contacts.read"]);
        let g = RegistryActiveSkillResolver::effective_grants(&e, CapabilityGrant::Granted);
        assert_eq!(g.get("cal.read"), Some(&CapabilityGrant::Granted));
        assert_eq!(g.get("contacts.read"), Some(&CapabilityGrant::Granted));
    }

    /// A revoked install withholds everything — the conservative direction.
    #[test]
    fn a_revoked_install_withholds_every_declared_capability() {
        let e = entry_with(&["cal.read"], &["contacts.read"]);
        let g = RegistryActiveSkillResolver::effective_grants(&e, CapabilityGrant::Revoked);
        assert_eq!(g.get("cal.read"), Some(&CapabilityGrant::Revoked));
        assert_eq!(g.get("contacts.read"), Some(&CapabilityGrant::Revoked));
    }

    /// Undeclared capabilities never appear, so the grant map cannot widen the
    /// skill's scope beyond its manifest declaration.
    #[test]
    fn undeclared_capabilities_are_absent_from_the_grant_map() {
        let e = entry_with(&["cal.read"], &[]);
        let g = RegistryActiveSkillResolver::effective_grants(&e, CapabilityGrant::Granted);
        assert_eq!(g.len(), 1);
        assert!(!g.contains_key("admin.everything"));
    }
}
