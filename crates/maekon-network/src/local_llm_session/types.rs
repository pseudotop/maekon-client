use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio::sync::RwLock;

use maekon_core::config::AiSessionConfig;
use maekon_core::models::ai_session::{ChatMessage, ChatRole, SessionState};

pub(super) const MAX_ATTACHMENT_PREVIEW_BYTES: usize = 8 * 1024;
pub(super) const MAX_ATTACHMENT_PREVIEW_FILES: usize = 4;

/// #6205: Bound the connect phase so a black-holed Ollama endpoint (host that
/// silently drops SYNs rather than refusing) cannot stall the turn before any
/// bytes are exchanged. Generous because the server is normally on loopback or
/// LAN, but finite so a turn can never hang on connect.
pub(super) const OLLAMA_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// #6205: Per-read idle window for the streaming NDJSON body. This is a
/// `read_timeout`, NOT a total `.timeout()`: it resets after every successful
/// read, so a legitimately long generation that keeps emitting tokens is never
/// aborted. It fires only when a *connected-but-stalled* endpoint stops sending
/// bytes (no progress within the window), which would otherwise hang the turn
/// forever. Sized to comfortably cover slow-model first-token latency.
pub(super) const OLLAMA_READ_TIMEOUT: Duration = Duration::from_secs(120);

/// #6205: Build the HTTP client used for Ollama `/api/chat` streaming with a
/// connect timeout and a per-read idle timeout. Falls back to `Client::new()`
/// only if the builder somehow fails (it does not for plain timeout settings),
/// keeping the caller infallible.
pub(super) fn build_ollama_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(OLLAMA_CONNECT_TIMEOUT)
        .read_timeout(OLLAMA_READ_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Single NDJSON line from Ollama `/api/chat` with `stream: true`.
#[derive(Debug, Deserialize)]
pub(super) struct OllamaChatChunk {
    #[serde(default)]
    pub(super) message: Option<OllamaChunkMessage>,
    #[serde(default)]
    pub(super) done: bool,
    /// Token count for the generated response (present on final chunk).
    #[serde(default)]
    pub(super) eval_count: Option<u64>,
    /// Token count for the prompt (present on final chunk).
    #[serde(default)]
    pub(super) prompt_eval_count: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct OllamaChunkMessage {
    #[serde(default)]
    pub(super) content: String,
}

// ── LocalLlmSession ─────────────────────────────────────────────

pub struct LocalLlmSession {
    pub(super) session_id: String,
    pub(super) model: String,
    pub(super) base_url: String,
    pub(super) history: Arc<RwLock<Vec<ChatMessage>>>,
    /// Retained for session introspection; content is pre-seeded into `history`.
    #[allow(dead_code)]
    pub(super) system_prompt: Option<String>,
    pub(super) state: parking_lot::Mutex<SessionState>,
    pub(super) turn_count: AtomicU32,
    pub(super) created_at: DateTime<Utc>,
    pub(super) last_active: parking_lot::Mutex<Instant>,
    pub(super) http_client: reqwest::Client,
    pub(super) config: Arc<AiSessionConfig>,
}

impl LocalLlmSession {
    /// Create a new session targeting an Ollama-compatible server.
    pub fn new(
        session_id: String,
        model: String,
        base_url: String,
        system_prompt: Option<String>,
        config: Arc<AiSessionConfig>,
    ) -> Self {
        let mut initial_history = Vec::new();
        if let Some(ref prompt) = system_prompt {
            initial_history.push(ChatMessage {
                role: ChatRole::System,
                content: prompt.clone(),
                content_blocks: None,
            });
        }

        Self {
            session_id,
            model,
            base_url: base_url.trim_end_matches('/').to_string(),
            history: Arc::new(RwLock::new(initial_history)),
            system_prompt,
            state: parking_lot::Mutex::new(SessionState::Active),
            turn_count: AtomicU32::new(0),
            created_at: Utc::now(),
            last_active: parking_lot::Mutex::new(Instant::now()),
            // #6205: connect + per-read idle timeouts so a stalled Ollama
            // endpoint cannot hang the turn forever (see build_ollama_http_client).
            http_client: build_ollama_http_client(),
            config,
        }
    }
}
