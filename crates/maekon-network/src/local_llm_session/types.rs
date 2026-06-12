use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio::sync::RwLock;

use maekon_core::config::AiSessionConfig;
use maekon_core::models::ai_session::{ChatMessage, ChatRole, SessionState};

pub(super) const MAX_ATTACHMENT_PREVIEW_BYTES: usize = 8 * 1024;
pub(super) const MAX_ATTACHMENT_PREVIEW_FILES: usize = 4;

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
            http_client: reqwest::Client::new(),
            config,
        }
    }
}
