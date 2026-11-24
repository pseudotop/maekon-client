//! Live upload judgment and title-policy identity.

use super::{PlatformEgressPolicy, REDACTED_WINDOW_TITLE, TITLE_POLICY_SEMANTICS_VERSION};
use maekon_core::config::{ExternalDataPolicy, PrivacyConfig};
use maekon_core::models::event::Event;
use maekon_core::ports::batch_sink::{UploadDecision, UploadPolicy, UploadProvenance};
use maekon_vision::privacy::{sanitize_title_with_level, should_exclude};
use sha2::{Digest, Sha256};

impl PlatformEgressPolicy {
    pub(super) fn title_policy_fingerprint(privacy: &PrivacyConfig) -> [u8; 32] {
        // Match the actual wildcard matcher: case-insensitive, no trimming.
        // Order and duplicate rules do not change the predicate.
        let mut patterns: Vec<_> = privacy
            .excluded_title_patterns
            .iter()
            .map(|pattern| pattern.to_lowercase())
            .collect();
        patterns.sort_unstable();
        patterns.dedup();
        let mut hash = Sha256::new();
        hash.update(b"maekon-title-policy");
        hash.update([
            TITLE_POLICY_SEMANTICS_VERSION,
            u8::from(privacy.auto_exclude_sensitive),
        ]);
        for pattern in patterns {
            hash.update((pattern.len() as u64).to_le_bytes());
            hash.update(pattern.as_bytes());
        }
        hash.finalize().into()
    }

    pub(super) fn title_excluded(privacy: &PrivacyConfig, title: &str) -> bool {
        should_exclude(
            "",
            title,
            &[],
            &[],
            &privacy.excluded_title_patterns,
            privacy.auto_exclude_sensitive,
        )
    }

    fn app_excluded(privacy: &PrivacyConfig, app_name: &str) -> bool {
        should_exclude(
            app_name,
            "",
            &privacy.excluded_apps,
            &privacy.excluded_app_patterns,
            &[],
            privacy.auto_exclude_sensitive,
        )
    }

    pub(super) fn evaluate_with_provenance(
        &self,
        storage_id: &str,
        event: &mut Event,
        provenance: Option<&UploadProvenance>,
    ) -> (UploadDecision, Option<UploadProvenance>) {
        let snapshot = self
            .config_manager
            .as_ref()
            .map(|manager| manager.snapshot());
        let privacy = snapshot
            .as_ref()
            .map(|config| &config.privacy)
            .unwrap_or(&self.privacy_config);
        let external_policy = snapshot
            .as_ref()
            .map(|config| config.ai_provider.external_data_policy)
            .unwrap_or(self.external_data_policy);
        let enabled = snapshot
            .as_ref()
            .map(|config| config.monitor.upload_enabled)
            .unwrap_or(self.enabled);
        let scheduled = snapshot
            .as_ref()
            .is_none_or(|config| super::super::schedule::tracking_schedule_allows_capture(config));
        if !enabled || !scheduled || !self.telemetry_consented() {
            return (UploadDecision::Defer, None);
        }
        let known = provenance.filter(|proof| proof.is_bound_to(storage_id, event));
        let (app_name, title) = match event {
            Event::Context(context) => {
                if context
                    .prev_app_name
                    .as_deref()
                    .is_some_and(|app| Self::app_excluded(privacy, app))
                {
                    return (UploadDecision::Block, None);
                }
                (&context.app_name, &mut context.window_title)
            }
            Event::Window(layout) => (&layout.window.app_name, &mut layout.window.window_title),
            Event::User(user) => (&user.app_name, &mut user.window_title),
            Event::Input(input) => {
                return (
                    if Self::app_excluded(privacy, &input.app_name) {
                        UploadDecision::Block
                    } else {
                        UploadDecision::Allow
                    },
                    known.cloned(),
                );
            }
            Event::Process(processes) => {
                // Block the whole snapshot rather than changing its declared
                // count or retaining an excluded process identifier.
                return (
                    if processes
                        .processes
                        .iter()
                        .any(|process| Self::app_excluded(privacy, &process.name))
                    {
                        UploadDecision::Block
                    } else {
                        UploadDecision::Allow
                    },
                    known.cloned(),
                );
            }
            Event::System(_) => {
                return (UploadDecision::Allow, known.cloned());
            }
            Event::Clipboard(_) | Event::FileAccess(_) => return (UploadDecision::Block, None),
        };
        if Self::app_excluded(privacy, app_name) {
            return (UploadDecision::Block, None);
        }
        let current_fingerprint = Self::title_policy_fingerprint(privacy);
        let no_title_restrictions =
            !privacy.auto_exclude_sensitive && privacy.excluded_title_patterns.is_empty();
        let allowed = match known {
            Some(proof) if !proof.title_information_lost => !Self::title_excluded(privacy, title),
            Some(proof) if proof.title_policy_fingerprint == current_fingerprint => {
                proof.title_allowed
            }
            _ => no_title_restrictions,
        };
        if !allowed {
            return (UploadDecision::Block, None);
        }
        let previous_title = title.clone();
        *title = match external_policy {
            ExternalDataPolicy::AllowFiltered => sanitize_title_with_level(
                title,
                external_policy.effective_egress_pii_level(privacy.pii_filter_level),
            ),
            ExternalDataPolicy::PiiFilterStrict | ExternalDataPolicy::PiiFilterStandard => {
                REDACTED_WINDOW_TITLE.to_string()
            }
        };
        let changed = previous_title != *title;
        // Unknown stays Unknown even after an unconstrained Allow. Otherwise a
        // retry could launder lost historical information into a fresh hash.
        let next = match known {
            Some(proof) => {
                let (fingerprint, lost, title_allowed) = if proof.title_information_lost {
                    // Preserve the old verdict, even when the current policy is
                    // less restrictive; it is not evidence about another policy.
                    (proof.title_policy_fingerprint, true, proof.title_allowed)
                } else {
                    (current_fingerprint, changed, true)
                };
                match UploadProvenance::new(storage_id, event, fingerprint, lost, title_allowed) {
                    Ok(proof) => Some(proof),
                    Err(_) => return (UploadDecision::Block, None),
                }
            }
            None => None,
        };
        (UploadDecision::Allow, next)
    }
}

impl UploadPolicy for PlatformEgressPolicy {
    fn evaluate(
        &self,
        storage_id: &str,
        event: &mut Event,
        provenance: Option<&UploadProvenance>,
    ) -> UploadDecision {
        self.evaluate_with_provenance(storage_id, event, provenance)
            .0
    }
}

#[cfg(test)]
mod tests;
