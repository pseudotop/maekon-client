//! Read the live consent authority before pinning WBS document/session proofs.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use maekon_core::config::{AppConfig, ConfirmationRequirement};
use maekon_core::config_manager::ConfigManager;
use maekon_core::models::automation::wbs::{WbsDocumentScope, WbsProof};
use sha2::{Digest, Sha256};

use super::super::AutomationController;

use chrono::{DateTime, Utc};
use maekon_core::consent::{ConsentRecord, ConsentStatus};
use maekon_core::models::automation::wbs::{WbsAutomationError, WbsIdentifier};
use maekon_core::ports::consent_manager::ConsentManagerPort;

/// A validated observation, not a transferable write grant or an atomic lease.
/// The controller must compare its retained pin and recheck after awaits and
/// immediately before dispatch. This function never grants consent or clears
/// erasure signals; both effective permissions and the returned record must agree.
pub fn validated_consent_record(
    authority: &dyn ConsentManagerPort,
) -> Result<ConsentRecord, WbsAutomationError> {
    if authority.check_consent() != ConsentStatus::Valid
        || !authority.effective_permissions().process_monitoring
        || authority.has_pending_deletion()
        || authority.pending_erasure_id().is_some()
        || authority.deletion_flag().load(Ordering::Acquire)
        || authority.erasing().load(Ordering::Acquire)
    {
        return Err(WbsAutomationError::ConsentRequired);
    }
    let record = authority
        .current_consent()
        .ok_or(WbsAutomationError::ConsentRequired)?;
    if !record.permissions.process_monitoring
        || record.revoked_at.is_some()
        || record.data_deletion_requested
        || record.erasure_nonce.is_some()
        || record.expires_at.is_some_and(|time| time <= Utc::now())
    {
        return Err(WbsAutomationError::ConsentRequired);
    }
    WbsIdentifier::new(record.consent_id.clone())
        .map_err(|_| WbsAutomationError::ConsentRequired)?;
    WbsIdentifier::new(record.version.clone()).map_err(|_| WbsAutomationError::ConsentRequired)?;
    Ok(record)
}

/// The runtime's shared controller and live configuration/consent authorities.
/// Local document scopes do not require a server origin or organization login.
#[derive(Clone)]
pub struct WbsLiveAuthority {
    source: Arc<AuthoritySource>,
}

struct AuthoritySource {
    controller: Arc<AutomationController>,
    config: ConfigManager,
    consent: Arc<dyn ConsentManagerPort>,
    scope: WbsDocumentScope,
}

/// A retained observation bound to its originating live authority.
/// This is not an execution ticket: callers must revalidate after each await
/// and immediately before dispatch, in addition to consuming a signed ticket.
#[derive(Clone)]
pub struct WbsAuthorityPin {
    source: Arc<AuthoritySource>,
    pub(super) config: Arc<AppConfig>,
    pub(super) consent: ConsentRecord,
    pub(super) until: Instant,
    expiry: DateTime<Utc>,
}

impl WbsLiveAuthority {
    pub fn new(
        controller: Arc<AutomationController>,
        config: ConfigManager,
        consent: Arc<dyn ConsentManagerPort>,
        scope: WbsDocumentScope,
    ) -> Self {
        Self {
            source: Arc::new(AuthoritySource {
                controller,
                config,
                consent,
                scope,
            }),
        }
    }

    /// Offers/sessions may live for at most five minutes and never past consent.
    pub fn capture(&self, lifetime: Duration) -> Result<WbsAuthorityPin, WbsAutomationError> {
        if lifetime.is_zero() || lifetime > Duration::from_secs(300) {
            return Err(WbsAutomationError::InvalidInput);
        }
        let start = Instant::now();
        let now = Utc::now();
        let (config, consent) = self.source.observe()?;
        let deadline = now
            + chrono::Duration::from_std(lifetime).map_err(|_| WbsAutomationError::InvalidInput)?;
        let expiry = consent.expires_at.unwrap_or(deadline).min(deadline);
        let remaining = (expiry - now)
            .to_std()
            .map_err(|_| WbsAutomationError::Expired)?;
        let pin = WbsAuthorityPin {
            source: self.source.clone(),
            config,
            consent,
            until: start + remaining,
            expiry,
        };
        // Configuration/consent may have changed while the first observation ran.
        pin.revalidate()?;
        Ok(pin)
    }
}

impl AuthoritySource {
    fn observe(&self) -> Result<(Arc<AppConfig>, ConsentRecord), WbsAutomationError> {
        self.controller
            .ensure_enabled()
            .map_err(|_| WbsAutomationError::AutomationDisabled)?;
        let config = self.config.snapshot();
        if !config.automation.enabled {
            return Err(WbsAutomationError::AutomationDisabled);
        }
        if config.automation.sandbox.enabled || self.controller.base_sandbox_config.enabled {
            return Err(WbsAutomationError::SandboxUnsupported);
        }
        if config.automation.confirmation_policy == ConfirmationRequirement::Block
            || self.controller.confirmation_policy == ConfirmationRequirement::Block
        {
            return Err(WbsAutomationError::PolicyBlocked);
        }
        if let Some(expected) = self.scope.server_origin_digest() {
            if expected != &wbs_server_origin_digest(&config)? {
                return Err(WbsAutomationError::ConfigurationChanged);
            }
        }
        Ok((config, validated_consent_record(self.consent.as_ref())?))
    }
}

impl WbsAuthorityPin {
    pub fn expires_at(&self) -> DateTime<Utc> {
        self.expiry
    }

    /// Authenticate old proofs against the retained record, but authorize only
    /// against this fresh observation. Restoring configuration values does not
    /// restore its Arc identity; revoke/regrant does not restore consent identity.
    pub fn revalidate(&self) -> Result<(), WbsAutomationError> {
        if Instant::now() >= self.until || Utc::now() >= self.expiry {
            return Err(WbsAutomationError::Expired);
        }
        let (config, consent) = self.source.observe()?;
        if !Arc::ptr_eq(&config, &self.config) {
            return Err(WbsAutomationError::ConfigurationChanged);
        }
        let identity = |record: &ConsentRecord| {
            (
                record.consent_id.clone(),
                record.version.clone(),
                record.granted_at,
                record.expires_at,
            )
        };
        if identity(&consent) != identity(&self.consent) {
            return Err(WbsAutomationError::ConsentChanged);
        }
        Ok(())
    }
}

/// Composition fingerprint over server origin/TLS only, without credentials.
pub fn wbs_server_origin_digest(config: &AppConfig) -> Result<WbsProof, WbsAutomationError> {
    let bytes = serde_json::to_vec(&(&config.server.base_url, &config.tls))
        .map_err(|_| WbsAutomationError::InvalidInput)?;
    WbsProof::new(super::crypto::encode(&Sha256::digest(bytes)))
}

impl super::WbsDocumentSessionAccess {
    /// Request the configured user decision while the authenticated session is live.
    /// This is neither an execution ticket nor permission to write a native cell.
    pub async fn confirm_cell_apply(
        &self,
        operation: &WbsIdentifier,
    ) -> Result<(), WbsAutomationError> {
        self.revalidate()?;
        let controller = self.pin.source.controller.clone();
        let policy = maekon_core::config::ConfirmationRequirement::Confirm;
        if self.pin.config.automation.confirmation_policy != policy
            && controller.confirmation_policy != policy
        {
            return Ok(());
        }
        if controller.on_confirmation_needed.is_none() {
            return Err(WbsAutomationError::ConfirmationDenied);
        }
        let _cleanup = ConfirmationCleanup {
            controller: controller.clone(),
            operation: operation.clone(),
        };
        let args = vec![operation.as_str().to_owned()];
        let request =
            controller.request_confirmation(operation.as_str(), "WBS cell apply", &args, "Full");
        tokio::pin!(request);
        let until = self.pin.until.min(Instant::now() + Duration::from_secs(30));
        let deadline = tokio::time::sleep_until(until.into());
        tokio::pin!(deadline);
        let mut checks = tokio::time::interval(Duration::from_millis(50));
        loop {
            tokio::select! {
                answer = &mut request => {
                    if !answer.map_err(|_| WbsAutomationError::ConfirmationDenied)? {
                        return Err(WbsAutomationError::ConfirmationDenied);
                    }
                    if Instant::now() >= until {
                        return Err(WbsAutomationError::ConfirmationDenied);
                    }
                    return self.revalidate();
                }
                _ = &mut deadline => return Err(WbsAutomationError::ConfirmationDenied),
                _ = checks.tick() => self.revalidate()?,
            }
        }
    }
}

// A callback panic or cancellation must not leave its modal nonce reusable.
struct ConfirmationCleanup {
    controller: Arc<AutomationController>,
    operation: WbsIdentifier,
}

impl Drop for ConfirmationCleanup {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.controller.pending_confirmations.try_lock() {
            pending.remove(self.operation.as_str());
        } else if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let controller = self.controller.clone();
            let operation = self.operation.clone();
            handle.spawn(async move {
                if let Ok(mut pending) = tokio::time::timeout(
                    Duration::from_secs(7),
                    controller.pending_confirmations.lock(),
                )
                .await
                {
                    pending.remove(operation.as_str());
                }
            });
        }
    }
}
