//! OAuth loopback callback server.
//!
//! Starts a temporary HTTP server on a fixed port to receive the OAuth
//! authorization code callback from the browser.

use axum::extract::Query;
use axum::response::Html;
use axum::routing::get;
use axum::Router;
use oneshim_core::error::CoreError;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{oneshot, Mutex};
use tracing::{debug, error, warn};

/// Result of a successful OAuth callback.
#[derive(Debug, Clone)]
pub struct CallbackResult {
    pub code: String,
    pub state: String,
}

/// Check whether a port is available for binding.
pub async fn check_port_available(port: u16) -> bool {
    tokio::net::TcpListener::bind(format!("127.0.0.1:{port}"))
        .await
        .map(|l| {
            drop(l);
            true
        })
        .unwrap_or(false)
}

/// Start the callback server and wait for the authorization code.
///
/// Returns when either:
/// - A valid callback is received with matching state
/// - The cancellation signal fires
/// - An error callback is received from the provider
pub async fn wait_for_callback(
    port: u16,
    expected_state: String,
    cancel_rx: oneshot::Receiver<()>,
) -> Result<CallbackResult, CoreError> {
    let (tx, rx) = oneshot::channel::<Result<CallbackResult, String>>();
    let tx = Arc::new(Mutex::new(Some(tx)));
    let expected = expected_state.clone();

    let tx_clone = tx.clone();
    let app = Router::new().route(
        "/auth/callback",
        get(move |Query(params): Query<HashMap<String, String>>| {
            let tx = tx_clone.clone();
            let expected = expected.clone();
            async move {
                let mut guard = tx.lock().await;

                // Check for error response from provider
                if let Some(err) = params.get("error") {
                    let desc = params.get("error_description").cloned().unwrap_or_default();
                    warn!("OAuth callback error: {err} — {desc}");
                    if let Some(sender) = guard.take() {
                        if let Err(e) = sender.send(Err(format!("{err}: {desc}"))) {
                            debug!("channel send failed: {e:?}");
                        }
                    }
                    return Html(error_html(err, &desc));
                }

                // Validate state parameter (CSRF protection)
                let state = params.get("state").cloned().unwrap_or_default();
                if state != expected {
                    warn!("OAuth callback state mismatch");
                    if let Some(sender) = guard.take() {
                        if let Err(e) = sender.send(Err("state mismatch".into())) {
                            debug!("channel send failed: {e:?}");
                        }
                    }
                    return Html(error_html(
                        "state_mismatch",
                        "The state parameter does not match. Please try again.",
                    ));
                }

                // Extract authorization code
                let code = match params.get("code") {
                    Some(c) => c.clone(),
                    None => {
                        if let Some(sender) = guard.take() {
                            if let Err(e) = sender.send(Err("missing code parameter".into())) {
                                debug!("channel send failed: {e:?}");
                            }
                        }
                        return Html(error_html(
                            "missing_code",
                            "No authorization code received.",
                        ));
                    }
                };

                debug!("OAuth callback received (code length: {})", code.len());
                if let Some(sender) = guard.take() {
                    if let Err(e) = sender.send(Ok(CallbackResult { code, state })) {
                        debug!("channel send failed: {e:?}");
                    }
                }

                Html(success_html())
            }
        }),
    );

    // Bind to the fixed port
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}"))
        .await
        .map_err(|e| CoreError::OAuthError {
            code: oneshim_core::error_codes::OAuthCode::Failed,
            provider: "callback".into(),
            message: format!("port {port} already in use (is Codex CLI running?): {e}"),
        })?;

    debug!("OAuth callback server listening on 127.0.0.1:{port}");

    // Run server until callback or cancellation.
    // Use a shared shutdown signal so we can stop the server once we have a result.
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let shutdown_tx = Arc::new(Mutex::new(Some(shutdown_tx)));

    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        if let Err(e) = shutdown_rx.await {
            debug!("operation failed: {e}");
        }
    });

    // Run server in background
    let server_handle = tokio::spawn(async move {
        if let Err(e) = server.await {
            error!("OAuth callback server error: {e}");
        }
    });

    let result = tokio::select! {
        result = rx => {
            match result {
                Ok(Ok(callback)) => Ok(callback),
                Ok(Err(e)) => Err(CoreError::OAuthError { code: oneshim_core::error_codes::OAuthCode::Failed, provider: "callback".into(),
                    message: e, }),
                Err(_) => Err(CoreError::OAuthError { code: oneshim_core::error_codes::OAuthCode::Failed, provider: "callback".into(),
                    message: "callback channel closed unexpectedly".into(), }),
            }
        }
        _ = cancel_rx => {
            debug!("OAuth callback server cancelled");
            Err(CoreError::OAuthError { code: oneshim_core::error_codes::OAuthCode::Failed, provider: "callback".into(),
                message: "flow cancelled by user".into(), })
        }
        _ = tokio::time::sleep(std::time::Duration::from_secs(300)) => {
            debug!("OAuth callback server timed out after 5 minutes");
            Err(CoreError::OAuthError { code: oneshim_core::error_codes::OAuthCode::Failed, provider: "callback".into(),
                message: "OAuth flow timed out — no browser callback received within 5 minutes".into(), })
        }
    };

    // Shut down the server
    if let Some(tx) = shutdown_tx.lock().await.take() {
        if let Err(e) = tx.send(()) {
            debug!("channel send failed: {e:?}");
        }
    }
    if let Err(e) = server_handle.await {
        debug!("operation failed: {e}");
    }

    result
}

fn success_html() -> String {
    r#"<!DOCTYPE html>
<html><head><title>Maekon — Authentication Complete</title>
<style>body{font-family:system-ui;display:flex;justify-content:center;align-items:center;min-height:100vh;margin:0;background:#f8f9fa}
.card{background:#fff;border-radius:12px;padding:2rem 3rem;box-shadow:0 2px 8px rgba(0,0,0,.1);text-align:center}
h1{color:#22c55e;margin:0 0 .5rem}p{color:#6b7280}</style></head>
<body><div class="card"><h1>&#10003; Connected</h1><p>You can close this tab and return to Maekon.</p></div></body></html>"#.to_string()
}

fn error_html(error: &str, description: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html><head><title>Maekon — Authentication Error</title>
<style>body{{font-family:system-ui;display:flex;justify-content:center;align-items:center;min-height:100vh;margin:0;background:#f8f9fa}}
.card{{background:#fff;border-radius:12px;padding:2rem 3rem;box-shadow:0 2px 8px rgba(0,0,0,.1);text-align:center}}
h1{{color:#ef4444;margin:0 0 .5rem}}p{{color:#6b7280}}</style></head>
<body><div class="card"><h1>&#10007; Error</h1><p>{error}: {description}</p><p>Please close this tab and try again in Maekon.</p></div></body></html>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn check_port_available_on_unused_port() {
        // High port is very likely available
        let available = check_port_available(19876).await;
        assert!(available);
    }

    #[test]
    fn success_html_contains_connected() {
        let html = success_html();
        assert!(html.contains("Connected"));
        assert!(html.contains("Maekon"));
    }

    #[test]
    fn error_html_contains_error_info() {
        let html = error_html("access_denied", "User cancelled");
        assert!(html.contains("access_denied"));
        assert!(html.contains("User cancelled"));
    }
}
