//! The Skill Pack activation resolver (ADR-029 §6/§10, #8588).
//!
//! One pure function decides every activation. Keeping it pure and total means
//! the fail-closed ordering is testable without a database, a registry, or a
//! network — and that there is exactly one place where "may this text become an
//! instruction?" is answered.

use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, Utc};

use super::{
    body_digest, build_active_skill, CapabilityDecision, CapabilityOutcome, CapabilityRequirement,
    EffectiveGrants, SkillActivationOutcome, SkillBlockedReason, SkillPackEntry,
    SkillSelectionKind, StoredActivation, MAX_SKILL_BODY_BYTES, MAX_SKILL_REFERENCES,
    MAX_SKILL_REFERENCE_DEPTH,
};
use crate::models::extension::{
    Availability, CapabilityGrant, ContributionKind, Enablement, ExtensionInstall,
    InstallationState,
};

/// Everything the resolver needs. Borrowed, so callers cannot accidentally hand
/// over ownership of a body and forget to zero it.
#[derive(Debug, Clone, Copy)]
pub struct SkillActivationRequest<'a> {
    /// Catalog entry for the candidate skill.
    pub entry: &'a SkillPackEntry,
    /// Registry install record for the owning extension package.
    pub install: &'a ExtensionInstall,
    /// Availability verdict for the owning package's manifest (§10 gates).
    pub availability: &'a Availability,
    /// The body presented for activation. Hashed and bounded here; never
    /// trusted because it was read from a file.
    pub presented_body: &'a str,
    /// How the skill was chosen. `None` means nothing was selected.
    pub selection: Option<&'a SkillSelectionKind>,
    /// Capability grants that already survived the §6 intersection.
    pub effective_grants: &'a EffectiveGrants,
    /// Full catalog reference graph (`skill_id -> direct references`), used for
    /// cycle and depth detection.
    pub reference_graph: &'a BTreeMap<String, Vec<String>>,
    /// Decision timestamp.
    pub now: DateTime<Utc>,
    /// Requested activation lifetime; clamped to the module maximum.
    pub lifetime_secs: i64,
}

/// Resolve an activation, failing closed at the first unmet gate.
///
/// Gate order is deliberate and load-bearing: trust of the owning package is
/// settled before the body is even hashed, and capabilities are resolved last so
/// a capability decision is never recorded for a package that was never
/// trustworthy in the first place.
pub fn resolve_activation(request: SkillActivationRequest<'_>) -> SkillActivationOutcome {
    let entry = request.entry;

    // Gate 1 — nothing was selected. Benign: the caller runs with no skill.
    let Some(selection) = request.selection else {
        return blocked(SkillBlockedReason::NoSelection);
    };

    // Gate 2 — the owning package must clear every §10 availability gate. This
    // is where an unverified signature, an untrusted publisher, an incompatible
    // host API, and a managed-policy denial all land.
    if let Availability::Unavailable { reason } = request.availability {
        return blocked(SkillBlockedReason::PackageUnavailable {
            reason: reason.as_str().to_string(),
        });
    }

    // Gate 3 — installed.
    if request.install.installation != InstallationState::Installed {
        return blocked(SkillBlockedReason::PackageNotInstalled {
            installation: request.install.installation.as_sql_str().to_string(),
        });
    }

    // Gate 4 — enabled. Disabling a package must block new work immediately
    // (ADR-029 §5), including an activation that was resolved a moment ago.
    if request.install.enablement != Enablement::Enabled {
        return blocked(SkillBlockedReason::PackageDisabled);
    }

    // Gate 5 — the contribution really is a skill pack. A `context_source`
    // contribution must never reach instruction position; that is precisely the
    // "external payload becomes Skill Pack instruction" threat.
    if entry.contribution_kind != ContributionKind::SkillPack {
        return blocked(SkillBlockedReason::ContributionNotSkillPack {
            kind: entry.contribution_kind.as_sql_str(),
        });
    }

    // Gate 6 — the catalog entry must match the version currently installed.
    // An update or a rollback changes `install.version`, which strands the old
    // entry here instead of letting its body be reactivated.
    if entry.version != request.install.version {
        return blocked(SkillBlockedReason::VersionChanged {
            activated: entry.version.clone(),
            current: request.install.version.clone(),
        });
    }

    // Gate 7 — bound the body before hashing it, so an oversized body is
    // rejected without paying for the digest.
    let bytes = request.presented_body.len();
    if bytes > MAX_SKILL_BODY_BYTES {
        return blocked(SkillBlockedReason::BodyTooLarge {
            bytes,
            limit: MAX_SKILL_BODY_BYTES,
        });
    }

    // Gate 8 — the presented body must hash to the digest the catalog recorded
    // at registration. A file swapped on disk after verification dies here.
    let actual = body_digest(request.presented_body);
    if actual != entry.body_sha256 {
        return blocked(SkillBlockedReason::BodyHashMismatch {
            expected: entry.body_sha256.clone(),
            actual,
        });
    }

    // Gate 9 — bound the declared reference fan-out.
    if entry.references.len() > MAX_SKILL_REFERENCES {
        return blocked(SkillBlockedReason::TooManyReferences {
            count: entry.references.len(),
            limit: MAX_SKILL_REFERENCES,
        });
    }

    // Gate 10 — the reference graph must be acyclic and shallow.
    match inspect_reference_graph(&entry.skill_id, request.reference_graph) {
        GraphVerdict::Cycle { at } => {
            return blocked(SkillBlockedReason::DependencyCycle { at });
        }
        GraphVerdict::Acyclic { depth } if depth > MAX_SKILL_REFERENCE_DEPTH => {
            return blocked(SkillBlockedReason::ReferenceDepthExceeded {
                depth,
                limit: MAX_SKILL_REFERENCE_DEPTH,
            });
        }
        GraphVerdict::Acyclic { .. } => {}
    }

    // Gate 11 — capabilities. Required ones must be granted; optional ones are
    // simply absent from the granted set when they are not.
    let decisions = resolve_capabilities(entry, request.effective_grants);
    if let Some(missing) = decisions
        .iter()
        .find(|d| d.requirement == CapabilityRequirement::Required && !d.is_granted())
    {
        let grant = match &missing.outcome {
            CapabilityOutcome::Withheld { grant } => *grant,
            CapabilityOutcome::Granted => CapabilityGrant::NotRequested,
        };
        return SkillActivationOutcome::Blocked {
            reason: SkillBlockedReason::RequiredCapabilityMissing {
                capability: missing.capability.clone(),
                grant,
            },
            decisions,
        };
    }

    SkillActivationOutcome::Activated(Box::new(build_active_skill(
        entry,
        selection.clone(),
        request.presented_body.to_string(),
        decisions,
        request.now,
        request.lifetime_secs,
    )))
}

/// Classify every declared capability against the effective grants.
///
/// A missing optional capability yields [`CapabilityOutcome::Withheld`] — the
/// skill runs without it. It never causes a different capability to be
/// substituted, a broader capability to be selected, or a different provider to
/// be chosen; the returned set is a subset of what the skill declared, always.
fn resolve_capabilities(
    entry: &SkillPackEntry,
    effective: &EffectiveGrants,
) -> Vec<CapabilityDecision> {
    let mut decisions =
        Vec::with_capacity(entry.required_capabilities.len() + entry.optional_capabilities.len());
    for (list, requirement) in [
        (
            &entry.required_capabilities,
            CapabilityRequirement::Required,
        ),
        (
            &entry.optional_capabilities,
            CapabilityRequirement::Optional,
        ),
    ] {
        for capability in list {
            // Absent from the intersection is treated exactly like an explicit
            // `NotRequested` — unknown never means allow.
            let grant = effective
                .get(capability)
                .copied()
                .unwrap_or(CapabilityGrant::NotRequested);
            let outcome = if grant.is_ready() {
                CapabilityOutcome::Granted
            } else {
                CapabilityOutcome::Withheld { grant }
            };
            decisions.push(CapabilityDecision {
                capability: capability.clone(),
                requirement,
                outcome,
            });
        }
    }
    decisions
}

/// Verdict from walking the reference graph.
enum GraphVerdict {
    /// Acyclic, with the longest path length from the root.
    Acyclic {
        /// Depth in edges; a skill with no references has depth 0.
        depth: u32,
    },
    /// A cycle was found.
    Cycle {
        /// Skill id where the back-edge closed the cycle.
        at: String,
    },
}

/// Depth-first walk detecting cycles and measuring depth in one pass.
///
/// An explicit stack keeps a hostile (deep) catalog from overflowing the real
/// stack before the depth bound can reject it.
fn inspect_reference_graph(root: &str, graph: &BTreeMap<String, Vec<String>>) -> GraphVerdict {
    // (node, depth, entering?) — `entering == false` marks the post-visit pop.
    let mut stack: Vec<(String, u32, bool)> = vec![(root.to_string(), 0, true)];
    let mut on_path: HashSet<String> = HashSet::new();
    let mut settled: HashSet<String> = HashSet::new();
    let mut max_depth = 0_u32;

    while let Some((node, depth, entering)) = stack.pop() {
        if !entering {
            on_path.remove(&node);
            settled.insert(node);
            continue;
        }
        if on_path.contains(&node) {
            return GraphVerdict::Cycle { at: node };
        }
        if settled.contains(&node) {
            continue;
        }
        max_depth = max_depth.max(depth);
        // Stop descending once the bound is already exceeded — the caller only
        // needs to know that it was, not how much worse it gets.
        if depth > MAX_SKILL_REFERENCE_DEPTH {
            return GraphVerdict::Acyclic { depth };
        }
        on_path.insert(node.clone());
        stack.push((node.clone(), depth, false));
        if let Some(children) = graph.get(&node) {
            for child in children {
                stack.push((child.clone(), depth + 1, true));
            }
        }
    }
    GraphVerdict::Acyclic { depth: max_depth }
}

impl StoredActivation {
    /// Re-validate a stored activation against current registry truth.
    ///
    /// This is the restart / update / rollback / revoke invalidation path. The
    /// stored row contributes only identity and expiry; every trust decision is
    /// recomputed from the live catalog entry, install record, and grants. A
    /// stored activation can therefore never resurrect a body whose package was
    /// disabled, downgraded, revoked, or replaced while the process was down.
    pub fn revalidate(&self, request: SkillActivationRequest<'_>) -> SkillActivationOutcome {
        // Expiry is the one fact the stored row owns.
        if request.now >= self.expires_at {
            return SkillActivationOutcome::Blocked {
                reason: SkillBlockedReason::ActivationExpired,
                decisions: Vec::new(),
            };
        }
        // The version the activation was bound to must still be the catalog's.
        // A rollback to an older version that happens to be re-registered under
        // the same id is caught here even when the install record agrees with
        // the entry.
        if self.version != request.entry.version {
            return SkillActivationOutcome::Blocked {
                reason: SkillBlockedReason::VersionChanged {
                    activated: self.version.clone(),
                    current: request.entry.version.clone(),
                },
                decisions: Vec::new(),
            };
        }
        // Likewise the digest: a same-version body edit is still a body change.
        if self.body_sha256 != request.entry.body_sha256 {
            return SkillActivationOutcome::Blocked {
                reason: SkillBlockedReason::BodyHashMismatch {
                    expected: self.body_sha256.clone(),
                    actual: request.entry.body_sha256.clone(),
                },
                decisions: Vec::new(),
            };
        }
        resolve_activation(request)
    }
}

/// Shorthand for a block with no capability decisions recorded yet.
fn blocked(reason: SkillBlockedReason) -> SkillActivationOutcome {
    SkillActivationOutcome::Blocked {
        reason,
        decisions: Vec::new(),
    }
}
