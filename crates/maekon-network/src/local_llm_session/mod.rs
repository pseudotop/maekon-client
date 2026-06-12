//! LocalLlmSession — Ollama `/api/chat` conversation adapter with NDJSON streaming.
//!
//! Self-managed conversation history targeting local LLM servers.
//! Streams responses line-by-line (NDJSON), mapping Ollama token usage
//! fields (`eval_count` / `prompt_eval_count`) to `TokenUsage`.

mod helpers;
mod session;
mod types;

pub use types::LocalLlmSession;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
