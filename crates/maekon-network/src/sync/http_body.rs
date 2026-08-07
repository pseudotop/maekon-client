//! #6923/#6925: Shared response-body size cap helper (sync transport).
//!
//! `resp.bytes()`/`resp.text()`/`resp.json()` buffer the body without bound, so a corrupted/
//! misconfigured peer/relay that streams a multi-GB body while omitting/shrinking Content-Length
//! can OOM a 24/7 agent before the 30s timeout fires (the codebase's own #6917 threat model). This
//! helper reads the HTTP body into memory with a hard cap — it rejects an honest oversized
//! Content-Length early, then accumulates chunks and aborts the moment the running total exceeds
//! the cap (defense against an absent/shrunk Content-Length). fail-closed: read errors and cap
//! overruns both return `Err`.
//!
//! #6917 applied this to the remote_transport pull only; this lifts it to a shared sync-module
//! helper (pub(crate)) so LAN pull (#6923) + remote push/peer/error reads (#6925) get the same cap.

use maekon_core::error::CoreError;

/// Changeset data body cap. Much larger than a real incremental changeset, yet within the memory budget.
pub(crate) const MAX_PULL_RESPONSE_BYTES: u64 = 64 * 1024 * 1024; // 64 MiB

/// control-plane body cap (push-ack, error messages, peer list, verify response). Normal bodies
/// are a few KiB, so 1 MiB rejects no legitimate response while still preventing OOM.
pub(crate) const MAX_CONTROL_RESPONSE_BYTES: u64 = 1024 * 1024; // 1 MiB

/// Reads the HTTP response body into memory with a `cap`-byte upper bound. See the module doc for the full rationale.
///
/// #6939/#6940: The implementation delegates to `maekon_http_core::outbound::read_body_capped` (shared
/// network-wide) and maps its `BodyReadError` to sync's existing `CoreError::Network` —
/// consolidated into a single source so each pass leaves no sibling behind.
pub(crate) async fn read_body_capped(
    resp: reqwest::Response,
    cap: u64,
) -> Result<Vec<u8>, CoreError> {
    maekon_http_core::outbound::read_body_capped(resp, cap)
        .await
        .map_err(map_body_read_error)
}

fn map_body_read_error(e: maekon_http_core::outbound::BodyReadError) -> CoreError {
    let message = match e {
        maekon_http_core::outbound::BodyReadError::Transport(err) => {
            format!("read response body: {err}")
        }
        maekon_http_core::outbound::BodyReadError::TooLarge { len, cap } => {
            format!("response exceeded cap {cap} bytes (len {len})")
        }
    };
    CoreError::Network {
        code: maekon_core::error_codes::NetworkCode::Generic,
        message,
    }
}

/// Reads via `read_body_capped`, then converts to a lossy (UTF-8 lossy) string. Convenience helper
/// for control-plane paths that read error/ack bodies with a cap.
pub(crate) async fn read_text_capped(
    resp: reqwest::Response,
    cap: u64,
) -> Result<String, CoreError> {
    let bytes = read_body_capped(resp, cap).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// When the body exceeds the cap, fail-closed (Err) — prevents OOM from a compromised peer.
    #[tokio::test]
    async fn read_body_capped_rejects_oversized_body() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/capped")
            .with_status(200)
            .with_body(vec![b'x'; 100])
            .create_async()
            .await;
        let resp = reqwest::get(format!("{}/capped", server.url()))
            .await
            .expect("request");
        let result = read_body_capped(resp, 10).await;
        assert!(
            matches!(result, Err(CoreError::Network { .. })),
            "100-byte body with 10-byte cap must fail-closed: {result:?}"
        );
        mock.assert_async().await;
    }

    /// A body within the cap passes through unchanged (no false rejection).
    #[tokio::test]
    async fn read_body_capped_allows_within_cap() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/ok")
            .with_status(200)
            .with_body(vec![b'y'; 100])
            .create_async()
            .await;
        let resp = reqwest::get(format!("{}/ok", server.url()))
            .await
            .expect("request");
        let body = read_body_capped(resp, 1024)
            .await
            .expect("within-cap body must succeed");
        assert_eq!(body.len(), 100);
        mock.assert_async().await;
    }

    /// read_text_capped applies the same cap.
    #[tokio::test]
    async fn read_text_capped_rejects_oversized() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/t")
            .with_status(200)
            .with_body("hello world this is too long")
            .create_async()
            .await;
        let resp = reqwest::get(format!("{}/t", server.url()))
            .await
            .expect("request");
        let result = read_text_capped(resp, 5).await;
        assert!(matches!(result, Err(CoreError::Network { .. })));
        mock.assert_async().await;
    }
}
