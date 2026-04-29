mod crypto;
mod helpers;
mod service;
mod service_execution;
mod types;

// ── Public re-exports (external API) ────────────────────────────────
pub use service::GuiInteractionService;
pub use types::{
    GuiConfirmRequest, GuiCreateSessionRequest, GuiCreateSessionResponse, GuiExecutionOutcome,
    GuiExecutionPlan, GuiExecutionRequest, GuiHighlightRequest, GuiInteractionError,
};

// ── Test-only re-exports (child `mod tests` accesses via `use super::*`) ──
#[cfg(test)]
use crate::controller::AutomationAction;
#[cfg(test)]
use chrono::{Duration as ChronoDuration, Utc};
#[cfg(test)]
use crypto::*;
#[cfg(test)]
use helpers::*;
#[cfg(test)]
use oneshim_core::error::CoreError;
#[cfg(test)]
use oneshim_core::models::gui::{
    ExecutionBinding, GuiActionRequest, GuiCandidate, GuiExecutionTicket, GuiSessionState,
    HighlightRequest,
};
#[cfg(test)]
use oneshim_core::ports::element_finder::ElementFinder;
#[cfg(test)]
use oneshim_core::ports::focus_probe::FocusProbe;
#[cfg(test)]
use oneshim_core::ports::overlay_driver::OverlayDriver;
#[cfg(test)]
use std::sync::atomic::Ordering;
#[cfg(test)]
use std::sync::Arc;

// ── Constants (used by sub-modules via `super::` and tests via `use super::*`) ──
const GUI_HMAC_SECRET_ENV: &str = "ONESHIM_GUI_TICKET_HMAC_SECRET";
const DEFAULT_MAX_CANDIDATES: usize = 20;
const DEFAULT_MIN_CONFIDENCE: f64 = 0.5;
const DEFAULT_SESSION_TTL_SECS: i64 = 300;
const DEFAULT_TICKET_TTL_SECS: i64 = 30;
const CLEANUP_INTERVAL_SECS: u64 = 30;
const GUI_EVENT_CHANNEL_CAPACITY: usize = 256;
const FOCUS_DRIFT_MAX_RETRIES: usize = 2;
const FOCUS_DRIFT_RETRY_DELAY_MS: u64 = 500;
const TICKET_EXPIRY_GRACE_SECS: i64 = 5;

#[cfg(test)]
mod tests;
