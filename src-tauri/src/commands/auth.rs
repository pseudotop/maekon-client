//! Authentication-related Tauri commands.
//!
//! OOS-TBD-N15-UI-EXPOSURE (2026-05-05): `logout_all_sessions` IPC entry point —
//! UI 측의 "Sign out of all devices" 버튼이 호출하는 Tauri command. server
//! `DELETE /api/v1/auth/tokens/all` 호출 + local TokenManager state clear.
//!
//! State pattern: `TokenManagerState(Option<Arc<TokenManager>>)` 를 별도 manage().
//! `cfg(feature = "server")` 비활성화 (offline / demo) 시 None — command 가 즉시
//! `IpcError` 반환.

use std::sync::Arc;

use tauri::{command, State};

#[cfg(feature = "server")]
use maekon_network::auth::TokenManager;

/// Placeholder type when `server` feature 가 비활성. Tauri State 가 항상 등록되어야
/// 하므로 (main.rs `.manage(TokenManagerState(None))`) 빌드 호환을 위해 stub 정의.
#[cfg(not(feature = "server"))]
pub struct TokenManager;

use crate::ipc_error::IpcError;

/// Tauri-managed wrapper around the optional `TokenManager`.
///
/// `None` 인 경우:
/// - `cfg(feature = "server")` 비활성화 빌드 (offline / demo)
/// - server bootstrap 실패로 token_manager 가 만들어지지 않은 경우
///
/// 두 경우 모두 Tauri command 가 호출 시 `IpcError` 반환.
pub struct TokenManagerState(pub Option<Arc<TokenManager>>);

/// Sign out of all devices.
///
/// OOS-TBD-N15-UI-EXPOSURE (2026-05-05): `TokenManager.logout_all_sessions()` 호출
/// → server `DELETE /api/v1/auth/tokens/all` → 모든 디바이스 토큰/세션 폐기 + local
/// state clear.
///
/// 본 디바이스 포함 모든 디바이스 logout — 호출 후 사용자는 재로그인 필요.
///
/// # Errors
///
/// - `cfg(feature = "server")` 비활성화 시: `IpcError`
/// - server 호출 실패: 무시 (local state cleared, `TokenManager.logout_all_sessions` 정책)
/// - 그 외 `CoreError` → `IpcError`
#[command]
pub async fn logout_all_sessions(
    state: State<'_, TokenManagerState>,
) -> Result<(), IpcError> {
    #[cfg(feature = "server")]
    {
        match state.0.as_ref() {
            Some(tm) => tm
                .logout_all_sessions()
                .await
                .map_err(IpcError::from),
            None => Err(IpcError::new(
                "auth.token_manager_unavailable",
                "TokenManager not initialized — server bootstrap likely failed",
            )),
        }
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = state; // suppress unused-variable warning
        Err(IpcError::new(
            "auth.feature_disabled",
            "logout_all_sessions: server feature disabled in this build",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_manager_state_none_constructs() {
        let state = TokenManagerState(None);
        assert!(state.0.is_none());
    }

    // OOS-TBD-N15-UI-EXPOSURE: Integration test (실 TokenManager + mockito server)
    // 는 별도 sprint 에서 추가 — Tauri State 주입 fixture 가 복잡하므로 본 sprint
    // 는 unit test 만 (TokenManager.logout_all_sessions 자체는 maekon-network/src/auth.rs
    // 에서 3 신규 unit test 검증 — PR #1351).
}
