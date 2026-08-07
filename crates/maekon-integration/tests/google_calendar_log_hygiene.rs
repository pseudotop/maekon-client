//! Google Calendar connector log-leak prevention (MK-EXT-01.C01 #8590).
//!
//! Isolated in a separate test binary (= separate process) to safely install a
//! **process-global** tracing subscriber. Being global, it captures events emitted
//! from any thread, so it does not miss a `warn!` emitted across a reqwest/async boundary.
//!
//! Captures at the `WARN` level only — excluding the DEBUG/TRACE noise from
//! mockito/hyper/reqwest (including the test infrastructure echoing incoming request
//! headers verbatim), and takes only **the connector's own error-path logs** as the
//! verification target.
//!
//! Verifies: the 403 error response's **raw body** · **bearer token** · **token-bearing URL**
//! never appear in the connector logs, and only the bounded reason token
//! (`insufficientPermissions`) is exposed.

// The mockito `ServerGuard` must stay alive during the request, so we allow the early-drop suggestion.
#![allow(clippy::significant_drop_tightening)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use mockito::Matcher;

use maekon_core::error::CoreError;
use maekon_core::ports::oauth::{
    OAuthConnectionStatus, OAuthFlowHandle, OAuthFlowStatus, OAuthPort, RefreshResult,
};
use maekon_core::ports::work_context::{ContextSourcePort, SourceHealth, SyncOutcome, SyncRequest};
use maekon_integration::google_calendar::{
    GoogleCalendarConfig, GoogleCalendarConnector, HttpCalendarEventsApi,
    GOOGLE_CALENDAR_PROVIDER_ID,
};

/// Writer that collects logs into a process-global buffer.
static LOG_BUF: Mutex<Vec<u8>> = Mutex::new(Vec::new());

#[derive(Clone, Copy)]
struct CaptureWriter;

impl std::io::Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        LOG_BUF.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
    type Writer = CaptureWriter;
    fn make_writer(&'a self) -> Self::Writer {
        CaptureWriter
    }
}

/// Minimal OAuth double holding a token.
struct FakeOAuthPort {
    token: String,
}

#[async_trait]
impl OAuthPort for FakeOAuthPort {
    async fn start_flow(&self, _p: &str) -> Result<OAuthFlowHandle, CoreError> {
        Ok(OAuthFlowHandle {
            flow_id: "f".into(),
            auth_url: "u".into(),
        })
    }
    async fn flow_status(&self, _f: &str) -> Result<OAuthFlowStatus, CoreError> {
        Ok(OAuthFlowStatus::Completed)
    }
    async fn cancel_flow(&self, _f: &str) -> Result<(), CoreError> {
        Ok(())
    }
    async fn get_access_token(&self, _p: &str) -> Result<Option<String>, CoreError> {
        Ok(Some(self.token.clone()))
    }
    async fn revoke(&self, _p: &str) -> Result<(), CoreError> {
        Ok(())
    }
    async fn connection_status(&self, p: &str) -> Result<OAuthConnectionStatus, CoreError> {
        Ok(OAuthConnectionStatus {
            provider_id: p.to_string(),
            connected: true,
            expires_at: None,
            scopes: vec![],
            api_base_url: None,
            has_refresh_token: true,
        })
    }
    async fn refresh_access_token(&self, _p: &str, _m: i64) -> Result<RefreshResult, CoreError> {
        Ok(RefreshResult::AlreadyFresh {
            expires_at: String::new(),
        })
    }
}

#[tokio::test]
async fn error_response_body_and_token_never_reach_logs() {
    use tracing::Level;

    // Process-global subscriber — a warn! from any thread is captured into the global buffer.
    // Capture WARN only: verify only the connector error-path logs, excluding the noise of the
    // test infrastructure (e.g. mockito's DEBUG that echoes the incoming request's Authorization header).
    tracing_subscriber::fmt()
        .with_writer(CaptureWriter)
        .with_max_level(Level::WARN)
        .with_ansi(false)
        .init();

    let mut server = mockito::Server::new_async().await;
    let oauth = Arc::new(FakeOAuthPort {
        token: "BEARER_SECRET_TOKEN_xyz".into(),
    });
    let oauth_dyn: Arc<dyn OAuthPort> = oauth.clone();
    let api = Arc::new(
        HttpCalendarEventsApi::new(server.url(), oauth_dyn, GOOGLE_CALENDAR_PROVIDER_ID).unwrap(),
    );
    let conn = GoogleCalendarConnector::new(api, oauth, GoogleCalendarConfig::new("inst_1"));

    // Embed a sensitive string in the 403 response body.
    let _m = server
        .mock("GET", "/calendars/primary/events")
        .match_query(Matcher::Any)
        .with_status(403)
        .with_body(
            r#"{"error":{"code":403,"message":"BODY_SECRET_do_not_log",
                "errors":[{"reason":"insufficientPermissions"}]}}"#,
        )
        .create_async()
        .await;

    let request = SyncRequest {
        install_id: "inst_1".into(),
        account_subject_ref: "acct_1".into(),
        cursor: None,
        access_epoch_id: 1,
        max_records: 100,
    };
    let outcome = conn.sync(request).await.unwrap();
    assert!(matches!(
        outcome,
        SyncOutcome::Unhealthy(SourceHealth::Forbidden)
    ));

    let logs = {
        let guard = LOG_BUF.lock().unwrap();
        String::from_utf8_lossy(&guard).to_string()
    };
    assert!(
        !logs.is_empty(),
        "expected the connector to emit a structured warn for the 403"
    );
    // The raw error body and bearer token must never appear in the logs (no-leak invariant).
    assert!(
        !logs.contains("BODY_SECRET_do_not_log"),
        "raw error body leaked into logs:\n{logs}"
    );
    assert!(
        !logs.contains("BEARER_SECRET_TOKEN_xyz"),
        "bearer token leaked into logs:\n{logs}"
    );
    // Only the bounded reason token is exposed (allowed).
    assert!(
        logs.contains("insufficientPermissions"),
        "bounded reason token should be present:\n{logs}"
    );
    // The token-bearing URL is also absent from the logs.
    assert!(
        !logs.contains("syncToken="),
        "url/token leaked into logs:\n{logs}"
    );
}
