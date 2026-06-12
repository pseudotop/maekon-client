use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use maekon_core::config::TlsConfig;
use maekon_core::consent::{ConsentManager, ConsentPermissions};
use maekon_core::models::feature_performance::{feature_keys, FeaturePerfSample};
use maekon_core::ports::feature_perf::{time_feature, FeaturePerfRecorder, FeaturePerfSink};
use maekon_network::auth::TokenManager;
use maekon_network::feature_perf_uploader::FeaturePerfUploader;
use maekon_network::http_client::HttpApiClient;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

#[derive(Debug, Clone)]
struct CapturedPerfRequest {
    path: String,
    body: Value,
}

#[derive(Default)]
struct FeaturePerfContractState {
    requests: Mutex<Vec<CapturedPerfRequest>>,
    samples: Mutex<Vec<(String, FeaturePerfSample)>>,
}

#[derive(Clone)]
struct FeaturePerfContractServer {
    base_url: String,
    state: Arc<FeaturePerfContractState>,
    shutdown_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

impl FeaturePerfContractServer {
    async fn start() -> Self {
        let state = Arc::new(FeaturePerfContractState::default());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind feature performance contract server");
        let address = listener
            .local_addr()
            .expect("feature performance contract server addr");
        let base_url = format!("http://{address}");
        let router = Router::new()
            .route("/api/v1/auth/tokens", post(issue_token))
            .route(
                "/api/v1/system/features/{feature_key}/performance",
                post(record_feature_performance),
            )
            .route(
                "/__test/features/{feature_key}/performance",
                get(read_feature_performance),
            )
            .with_state(state.clone());
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("feature performance contract server run");
        });

        tokio::time::sleep(Duration::from_millis(30)).await;

        Self {
            base_url,
            state,
            shutdown_tx: Arc::new(Mutex::new(Some(shutdown_tx))),
        }
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    fn captured_requests(&self) -> Vec<CapturedPerfRequest> {
        self.state.requests.lock().clone()
    }
}

impl Drop for FeaturePerfContractServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.lock().take() {
            let _ = tx.send(());
        }
    }
}

async fn issue_token() -> impl IntoResponse {
    Json(json!({
        "access_token": "test_jwt",
        "refresh_token": "refresh_jwt",
        "expires_in": 3600
    }))
}

async fn record_feature_performance(
    Path(feature_key): Path<String>,
    State(state): State<Arc<FeaturePerfContractState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if auth != "Bearer test_jwt" {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "missing bearer"})),
        );
    }

    let Some(samples_value) = body.get("samples").cloned() else {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "missing samples"})),
        );
    };
    if body.as_object().is_none_or(|obj| obj.len() != 1) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "forbidden top-level field"})),
        );
    }

    let allowed_sample_fields = [
        "response_time_ms",
        "timestamp",
        "total_requests",
        "error_count",
    ];
    let Some(sample_values) = samples_value.as_array() else {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "samples must be array"})),
        );
    };
    if sample_values.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "empty samples"})),
        );
    }
    for sample in sample_values {
        let Some(obj) = sample.as_object() else {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({"error": "invalid sample"})),
            );
        };
        if obj
            .keys()
            .any(|key| !allowed_sample_fields.contains(&key.as_str()))
        {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({"error": "forbidden sample field"})),
            );
        }
    }

    let samples: Vec<FeaturePerfSample> = match serde_json::from_value(samples_value) {
        Ok(samples) => samples,
        Err(error) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({"error": error.to_string()})),
            );
        }
    };

    state.requests.lock().push(CapturedPerfRequest {
        path: format!("/api/v1/system/features/{feature_key}/performance"),
        body,
    });
    let recorded_count = samples.len();
    state.samples.lock().extend(
        samples
            .into_iter()
            .map(|sample| (feature_key.clone(), sample)),
    );

    (
        StatusCode::OK,
        Json(json!({
            "feature_key": feature_key,
            "recorded_count": recorded_count,
            "status": "recorded"
        })),
    )
}

async fn read_feature_performance(
    Path(feature_key): Path<String>,
    State(state): State<Arc<FeaturePerfContractState>>,
) -> impl IntoResponse {
    let matching: Vec<FeaturePerfSample> = state
        .samples
        .lock()
        .iter()
        .filter(|(key, _sample)| key == &feature_key)
        .map(|(_key, sample)| sample.clone())
        .collect();
    let sample_count = matching.len();
    let avg_response_time_ms = if sample_count == 0 {
        None
    } else {
        Some(
            matching
                .iter()
                .map(|sample| sample.response_time_ms)
                .sum::<f64>()
                / sample_count as f64,
        )
    };

    Json(json!({
        "feature_key": feature_key,
        "sample_count": sample_count,
        "avg_response_time_ms": avg_response_time_ms,
        "status": if sample_count == 0 { "no_data" } else { "ok" }
    }))
}

fn telemetry_consent() -> Arc<ConsentManager> {
    let dir = tempfile::tempdir().expect("temp consent dir");
    let path = dir.keep().join("consent.json");
    let manager = Arc::new(ConsentManager::new(path));
    manager
        .grant_consent(
            ConsentPermissions {
                telemetry: true,
                ..Default::default()
            },
            30,
        )
        .expect("grant telemetry consent");
    manager
}

#[tokio::test]
async fn uploader_flush_posts_to_server_contract_and_reader_returns_non_empty() {
    let server = FeaturePerfContractServer::start().await;
    let tls = TlsConfig {
        enabled: false,
        ..Default::default()
    };
    let token_manager = Arc::new(
        TokenManager::new_with_tls(server.base_url(), &tls, Some(Duration::from_secs(5)))
            .expect("token manager for feature performance contract server"),
    );
    token_manager
        .login_with_org("agent@example.com", "password", "org_A")
        .await
        .expect("login against feature performance contract server");
    let http_client = HttpApiClient::new_with_tls(
        server.base_url(),
        token_manager,
        Duration::from_secs(5),
        &tls,
    )
    .expect("http feature performance client")
    .with_max_retries(0);
    let sink: Arc<dyn FeaturePerfSink> = Arc::new(http_client);
    let uploader = Arc::new(FeaturePerfUploader::new(sink, telemetry_consent(), None));
    let recorder: Arc<dyn FeaturePerfRecorder> = uploader.clone();
    let timed_result: Result<&'static str, ()> =
        time_feature(Some(&recorder), feature_keys::LOCAL_SUGGESTIONS, async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Ok("feature-result")
        })
        .await;
    assert_eq!(timed_result, Ok("feature-result"));

    let report = uploader.flush().await;

    assert_eq!(report.uploaded, 1);
    assert_eq!(report.requeued, 0);
    assert_eq!(report.dropped, 0);
    assert_eq!(report.blocked, 0);
    assert_eq!(uploader.buffered_len(), 0);

    let requests = server.captured_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].path,
        "/api/v1/system/features/local-suggestions/performance"
    );
    let response_time_ms = requests[0]
        .body
        .pointer("/samples/0/response_time_ms")
        .and_then(Value::as_f64)
        .expect("client payload includes measured response_time_ms");
    assert!(
        response_time_ms > 0.0,
        "time_feature must record the measured feature invocation duration"
    );
    for forbidden in [
        "success_rate",
        "availability",
        "error_rate",
        "status",
        "health_score",
        "metric_id",
        "feature_id",
        "organization_id",
    ] {
        assert!(
            !requests[0].body.to_string().contains(forbidden),
            "client payload must not include forbidden field `{forbidden}`"
        );
    }

    let reader_body: Value = reqwest::Client::new()
        .get(format!(
            "{}/__test/features/{}/performance",
            server.base_url(),
            feature_keys::LOCAL_SUGGESTIONS
        ))
        .send()
        .await
        .expect("read feature performance")
        .json()
        .await
        .expect("feature performance reader JSON");
    assert_eq!(reader_body["sample_count"], json!(1));
    assert_eq!(reader_body["avg_response_time_ms"], json!(response_time_ms));
    assert_eq!(reader_body["status"], json!("ok"));
}
