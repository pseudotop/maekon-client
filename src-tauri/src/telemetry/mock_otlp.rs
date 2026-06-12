//! Test-only mock OTLP collector.
//!
//! Starts a minimal Axum server on a random loopback port that accepts POSTs to
//! `/v1/traces` AND `/v1/metrics` (E20-43 #4835), forwarding each request as a
//! `(Signal, Vec<u8>)` over an `mpsc::UnboundedChannel`. Used by T-X2-3 /
//! T-X2-10 (spans) and T-metrics-1 / T-metrics-2 (metrics).

#![cfg(all(test, feature = "telemetry"))]

use axum::{body::Bytes, http::StatusCode, routing::post, Router};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Which OTLP signal a received POST belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Signal {
    Traces,
    Metrics,
}

pub(super) struct MockCollector {
    /// Full OTLP/HTTP traces endpoint (includes `/v1/traces` suffix).
    /// `opentelemetry-otlp` 0.32 does NOT append the signal path when the user
    /// passes an explicit `with_endpoint(...)` — only when resolving from the
    /// `OTEL_EXPORTER_OTLP_ENDPOINT` env var — so tests that set
    /// `cfg.otlp_endpoint = Some(mock.endpoint)` must get the full traces path.
    pub endpoint: String,
    /// Receives `(signal, body_bytes)` for every accepted POST.
    pub rx: mpsc::UnboundedReceiver<(Signal, Vec<u8>)>,
}

pub(super) async fn start() -> MockCollector {
    let (tx, rx) = mpsc::unbounded_channel::<(Signal, Vec<u8>)>();
    let tx = Arc::new(tx);

    let app = Router::new()
        .route(
            "/v1/traces",
            post({
                let tx = Arc::clone(&tx);
                move |body: Bytes| async move {
                    let _ = tx.send((Signal::Traces, body.to_vec()));
                    StatusCode::OK
                }
            }),
        )
        .route(
            "/v1/metrics",
            post({
                let tx = Arc::clone(&tx);
                move |body: Bytes| async move {
                    let _ = tx.send((Signal::Metrics, body.to_vec()));
                    StatusCode::OK
                }
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    MockCollector {
        endpoint: format!("http://{addr}/v1/traces"),
        rx,
    }
}
