pub(super) mod autostart_helper;
mod coaching_helper;
pub(super) mod detection_helper;
mod events;
mod focus_auto_helper;
pub(crate) mod health;
mod helpers;
mod intelligence;
mod monitor;
mod network;
#[cfg(feature = "server")]
pub(crate) mod suggestions;
mod sync;
mod system;
pub(super) mod tracking_schedule_helper;
mod vision_helper;

// ── Public re-exports ────────────────────────────────────────────────
pub(crate) use helpers::record_to_segment_summary;
