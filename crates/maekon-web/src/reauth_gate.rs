//! #8044: capture-history viewing re-authentication (re-auth) middleware.
//!
//! Requires a fresh OS biometric/PIN re-auth before serving **sensitive
//! capture-history surfaces** — the captured screenshot timeline, frame
//! images, capture search, bulk frame export, backup download, and the full
//! personal-data export.
//!
//! This is a different control from `require_local_auth` (the session
//! token): the token protects against *other local processes*, whereas this
//! gate protects against a *physical accessor* on an already-unlocked,
//! already-token-authenticated session — the same threat model as the
//! Windows Hello re-prompt Microsoft Recall added after backlash. Non-
//! capture-history `/api` paths pass through, so the dashboard/settings/
//! automation surfaces are unaffected.

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::AppState;

/// Capture-history viewing re-authentication gate middleware.
///
/// Fail-closed: when the gate is **enabled** (config opt-in, on by default)
/// and there is no valid re-auth session (never authenticated, or the idle
/// window expired), every gated path returns `403 auth.reauth_required` —
/// the machine code the frontend keys on to raise the biometric/PIN prompt.
/// When the gate is **disabled**
/// (`config.privacy.reauth.enabled=false`), `is_satisfied()` is always true,
/// so this passes through (an explicit user opt-out).
pub async fn require_capture_reauth(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    // CORS preflight carries no context and must reach the CorsLayer — never 403 it.
    if request.method() == axum::http::Method::OPTIONS {
        return next.run(request).await;
    }

    // Only capture-history surfaces are gated; everything else passes through.
    if !is_capture_history_path(request.uri().path()) {
        return next.run(request).await;
    }

    if state.auth.reauth_gate.is_satisfied() {
        next.run(request).await
    } else {
        crate::error::ApiError::Coded {
            status: StatusCode::FORBIDDEN.as_u16(),
            code: "auth.reauth_required".to_string(),
            message: "Re-authentication is required to view capture history.".to_string(),
        }
        .into_response()
    }
}

/// Whether `path` targets a sensitive capture-history surface protected by
/// the #8044 re-auth gate.
///
/// The `nest("/api", …)` boundary may or may not leave the `/api` prefix on
/// `request.uri().path()` depending on layer position, so an optional `/api`
/// prefix is stripped first (mirroring the dual-form matching in
/// `local_auth_query_allowed`). Per-frame subpaths (`/frames/{id}/image`,
/// annotations, tags) are covered by the `/frames/` prefix; the rest (the
/// bare `/frames` list, `/timeline`, `/export/frames`, `/backup`,
/// `/export/full`, `/search`, and `/semantic-search` — the actual query only, excluding
/// `/semantic-search/capabilities`) are matched exactly.
pub fn is_capture_history_path(path: &str) -> bool {
    let path = path.strip_prefix("/api").unwrap_or(path);
    path == "/frames"
        || path.starts_with("/frames/")
        || path == "/timeline"
        || path == "/export/frames"
        || path == "/backup"
        || path == "/export/full"
        || path == "/search"
        || path == "/semantic-search"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WebServer;
    use axum::body::Body;
    use axum::extract::connect_info::MockConnectInfo;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use maekon_core::reauth::CaptureReauthGate;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::Duration;
    use tower::ServiceExt;

    // These tests exercise the REAL production middleware
    // (require_capture_reauth) via the oneshot+MockConnectInfo harness — the
    // gate reads in-process shared state rather than the socket, so it is
    // transport-agnostic and faithfully reproduced by this harness.

    /// A production router that satisfies the local-auth gate + a
    /// caller-supplied re-auth gate.
    fn reauth_app(gate: Arc<CaptureReauthGate>) -> axum::Router {
        let mut state = crate::test_local_auth::test_app_state_with_event_capacity(8);
        state.auth.reauth_gate = gate;
        crate::test_local_auth::authed_loopback_router(state)
    }

    async fn status_of(app: axum::Router, req: Request<Body>) -> StatusCode {
        app.oneshot(req).await.unwrap().status()
    }

    async fn status_and_code(app: axum::Router, req: Request<Body>) -> (StatusCode, String) {
        let response = app.oneshot(req).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let code = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("code").and_then(|c| c.as_str()).map(str::to_string))
            .unwrap_or_default();
        (status, code)
    }

    fn get(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).body(Body::empty()).unwrap()
    }

    #[test]
    fn is_capture_history_path_matches_sensitive_surfaces() {
        // Both the bare and /api-prefixed forms must match (nest-boundary tolerance).
        for path in [
            "/frames",
            "/api/frames",
            "/frames/12/image",
            "/api/frames/12/image",
            "/frames/9/annotations",
            "/timeline",
            "/api/timeline",
            "/export/frames",
            "/backup",
            "/api/backup",
            "/export/full",
            "/search",
            "/semantic-search",
        ] {
            assert!(
                is_capture_history_path(path),
                "{path} must be gated as capture history"
            );
        }
    }

    #[test]
    fn is_capture_history_path_ignores_non_sensitive_surfaces() {
        for path in [
            "/metrics",
            "/api/metrics",
            "/events",
            "/stats/summary",
            "/settings",
            "/export/metrics",
            "/export/events",
            "/semantic-search/capabilities",
            "/dashboard/day",
            "/framesomething", // must NOT prefix-match /frames
        ] {
            assert!(!is_capture_history_path(path), "{path} must NOT be gated");
        }
    }

    #[tokio::test]
    async fn enabled_blocks_capture_history_until_authenticated() {
        // Enabled gate, not yet re-authenticated ⇒ capture history is fail-closed 403.
        let gate = Arc::new(CaptureReauthGate::new(true, Duration::from_secs(300)));
        let (status, code) = status_and_code(reauth_app(gate), get("/api/frames")).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(
            code, "auth.reauth_required",
            "gated capture-history must return the machine code the UI keys on"
        );
    }

    #[tokio::test]
    async fn enabled_allows_non_capture_history_paths() {
        // The gate must not affect the rest of the dashboard: /api/metrics
        // still passes with an enabled, unauthenticated gate.
        let gate = Arc::new(CaptureReauthGate::new(true, Duration::from_secs(300)));
        let status = status_of(reauth_app(gate), get("/api/metrics")).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn opens_gate_after_record_success() {
        let gate = Arc::new(CaptureReauthGate::new(true, Duration::from_secs(300)));
        // Simulate the Tauri command recording a successful biometric/PIN auth.
        gate.record_success();
        let status = status_of(reauth_app(gate), get("/api/frames")).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "after a recorded re-auth, capture history must be served"
        );
    }

    #[tokio::test]
    async fn relock_reblocks_capture_history() {
        // Idle expiry / foreground re-entry is modeled via lock() — should
        // return to fail-closed afterward.
        let gate = Arc::new(CaptureReauthGate::new(true, Duration::from_secs(300)));
        gate.record_success();
        gate.lock();
        let (status, code) = status_and_code(reauth_app(gate), get("/api/timeline")).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(code, "auth.reauth_required");
    }

    #[tokio::test]
    async fn disabled_gate_passes_through() {
        // A default (disabled) gate must not block capture history — an
        // explicit opt-out.
        let gate = Arc::new(CaptureReauthGate::disabled());
        let status = status_of(reauth_app(gate), get("/api/frames")).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn per_frame_image_subpath_is_gated() {
        // Confirms the most sensitive surface (the actual screenshot bytes)
        // is definitely gated.
        let gate = Arc::new(CaptureReauthGate::new(true, Duration::from_secs(300)));
        let (status, code) = status_and_code(reauth_app(gate), get("/api/frames/1/image")).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(code, "auth.reauth_required");
    }

    #[tokio::test]
    async fn runs_after_local_auth_gate() {
        // With no local-auth token, the request must 401 BEFORE the re-auth
        // gate is consulted (defense-in-depth ordering: the token gate is outer).
        let gate = Arc::new(CaptureReauthGate::new(true, Duration::from_secs(300)));
        let mut state = crate::test_local_auth::test_app_state_with_event_capacity(8);
        state.auth.local_auth_token = Some(Arc::from("secret"));
        state.auth.reauth_gate = gate;
        // Build WITHOUT the auto-injected local-auth header ⇒ the token gate
        // rejects first.
        let app = WebServer::build_router(state)
            .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
        let status = status_of(app, get("/api/frames")).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "the local-auth token gate must reject before the reauth gate"
        );
    }
}
