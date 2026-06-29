// Ollama instance discovery helpers — used by the LocalLlm session factory
// (src-tauri/src/session_manager/factory.rs) to negotiate the best available
// model and detect non-Ollama OpenAI-compatible servers before the first chat
// message.
//
// Cross-reference: maekon-web's `ai_model_catalog_web_service` exposes a
// similar model-list surface for the dashboard UI via the
// `ProviderModelCatalogPort` / `ReqwestProviderModelCatalogClient` transport,
// which routes through the web server's request context and authentication
// resolution.  That path is intentionally separate: pulling a web-adapter
// dependency into session factory wiring would violate the Hexagonal rule
// (adapter→adapter coupling).  These helpers are a thin, standalone HTTP probe
// that has no web-layer dependency.
//
// Timeouts: both helpers default to 2 seconds.  Callers may pass a shorter
// duration; neither helper blocks the Tokio executor (reqwest is async).

use std::time::Duration;

/// Result of probing `GET {base}/api/version` — the endpoint that is unique
/// to Ollama.  Every variant represents a valid runtime state; none is an
/// error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OllamaProbe {
    /// The endpoint responded with a JSON body containing a `version` field.
    /// This is a strong signal that the process is an Ollama daemon.
    Ollama { version: String },
    /// The TCP connection succeeded but the response was either a non-2xx
    /// status or the body was not valid Ollama JSON.  Likely an OpenAI-
    /// compatible server (LM Studio, vLLM, LocalAI, etc.).
    NotOllama,
    /// The TCP connection itself failed — the daemon is likely not running.
    Unreachable,
}

/// Probe `GET {base_url}/api/version` to determine whether the endpoint is an
/// Ollama daemon.
///
/// * `Ollama { version }` — 2xx + JSON with a `version` string field.
/// * `NotOllama` — connected but the response is not Ollama-shaped (e.g. a
///   plain OpenAI-compatible server).
/// * `Unreachable` — TCP/DNS failure; Ollama is likely not running.
///
/// The probe never panics and never returns `Err`.  It is a best-effort
/// auxiliary signal; the caller must remain fail-open (continue session
/// creation regardless of the result).
///
/// One-time cost per session creation; bounded by `timeout` (default 2 s).
pub async fn probe_ollama(base_url: &str, timeout: Duration) -> OllamaProbe {
    let url = format!("{}/api/version", base_url.trim_end_matches('/'));
    let client = match reqwest::Client::builder().timeout(timeout).build() {
        Ok(c) => c,
        Err(_) => return OllamaProbe::Unreachable,
    };
    let response = match client.get(&url).send().await {
        Ok(r) => r,
        Err(_) => return OllamaProbe::Unreachable,
    };
    if !response.status().is_success() {
        return OllamaProbe::NotOllama;
    }
    // #6949: cap Ollama version-probe response body (OOM guard)
    let body =
        match crate::outbound::read_text_capped(response, crate::outbound::MAX_AI_RESPONSE_BYTES)
            .await
        {
            Ok(b) => b,
            Err(_) => return OllamaProbe::NotOllama,
        };
    // Ollama returns e.g. {"version":"0.6.8"}.  Any JSON object with a
    // non-empty string `version` field is accepted.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
        if let Some(version) = v.get("version").and_then(|s| s.as_str()) {
            if !version.is_empty() {
                return OllamaProbe::Ollama {
                    version: version.to_string(),
                };
            }
        }
    }
    OllamaProbe::NotOllama
}

/// Fetch the list of installed model names from `GET {base_url}/api/tags`.
///
/// Returns `Some(names)` on success — each entry is the `name` field from
/// Ollama's response (e.g. `"qwen3:8b"`, `"llama3:latest"`).  Returns `None`
/// on any error (connection failure, non-2xx status, parse failure).
///
/// The caller should treat `None` as "no information available" and skip
/// model negotiation entirely.
pub async fn list_installed_models(base_url: &str, timeout: Duration) -> Option<Vec<String>> {
    let url = format!("{}/api/tags", base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder().timeout(timeout).build().ok()?;
    let response = client.get(&url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    // #6949: cap Ollama model-list response body (OOM guard)
    let body = crate::outbound::read_text_capped(response, crate::outbound::MAX_AI_RESPONSE_BYTES)
        .await
        .ok()?;
    // Ollama response: {"models":[{"name":"qwen3:8b",...},...]}
    let parsed: serde_json::Value = serde_json::from_str(&body).ok()?;
    let models = parsed.get("models")?.as_array()?;
    let names: Vec<String> = models
        .iter()
        .filter_map(|m| m.get("name")?.as_str().map(str::to_string))
        .collect();
    Some(names)
}

// ── Unit tests ────────────────────────────────────────────────────────────────
//
// All HTTP calls use mockito OS-allocated ports.  No real Ollama daemon is
// contacted.

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const TIMEOUT: Duration = Duration::from_secs(2);

    // ── probe_ollama ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn probe_ollama_returns_version_on_200_with_version_field() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/version")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"version":"0.6.8"}"#)
            .create_async()
            .await;

        let result = probe_ollama(&server.url(), TIMEOUT).await;
        assert_eq!(
            result,
            OllamaProbe::Ollama {
                version: "0.6.8".to_string()
            }
        );
    }

    #[tokio::test]
    async fn probe_ollama_returns_not_ollama_on_404() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/version")
            .with_status(404)
            .with_body("Not Found")
            .create_async()
            .await;

        let result = probe_ollama(&server.url(), TIMEOUT).await;
        assert_eq!(result, OllamaProbe::NotOllama);
    }

    #[tokio::test]
    async fn probe_ollama_returns_not_ollama_on_malformed_json() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/version")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"status":"ok"}"#) // no "version" field
            .create_async()
            .await;

        let result = probe_ollama(&server.url(), TIMEOUT).await;
        assert_eq!(result, OllamaProbe::NotOllama);
    }

    #[tokio::test]
    async fn probe_ollama_returns_unreachable_on_connection_refused() {
        // Bind to a random OS port and immediately drop the listener — any
        // connect attempt will be refused (ECONNREFUSED), not time out.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let url = format!("http://127.0.0.1:{port}");
        // Use a short timeout so the test is fast even if the OS delays.
        let result = probe_ollama(&url, Duration::from_millis(500)).await;
        assert_eq!(result, OllamaProbe::Unreachable);
    }

    // ── list_installed_models ─────────────────────────────────────────────────

    #[tokio::test]
    async fn list_installed_models_returns_names_on_success() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/tags")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"models":[
                    {"name":"qwen3:8b","size":123},
                    {"name":"llama3:latest","size":456}
                ]}"#,
            )
            .create_async()
            .await;

        let result = list_installed_models(&server.url(), TIMEOUT).await;
        assert_eq!(
            result,
            Some(vec!["qwen3:8b".to_string(), "llama3:latest".to_string()])
        );
    }

    #[tokio::test]
    async fn list_installed_models_returns_empty_vec_for_no_models() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/tags")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"models":[]}"#)
            .create_async()
            .await;

        let result = list_installed_models(&server.url(), TIMEOUT).await;
        assert_eq!(result, Some(vec![]));
    }

    #[tokio::test]
    async fn list_installed_models_returns_none_on_malformed_body() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/tags")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"not json"#)
            .create_async()
            .await;

        let result = list_installed_models(&server.url(), TIMEOUT).await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn list_installed_models_returns_none_on_non_200() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/tags")
            .with_status(503)
            .with_body("Service Unavailable")
            .create_async()
            .await;

        let result = list_installed_models(&server.url(), TIMEOUT).await;
        assert_eq!(result, None);
    }
}
