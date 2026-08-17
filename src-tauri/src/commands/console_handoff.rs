//! Dedicated Maekon→Console assignment-board handoff (#9628).
//!
//! The WebView supplies no URL, actor, organization or token. Rust issues a
//! server-side pending record through the shared authenticated client, builds a
//! fixed `/console/handoff` target from an operator-configured HTTPS origin, and
//! sends that validated target through the single OS handoff boundary.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use maekon_core::models::console_handoff::ConsoleHandoffIssue;
use maekon_core::ports::console_handoff_client::ConsoleHandoffClient;
use tauri::State;
use url::Url;

use crate::commands::os_handoff::{launch_validated, validate, ValidatedTarget};
use crate::ipc_error::IpcError;

const CONSOLE_HANDOFF_PATH: &str = "/console/handoff";
const CODE_CONFIG_MISSING: &str = "config.missing";
const CODE_CONFIG_INVALID: &str = "config.invalid";
const CODE_UNAVAILABLE: &str = "service.unavailable";

pub struct ConsoleHandoffState {
    client: Mutex<Option<Arc<dyn ConsoleHandoffClient>>>,
    in_flight: AtomicBool,
}

impl ConsoleHandoffState {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            client: Mutex::new(None),
            in_flight: AtomicBool::new(false),
        }
    }

    pub fn set(&self, client: Arc<dyn ConsoleHandoffClient>) {
        let mut slot = self
            .client
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = Some(client);
    }

    fn get(&self) -> Option<Arc<dyn ConsoleHandoffClient>> {
        self.client
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

struct InFlightGuard<'a>(&'a AtomicBool);

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Build the only URL this command may hand to the OS.
fn build_target(
    console_base_url: Option<&str>,
    allowed_hosts: &[String],
) -> Result<ValidatedTarget, IpcError> {
    let configured = console_base_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            IpcError::new(
                CODE_CONFIG_MISSING,
                "server.console_base_url is required for Console handoff",
            )
        })?;
    let mut origin = Url::parse(configured).map_err(|_| {
        IpcError::new(
            CODE_CONFIG_INVALID,
            "server.console_base_url must be an absolute HTTPS origin",
        )
    })?;
    if origin.scheme() != "https"
        || origin.host_str().is_none()
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.query().is_some()
        || origin.fragment().is_some()
        || !matches!(origin.path(), "" | "/")
    {
        return Err(IpcError::new(
            CODE_CONFIG_INVALID,
            "server.console_base_url must be an HTTPS origin without credentials, path, query, or fragment",
        ));
    }
    origin.set_path(CONSOLE_HANDOFF_PATH);
    validate(origin.as_str(), allowed_hosts).map_err(IpcError::from)
}

/// Issue one authenticated pending handoff and open the fixed Console route.
///
/// No command argument is accepted. One invocation can open at most one window,
/// and concurrent invocations are rejected before a second server request.
#[tauri::command]
pub async fn open_console_assignment_board(
    handoff_state: State<'_, ConsoleHandoffState>,
    config_state: State<'_, crate::runtime_state::ConfigRuntimeState>,
) -> Result<ConsoleHandoffIssue, IpcError> {
    let config = config_state.config_manager().get().server;
    let target = build_target(
        config.console_base_url.as_deref(),
        &config.allowed_handoff_hosts,
    )?;

    if handoff_state.in_flight.swap(true, Ordering::SeqCst) {
        return Err(IpcError::new(
            CODE_UNAVAILABLE,
            "a Console handoff is already in progress",
        ));
    }
    let _guard = InFlightGuard(&handoff_state.in_flight);
    let client = handoff_state.get().ok_or_else(|| {
        IpcError::new(
            CODE_UNAVAILABLE,
            "Console handoff transport is not wired in this build",
        )
    })?;
    let receipt = client
        .issue_console_handoff()
        .await
        .map_err(IpcError::from)?;
    launch_validated(target).await?;
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed() -> Vec<String> {
        vec!["console.example.com".to_string()]
    }

    #[test]
    fn target_is_fixed_and_has_no_query_fragment_or_identity() {
        let target = build_target(Some("https://console.example.com"), &allowed()).unwrap();
        assert_eq!(
            target.as_str(),
            "https://console.example.com/console/handoff"
        );
        for forbidden in ["?", "#", "actor", "organization", "token"] {
            assert!(!target.as_str().contains(forbidden));
        }
    }

    #[test]
    fn missing_origin_and_empty_allowlist_fail_closed() {
        assert_eq!(
            build_target(None, &allowed()).unwrap_err().code,
            CODE_CONFIG_MISSING
        );
        assert_eq!(
            build_target(Some("https://console.example.com"), &[])
                .unwrap_err()
                .code,
            "handoff.rejected"
        );
    }

    #[test]
    fn base_url_must_be_an_origin_not_a_caller_controlled_deep_link() {
        for invalid in [
            "http://console.example.com",
            "https://user@console.example.com",
            "https://console.example.com/org/other/workflows",
            "https://console.example.com?org=other",
            "https://console.example.com#token",
        ] {
            assert_eq!(
                build_target(Some(invalid), &allowed()).unwrap_err().code,
                CODE_CONFIG_INVALID,
                "{invalid}"
            );
        }
    }

    #[test]
    fn exact_host_allowlist_rejects_suffix_attack() {
        let error = build_target(Some("https://evil.console.example.com"), &allowed()).unwrap_err();
        assert_eq!(error.code, "handoff.rejected");
    }
}
