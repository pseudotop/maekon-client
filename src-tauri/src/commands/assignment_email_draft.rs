//! Receipt-only assignment email draft IPC boundary (#9627).
//!
//! The WebView may name a persisted receipt or draft, but never an actor,
//! organization, recipient, subject, body, or provider. The authenticated Rust
//! adapter owns the bearer and the server resolves all authority-bearing data.

use std::sync::{Arc, Mutex};

use maekon_core::models::assignment_email_draft::AssignmentEmailDraft;
use maekon_core::ports::assignment_email_draft_client::AssignmentEmailDraftClient;
use tauri::{command, State};

use crate::ipc_error::IpcError;

const CODE_UNAVAILABLE: &str = "service.unavailable";

pub struct AssignmentEmailDraftState(Mutex<Option<Arc<dyn AssignmentEmailDraftClient>>>);

impl AssignmentEmailDraftState {
    #[must_use]
    pub fn empty() -> Self {
        Self(Mutex::new(None))
    }

    pub fn set(&self, client: Arc<dyn AssignmentEmailDraftClient>) {
        let mut slot = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = Some(client);
    }

    #[must_use]
    pub fn get(&self) -> Option<Arc<dyn AssignmentEmailDraftClient>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

fn require_client(
    state: &State<'_, AssignmentEmailDraftState>,
) -> Result<Arc<dyn AssignmentEmailDraftClient>, IpcError> {
    state.get().ok_or_else(|| {
        IpcError::new(
            CODE_UNAVAILABLE,
            "assignment email draft transport is not wired in this build",
        )
    })
}

#[command]
pub async fn generate_assignment_email_draft(
    assignment_receipt_id: String,
    state: State<'_, AssignmentEmailDraftState>,
) -> Result<AssignmentEmailDraft, IpcError> {
    require_client(&state)?
        .generate(&assignment_receipt_id)
        .await
        .map_err(IpcError::from)
}

#[command]
pub async fn load_assignment_email_draft(
    draft_id: String,
    state: State<'_, AssignmentEmailDraftState>,
) -> Result<AssignmentEmailDraft, IpcError> {
    require_client(&state)?
        .load(&draft_id)
        .await
        .map_err(IpcError::from)
}

#[command]
pub async fn regenerate_assignment_email_draft(
    draft_id: String,
    assignment_receipt_id: String,
    state: State<'_, AssignmentEmailDraftState>,
) -> Result<AssignmentEmailDraft, IpcError> {
    require_client(&state)?
        .regenerate(&draft_id, &assignment_receipt_id)
        .await
        .map_err(IpcError::from)
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use maekon_core::error::CoreError;

    use super::*;

    struct Stub;

    #[async_trait]
    impl AssignmentEmailDraftClient for Stub {
        async fn generate(
            &self,
            _assignment_receipt_id: &str,
        ) -> Result<AssignmentEmailDraft, CoreError> {
            unreachable!("state contract test does not call transport")
        }

        async fn load(&self, _draft_id: &str) -> Result<AssignmentEmailDraft, CoreError> {
            unreachable!("state contract test does not call transport")
        }

        async fn regenerate(
            &self,
            _draft_id: &str,
            _assignment_receipt_id: &str,
        ) -> Result<AssignmentEmailDraft, CoreError> {
            unreachable!("state contract test does not call transport")
        }
    }

    #[test]
    fn state_is_empty_until_shared_authenticated_client_is_installed() {
        let state = AssignmentEmailDraftState::empty();
        assert!(state.get().is_none());
        state.set(Arc::new(Stub));
        assert!(state.get().is_some());
    }

    #[test]
    fn unavailable_uses_registered_wire_code() {
        assert_eq!(
            CODE_UNAVAILABLE,
            maekon_core::error_codes::ServiceCode::Unavailable.as_str()
        );
    }
}
