//! Typed WBS proofs and controller integration.

mod crypto;
mod execution;
mod state;

pub use crypto::WbsProofSigner;
pub use execution::WbsAutomationService;
pub use state::{
    validated_consent_record, wbs_server_origin_digest, WbsAuthorityPin, WbsLiveAuthority,
};

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use maekon_core::config_manager::ConfigManager;
use maekon_core::models::automation::wbs::{
    WbsAutomationError, WbsConsentAcceptance, WbsConsentOffer, WbsDocumentScope, WbsIdentifier,
    WbsSessionAuthorization, WbsSessionView, WBS_CONSENT_NOTICE, WBS_CONSENT_OPERATIONS,
};
use maekon_core::ports::consent_manager::ConsentManagerPort;
use zeroize::Zeroizing;

use super::AutomationController;

const MAX_DOCUMENT_SESSIONS: usize = 8;

/// Runtime-owned consent offers and authenticated document sessions.
/// Opening a session performs no native read, AI request or cell write.
#[derive(Clone)]
pub struct WbsDocumentSessions {
    runtime: Arc<DocumentSessionRuntime>,
}

struct DocumentSessionRuntime {
    controller: Arc<AutomationController>,
    authority: WbsLiveAuthority,
    signer: WbsProofSigner,
    scope: WbsDocumentScope,
    state: Mutex<DocumentSessionState>,
}

#[derive(Default)]
struct DocumentSessionState {
    generation: u64,
    offers: HashMap<String, DocumentOffer>,
    sessions: HashMap<String, DocumentSession>,
}

struct DocumentOffer {
    view: WbsConsentOffer,
    pin: WbsAuthorityPin,
}

struct DocumentSession {
    busy: bool,
    generation: u64,
    recommendation: Option<execution::Recommendation>,
    acceptance: WbsConsentAcceptance,
    view: WbsSessionView,
    pin: WbsAuthorityPin,
    cancelled: Arc<AtomicBool>,
}

/// A retained session observation for in-process consumers, never an IPC input.
/// Holding this value preserves the authentication record during cancellation
/// or expiry; an in-flight/unknown operation must keep its access until reconciled.
#[derive(Clone)]
pub struct WbsDocumentSessionAccess {
    view: WbsSessionView,
    pin: WbsAuthorityPin,
    cancelled: Arc<AtomicBool>,
}

impl WbsDocumentSessionAccess {
    pub fn view(&self) -> &WbsSessionView {
        &self.view
    }

    /// Call after each await and before dispatch; this is not an atomic write lease.
    pub fn revalidate(&self) -> Result<(), WbsAutomationError> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(WbsAutomationError::Expired);
        }
        self.pin.revalidate()
    }
}

impl WbsDocumentSessions {
    pub fn new(
        controller: Arc<AutomationController>,
        config: ConfigManager,
        consent: Arc<dyn ConsentManagerPort>,
        scope: WbsDocumentScope,
        secret: Zeroizing<Vec<u8>>,
    ) -> Result<Self, WbsAutomationError> {
        let signer = WbsProofSigner::new(secret)?;
        Ok(Self {
            runtime: Arc::new(DocumentSessionRuntime {
                authority: WbsLiveAuthority::new(
                    controller.clone(),
                    config,
                    consent,
                    scope.clone(),
                ),
                controller,
                signer,
                scope,
                state: Mutex::new(DocumentSessionState::default()),
            }),
        })
    }

    pub async fn offer_document_consent(&self) -> Result<WbsConsentOffer, WbsAutomationError> {
        let rt = &self.runtime;
        let pin = rt.authority.capture(Duration::from_secs(60))?;
        rt.require_audit().await?;
        pin.revalidate()?;
        let offer_id = document_session_id()?;
        let nonce = rt.signer.document_consent_nonce(
            &rt.scope,
            &pin.consent,
            &offer_id,
            &pin.expires_at(),
        )?;
        let view = WbsConsentOffer::new(
            offer_id,
            nonce,
            rt.scope.scope_id().clone(),
            WbsIdentifier::new(WBS_CONSENT_NOTICE.into())?,
            rt.scope.document_display_label().clone(),
            WBS_CONSENT_OPERATIONS,
            pin.expires_at(),
        );
        let mut state = rt.state()?;
        state
            .offers
            .retain(|_, offer| offer.pin.revalidate().is_ok());
        if state.offers.len() >= MAX_DOCUMENT_SESSIONS {
            return Err(WbsAutomationError::CapacityExceeded);
        }
        state.offers.insert(
            view.offer_id().as_str().into(),
            DocumentOffer {
                view: view.clone(),
                pin,
            },
        );
        Ok(view)
    }

    /// Consume the verified offer under the same lock that inserts its session.
    pub async fn open_session(
        &self,
        acceptance: WbsConsentAcceptance,
    ) -> Result<WbsSessionView, WbsAutomationError> {
        let rt = &self.runtime;
        rt.require_audit().await?;
        let mut state = rt.state()?;
        let offer = state
            .offers
            .get(acceptance.offer_id().as_str())
            .ok_or(WbsAutomationError::InvalidProof)?;
        rt.signer.verify_document_consent(
            &rt.scope,
            &offer.pin.consent,
            &acceptance,
            offer.view.expires_at(),
        )?;
        offer.pin.revalidate()?;
        let pin = rt.authority.capture(Duration::from_secs(300))?;
        if !Arc::ptr_eq(&pin.config, &offer.pin.config) {
            return Err(WbsAutomationError::ConfigurationChanged);
        }
        rt.signer
            .verify_document_consent(
                &rt.scope,
                &pin.consent,
                &acceptance,
                offer.view.expires_at(),
            )
            .map_err(|_| WbsAutomationError::ConsentChanged)?;
        state.sessions.retain(|_, session| {
            let inactive =
                session.cancelled.load(Ordering::Acquire) || session.pin.revalidate().is_err();
            // No access can be newly issued while this registry lock is held.
            // Keep any record retained by a consumer, including unknown operations.
            !(inactive && Arc::strong_count(&session.cancelled) == 1)
        });
        if state.sessions.len() >= MAX_DOCUMENT_SESSIONS {
            return Err(WbsAutomationError::CapacityExceeded);
        }
        pin.revalidate()?;
        let auth = rt.signer.session_authorization(
            &rt.scope,
            &pin.consent,
            &acceptance,
            document_session_id()?,
            &pin.expires_at(),
        )?;
        let view = WbsSessionView::new(auth, rt.scope.scope_id().clone(), pin.expires_at());
        state.offers.remove(acceptance.offer_id().as_str());
        state.sessions.insert(
            view.authorization().session_id().as_str().into(),
            DocumentSession {
                busy: false,
                generation: 0,
                recommendation: None,
                acceptance,
                view: view.clone(),
                pin,
                cancelled: Arc::new(AtomicBool::new(false)),
            },
        );
        Ok(view)
    }

    pub fn authorize(
        &self,
        auth: &WbsSessionAuthorization,
    ) -> Result<WbsDocumentSessionAccess, WbsAutomationError> {
        let state = self.runtime.state()?;
        let session = self.runtime.authenticate(&state, auth)?;
        let access = WbsDocumentSessionAccess {
            view: session.view.clone(),
            pin: session.pin.clone(),
            cancelled: session.cancelled.clone(),
        };
        access.revalidate()?;
        Ok(access)
    }

    /// Retained proof authentication permits cancelling revoked/expired sessions.
    /// Cancellation does not assert that an in-flight native operation did not run.
    pub fn cancel(&self, auth: &WbsSessionAuthorization) -> Result<(), WbsAutomationError> {
        let state = self.runtime.state()?;
        let session = self.runtime.authenticate(&state, auth)?;
        session.cancelled.store(true, Ordering::Release);
        Ok(())
    }
}

impl DocumentSessionRuntime {
    fn state(&self) -> Result<MutexGuard<'_, DocumentSessionState>, WbsAutomationError> {
        self.state
            .lock()
            .map_err(|_| WbsAutomationError::Unavailable)
    }

    fn authenticate<'a>(
        &self,
        state: &'a DocumentSessionState,
        auth: &WbsSessionAuthorization,
    ) -> Result<&'a DocumentSession, WbsAutomationError> {
        let session = state
            .sessions
            .get(auth.session_id().as_str())
            .ok_or(WbsAutomationError::InvalidCapability)?;
        self.signer
            .verify_session_authorization(
                &self.scope,
                &session.pin.consent,
                &session.acceptance,
                session.view.expires_at(),
                auth,
            )
            .map_err(|_| WbsAutomationError::InvalidCapability)?;
        Ok(session)
    }

    async fn require_audit(&self) -> Result<(), WbsAutomationError> {
        let audit =
            tokio::time::timeout(Duration::from_secs(7), self.controller.audit_logger.read())
                .await
                .map_err(|_| WbsAutomationError::AuditUnavailable)?;
        if !audit.has_persistence() {
            return Err(WbsAutomationError::AuditUnavailable);
        }
        Ok(())
    }
}

fn document_session_id() -> Result<WbsIdentifier, WbsAutomationError> {
    WbsIdentifier::new(uuid::Uuid::new_v4().to_string())
}
