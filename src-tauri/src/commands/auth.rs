//! Authentication-related Tauri commands.
//!
//! OOS-TBD-N15-UI-EXPOSURE (2026-05-05): `logout_all_sessions` IPC entry point —
//! the Tauri command invoked by the UI "Sign out of all devices" button. It
//! calls server `DELETE /api/v1/auth/tokens/all` and clears local TokenManager
//! state.
//!
//! State pattern: `TokenManagerState` holds an interior-mutable slot
//! (`Mutex<Option<Arc<TokenManager>>>`) that is registered ONCE at build time
//! in `main.rs` and POPULATED at setup time once server bootstrap creates the
//! token manager. When `cfg(feature = "server")` is disabled (offline / demo)
//! or bootstrap never populates it, the slot stays `None` and the command
//! returns `IpcError` immediately.
//!
//! Why a slot instead of re-`manage()`-ing? Tauri's `Manager::manage()` does
//! NOT overwrite an already-managed type — it returns `false` and the value is
//! discarded. Tauri's own docs recommend wrapping the state in a `Mutex` and
//! using `Option` (rather than the deprecated/unsafe `unmanage()`) precisely so
//! the managed slot can be populated after the builder registers it once.

use std::sync::{Arc, Mutex};

use tauri::{command, State};

#[cfg(feature = "server")]
use maekon_network::auth::TokenManager;

/// Placeholder type when the `server` feature is disabled. Tauri State must
/// always be registered (`main.rs` manages a `TokenManagerState`), so this stub
/// keeps non-server builds compatible.
#[cfg(not(feature = "server"))]
pub struct TokenManager;

use crate::ipc_error::IpcError;

/// Tauri-managed wrapper around the optional `TokenManager`.
///
/// Registered exactly once at build time (`main.rs`) with an empty slot, then
/// populated at setup time once a real `TokenManager` exists. The inner slot is
/// `None` when:
/// - `cfg(feature = "server")` is disabled (offline / demo)
/// - server bootstrap failed (or never ran) before a token manager was created
///
/// In all of those cases, the Tauri command returns `IpcError` when invoked.
pub struct TokenManagerState(Mutex<Option<Arc<TokenManager>>>);

impl TokenManagerState {
    /// Build-time constructor: an empty slot registered once on the Tauri
    /// builder. Setup-time wiring later calls [`TokenManagerState::set`].
    #[must_use]
    pub fn empty() -> Self {
        Self(Mutex::new(None))
    }

    /// Populate the slot after server bootstrap. Replaces any previous value.
    ///
    /// Called from `app_runtime_launch` setup wiring. Because the state is
    /// registered ONCE at build time, this slot write is what actually makes
    /// the real `TokenManager` reachable from `logout_all_sessions` — a second
    /// `Manager::manage()` would be a silent no-op.
    ///
    /// Only the `server` build ever populates the slot; without the `server`
    /// feature there is no `TokenManager` to write and the slot stays empty.
    #[cfg(feature = "server")]
    pub fn set(&self, manager: Arc<TokenManager>) {
        // A poisoned lock here only means a prior holder panicked while the
        // (brief, non-await) critical section was held; recover the guard so
        // bootstrap still wires the manager rather than panicking the setup.
        let mut slot = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = Some(manager);
    }

    /// Read the current manager, cloning the `Arc` out of the lock so the guard
    /// is never held across an `.await`.
    #[must_use]
    pub fn get(&self) -> Option<Arc<TokenManager>> {
        let slot = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        slot.clone()
    }
}

/// Sign out of all devices.
///
/// OOS-TBD-N15-UI-EXPOSURE (2026-05-05): calls
/// `TokenManager.logout_all_sessions()`, which invokes server
/// `DELETE /api/v1/auth/tokens/all`, revokes all device tokens/sessions, and
/// clears local state.
///
/// This includes the current device, so the user must sign in again.
///
/// # Errors
///
/// - `cfg(feature = "server")` disabled: `IpcError`
/// - local state unavailable: `IpcError`
/// - other `CoreError` values: converted to `IpcError`
#[command]
pub async fn logout_all_sessions(state: State<'_, TokenManagerState>) -> Result<(), IpcError> {
    #[cfg(feature = "server")]
    {
        // Clone the Arc out of the slot first; never hold the lock across the
        // `.await` below.
        match state.get() {
            Some(tm) => tm.logout_all_sessions().await.map_err(IpcError::from),
            None => Err(IpcError::new(
                "auth.token_manager_unavailable",
                "TokenManager not initialized — server bootstrap likely failed",
            )),
        }
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = state.get();
        Err(IpcError::new(
            "auth.feature_disabled",
            "logout_all_sessions: server feature disabled in this build",
        ))
    }
}

/// Sign-in status handed to the frontend by both [`login`] and [`auth_status`].
///
/// Serialized field names are snake_case verbatim — there is no
/// `serde(rename_all)` here, so the frontend reads `server_feature` /
/// `organization_id` exactly as spelled below.
///
/// `server_feature` distinguishes "this build cannot talk to a server at all"
/// (offline / demo builds compiled without `cfg(feature = "server")`) from
/// "this build can, but nobody is signed in". The Settings Account section
/// needs that distinction to decide between hiding the sign-in form and
/// showing it.
#[derive(Debug, serde::Serialize)]
pub struct AuthStatusResponse {
    pub server_feature: bool,
    pub authenticated: bool,
    pub identifier: Option<String>,
    pub organization_id: Option<String>,
}

/// Shared status logic for [`auth_status`] and the tail of [`login`].
///
/// Split out of the `#[command]` fn because a `State<'_, _>` argument cannot be
/// constructed without a running Tauri app, so the command itself is not unit
/// testable — the same reason the `logout_all_sessions` tests exercise the
/// underlying call directly.
///
/// An empty slot degrades to "signed out" rather than erroring: this is a query
/// the Settings page issues on every render, and a failed server bootstrap
/// should still let the page paint.
///
/// `pub` so `tests/auth_status_mapping.rs` (#9492) can assert the
/// `SessionInfo` → response field mapping after a real login round trip; the
/// in-module unit tests only ever reach the signed-out branch, where a swapped
/// `identifier` / `organization_id` pair is indistinguishable (both `None`).
#[cfg(feature = "server")]
pub async fn auth_status_inner(state: &TokenManagerState) -> AuthStatusResponse {
    match state.get() {
        Some(tm) => {
            let authenticated = tm.is_authenticated().await;
            // Only surface session metadata while the token is live. Reading it
            // unconditionally would leak the previous identifier after expiry.
            let info = if authenticated {
                tm.session_info().await
            } else {
                None
            };
            AuthStatusResponse {
                server_feature: true,
                authenticated,
                identifier: info.as_ref().and_then(|i| i.identifier.clone()),
                organization_id: info.and_then(|i| i.organization_id),
            }
        }
        None => AuthStatusResponse {
            server_feature: true,
            authenticated: false,
            identifier: None,
            organization_id: None,
        },
    }
}

/// Shared login logic for [`login`]; see [`auth_status_inner`] for why this is
/// split out of the `#[command]` fn.
#[cfg(feature = "server")]
async fn login_inner(
    state: &TokenManagerState,
    identifier: &str,
    password: &str,
    organization_id: &str,
) -> Result<AuthStatusResponse, IpcError> {
    // Reject blanks before opening a socket. The password is checked for
    // emptiness only — it is never trimmed, since leading/trailing whitespace is
    // legitimate password material.
    if identifier.trim().is_empty() || password.is_empty() || organization_id.trim().is_empty() {
        return Err(IpcError::new(
            "auth.invalid_arguments",
            "identifier, password, and organization id are all required",
        ));
    }
    let Some(tm) = state.get() else {
        return Err(IpcError::new(
            "auth.token_manager_unavailable",
            "TokenManager not initialized — server bootstrap likely failed",
        ));
    };
    // NOTE: never log or format `password`. The error propagated below carries
    // the server's status plus its (length-capped) response body — the password
    // is only ever sent, never rendered into a message on this side.
    tm.login_with_org(identifier.trim(), password, organization_id.trim())
        .await
        .map_err(IpcError::from)?;
    Ok(auth_status_inner(state).await)
}

/// Sign in to the configured ONESHIM server (`server.base_url`).
///
/// On success the session is persisted by `TokenManager` itself (#9459), so the
/// caller does not need a follow-up save step.
///
/// # Errors
///
/// - blank `identifier`, `password`, or `organization_id`: `auth.invalid_arguments`
/// - slot never populated (server bootstrap failed): `auth.token_manager_unavailable`
/// - `cfg(feature = "server")` disabled: `auth.feature_disabled`
/// - rejected credentials / transport failures: the `CoreError` wire code from
///   `TokenManager::login_with_org`
#[command]
pub async fn login(
    state: State<'_, TokenManagerState>,
    identifier: String,
    password: String,
    organization_id: String,
) -> Result<AuthStatusResponse, IpcError> {
    #[cfg(feature = "server")]
    {
        login_inner(&state, &identifier, &password, &organization_id).await
    }
    #[cfg(not(feature = "server"))]
    {
        // Consume every argument so the `unused` deny does not fire. The
        // password is dropped here without ever being logged or formatted.
        let _ = (state.get(), identifier, password, organization_id);
        Err(IpcError::new(
            "auth.feature_disabled",
            "login: server feature disabled in this build",
        ))
    }
}

/// Current sign-in status for the Settings Account section.
///
/// # Errors
///
/// Infallible in practice — the `Result` exists so the Tauri IPC signature can
/// still carry an `IpcError` if this grows a fallible path.
#[command]
pub async fn auth_status(
    state: State<'_, TokenManagerState>,
) -> Result<AuthStatusResponse, IpcError> {
    #[cfg(feature = "server")]
    {
        Ok(auth_status_inner(&state).await)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = state.get();
        Ok(AuthStatusResponse {
            server_feature: false,
            authenticated: false,
            identifier: None,
            organization_id: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn token_manager_state_empty_resolves_to_none() {
        let state = TokenManagerState::empty();
        assert!(state.get().is_none());
    }

    // Regression (2nd-pass #22): the build-time slot is registered ONCE and
    // populated later via `set()`. This mirrors the real wiring — `main.rs`
    // registers `TokenManagerState::empty()`, then setup calls `set(..)` — and
    // asserts the slot resolves to `Some` afterwards. Before the slot fix, the
    // setup-time `Manager::manage(Some(..))` was a silent no-op, so the command
    // always saw `None` and `DELETE /api/v1/auth/tokens/all` never ran.
    #[cfg(feature = "server")]
    #[test]
    #[allow(deprecated)] // TokenManager::new used intentionally in tests
    fn token_manager_state_set_populates_slot() {
        let state = TokenManagerState::empty();
        assert!(state.get().is_none(), "slot must start empty");

        let tm = Arc::new(maekon_network::auth::TokenManager::new(
            "http://127.0.0.1:19999",
        ));
        state.set(tm.clone());

        let resolved = state
            .get()
            .expect("slot must resolve to Some after set() — the bootstrap wiring path");
        assert!(
            Arc::ptr_eq(&resolved, &tm),
            "the populated slot must hand back the exact bootstrap TokenManager"
        );
    }

    // F1 (P1): Security IPC test — `Some(tm)` path in `logout_all_sessions`.
    //
    // The Tauri command takes `State<'_, TokenManagerState>` so it cannot be
    // called directly in a unit test without a running Tauri app. Instead we
    // exercise the *identical* logic by calling `TokenManager::logout_all_sessions`
    // directly and asserting the `IpcError` mapping:
    //
    //   Some(tm) => tm.logout_all_sessions().await.map_err(IpcError::from)
    //
    // This is the security-critical path: a live `TokenManager` invokes
    // `DELETE /api/v1/auth/tokens/all`, then unconditionally clears local state.
    // The server call is fire-and-forget; `logout_all_sessions` always returns
    // `Ok(())` regardless of server reachability (same semantics as `logout`).
    #[cfg(feature = "server")]
    #[tokio::test]
    #[allow(deprecated)] // TokenManager::new used intentionally in tests
    async fn logout_all_sessions_some_tm_path_always_succeeds() {
        // Construct a TokenManager pointing to a deliberately unreachable URL.
        // The server call will fail silently (warn-logged); local state is cleared.
        let tm = maekon_network::auth::TokenManager::new("http://127.0.0.1:19999");
        let tm_arc = Arc::new(tm);

        // This mirrors the `Some(tm) => tm.logout_all_sessions().await.map_err(IpcError::from)`
        // branch in the Tauri command, exercising the security-critical code path.
        let result: Result<(), IpcError> =
            tm_arc.logout_all_sessions().await.map_err(IpcError::from);

        // logout_all_sessions unconditionally returns Ok(()) — server failures are
        // warn-logged and ignored; local token state is cleared in all cases.
        result.expect(
            "logout_all_sessions Some(tm) path must succeed regardless of server reachability",
        );
    }

    // F1 (P1): IpcError wire-code propagation from CoreError through the TokenManager.
    //
    // Verifies that CoreError → IpcError conversion preserves the wire code so the
    // frontend receives a structured `{"code": "...", "message": "..."}` payload
    // instead of a raw string. This is the conversion that `map_err(IpcError::from)`
    // inside the Some(tm) branch would perform if logout_all_sessions could fail.
    #[cfg(feature = "server")]
    #[test]
    fn logout_all_sessions_ipc_error_conversion_preserves_wire_code() {
        let core = maekon_core::error::CoreError::Auth {
            code: maekon_core::error_codes::AuthCode::Failed,
            message: "forced test error for IpcError mapping verification".into(),
        };
        let ipc: IpcError = IpcError::from(core);
        assert_eq!(
            ipc.code, "auth.failed",
            "CoreError::Auth → IpcError must preserve 'auth.failed' wire code"
        );
        assert!(
            ipc.message.contains("auth.failed"),
            "IpcError message must embed the wire code per ADR-019 Display convention"
        );
    }

    // #9492 item 1: pin the exact `IpcError.message` shape the webview matches
    // on for a rejected login.
    //
    // The Settings sign-in form maps `auth.failed` to one of two sentences —
    // "check your credentials" for a 401, a generic "server refused or could
    // not be reached" otherwise — and it tells them apart by pattern-matching
    // this string (`LOGIN_FAILURE_401` in
    // `crates/maekon-web/frontend/src/i18n/tauriIpcErrors.ts`).
    //
    // Two formatters compose here, and the first review of #9492 got it wrong
    // by assuming only one: `login_with_org` builds
    // `login failure ({status}): {body}`, then `IpcError::from` stores
    // `CoreError`'s *Display*, which prefixes `Authentication error [{code}]: `.
    // A matcher anchored at `login failure` therefore never fires and the 401
    // branch is dead code. Asserting the full string here means any change to
    // either format breaks this test loudly instead of silently degrading the
    // toast to the generic sentence.
    #[cfg(feature = "server")]
    #[test]
    fn ipc_error_message_shape_matches_frontend_401_matcher() {
        let body = r#"{"type":"about:blank","title":"Unauthorized","status":401}"#;
        let core = maekon_core::error::CoreError::Auth {
            code: maekon_core::error_codes::AuthCode::Failed,
            // Verbatim `login_with_org` construction for a 401 response.
            message: format!("login failure (401 Unauthorized): {body}"),
        };
        let ipc: IpcError = IpcError::from(core);

        assert_eq!(ipc.code, "auth.failed");
        assert_eq!(
            ipc.message,
            format!("Authentication error [auth.failed]: login failure (401 Unauthorized): {body}"),
            "the webview's 401 matcher keys off this exact prefix chain — changing either \
             `CoreError::Auth`'s Display or `login_with_org`'s message requires updating \
             `LOGIN_FAILURE_401` in tauriIpcErrors.ts in the same change"
        );
        assert!(
            !ipc.message.starts_with("login failure"),
            "regression guard: the message is Display-wrapped, so a matcher anchored at \
             `login failure` would never fire"
        );
    }

    // #9459: argument validation must short-circuit BEFORE any network call.
    // The manager below points at an unreachable loopback port, so if the guard
    // regressed into a no-op the call would hang on connect and then surface a
    // transport error code instead of `auth.invalid_arguments`.
    #[cfg(feature = "server")]
    #[tokio::test]
    #[allow(deprecated)] // TokenManager::new used intentionally in tests
    async fn login_command_rejects_empty_fields_before_network() {
        let state = TokenManagerState::empty();
        state.set(Arc::new(maekon_network::auth::TokenManager::new(
            "http://127.0.0.1:19999",
        )));

        let err = login_inner(&state, "", "pw", "org")
            .await
            .expect_err("empty identifier must be rejected");
        assert_eq!(err.code, "auth.invalid_arguments");

        let err = login_inner(&state, "user", "", "org")
            .await
            .expect_err("empty password must be rejected");
        assert_eq!(err.code, "auth.invalid_arguments");

        let err = login_inner(&state, "user", "pw", "")
            .await
            .expect_err("empty organization id must be rejected");
        assert_eq!(err.code, "auth.invalid_arguments");
    }

    // #9459: `auth_status` is what the Settings Account section polls. A freshly
    // constructed manager holds no token, so the response must report the server
    // feature as present but the session as signed out — never leak a stale
    // identifier from a previous run.
    #[cfg(feature = "server")]
    #[tokio::test]
    #[allow(deprecated)] // TokenManager::new used intentionally in tests
    async fn auth_status_reflects_unauthenticated_manager() {
        let state = TokenManagerState::empty();
        state.set(Arc::new(maekon_network::auth::TokenManager::new(
            "http://127.0.0.1:19999",
        )));

        let status = auth_status_inner(&state).await;
        assert!(status.server_feature);
        assert!(!status.authenticated);
        assert!(status.identifier.is_none());
        assert!(status.organization_id.is_none());
    }

    // #9459: the slot stays empty when server bootstrap never ran. `login` must
    // fail with the same `auth.token_manager_unavailable` code that
    // `logout_all_sessions` already returns for that state, rather than panicking
    // or silently reporting success.
    #[cfg(feature = "server")]
    #[tokio::test]
    async fn login_without_token_manager_reports_unavailable() {
        let state = TokenManagerState::empty();

        let err = login_inner(&state, "user", "pw", "org")
            .await
            .expect_err("an empty slot must not be treated as a successful login");
        assert_eq!(err.code, "auth.token_manager_unavailable");
    }

    // #9459: same empty-slot state via the status path. This is a query, not a
    // mutation, so it must degrade to "signed out" instead of erroring — the
    // Settings page still needs to render.
    #[cfg(feature = "server")]
    #[tokio::test]
    async fn auth_status_without_token_manager_reports_signed_out() {
        let state = TokenManagerState::empty();

        let status = auth_status_inner(&state).await;
        assert!(status.server_feature);
        assert!(!status.authenticated);
        assert!(status.identifier.is_none());
        assert!(status.organization_id.is_none());
    }

    /// The exact JSON keys the webview reads off `AuthStatusResponse`.
    fn expected_wire_keys() -> BTreeSet<&'static str> {
        BTreeSet::from([
            "authenticated",
            "identifier",
            "organization_id",
            "server_feature",
        ])
    }

    // #9459 wire-contract pin. `AuthStatusResponse` carries no
    // `serde(rename_all)`, so these snake_case names are what crosses the IPC
    // boundary verbatim. Nothing else fails if someone later adds
    // `rename_all = "camelCase"` or a new field — the Rust side still compiles
    // and the TypeScript side still type-checks against its own hand-written
    // interface — so this set-equality assertion is the only thing standing
    // between such a change and a silently broken Settings Account section.
    //
    // Deliberately NOT gated on `cfg(feature = "server")`: the struct is defined
    // unconditionally and non-server builds serialize it too, so the contract
    // must hold in both configurations.
    #[test]
    fn auth_status_response_serializes_exact_snake_case_wire_keys() {
        let response = AuthStatusResponse {
            server_feature: true,
            authenticated: true,
            identifier: Some("someone@example.com".to_string()),
            organization_id: Some("org-1".to_string()),
        };

        let value = serde_json::to_value(&response).expect("AuthStatusResponse must serialize");
        let object = value
            .as_object()
            .expect("AuthStatusResponse must serialize as a JSON object");

        let keys: BTreeSet<&str> = object.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            expected_wire_keys(),
            "AuthStatusResponse wire keys are consumed verbatim by the frontend — \
             renaming or adding one requires updating the Task 4 consumer in the same change"
        );

        assert_eq!(object["server_feature"], serde_json::json!(true));
        assert_eq!(object["authenticated"], serde_json::json!(true));
        assert_eq!(
            object["identifier"],
            serde_json::json!("someone@example.com")
        );
        assert_eq!(object["organization_id"], serde_json::json!("org-1"));
    }

    // #9459: a signed-out response must still carry all four keys, with the
    // absent session fields as explicit `null`. There is no
    // `skip_serializing_if` on the `Option` fields, and the frontend types them
    // as `string | null` rather than optional properties — dropping the keys
    // would turn a "signed out" read into an undefined-property access.
    #[test]
    fn auth_status_response_serializes_absent_session_as_null_not_missing() {
        let response = AuthStatusResponse {
            server_feature: false,
            authenticated: false,
            identifier: None,
            organization_id: None,
        };

        let value = serde_json::to_value(&response).expect("AuthStatusResponse must serialize");
        let object = value
            .as_object()
            .expect("AuthStatusResponse must serialize as a JSON object");

        let keys: BTreeSet<&str> = object.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            expected_wire_keys(),
            "a signed-out response must carry the same four keys as a signed-in one"
        );

        assert!(
            object["identifier"].is_null(),
            "absent identifier must serialize as null, not be omitted"
        );
        assert!(
            object["organization_id"].is_null(),
            "absent organization_id must serialize as null, not be omitted"
        );
        assert_eq!(object["server_feature"], serde_json::json!(false));
        assert_eq!(object["authenticated"], serde_json::json!(false));
    }
}
