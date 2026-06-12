pub(super) mod autostart_helper;
mod coaching_helper;
pub(super) mod detection_helper;
mod events;
mod focus_auto_helper;
pub(crate) mod health;
mod helpers;
mod intelligence;
mod intervals;
mod monitor;
mod monitor_phases;
mod network;
// E20-24 (#4816): houses both the server-only SSE loop and the local maintenance
// loop (deferred resurface). Gated on `local-suggestions` (default-on); the SSE
// fn inside stays `#[cfg(feature = "server")]`.
#[cfg(feature = "local-suggestions")]
pub(crate) mod suggestions;
mod sync;
mod system;
pub(super) mod tracking_schedule_helper;
mod vision_helper;

// ── Public re-exports ────────────────────────────────────────────────
pub(crate) use helpers::record_to_segment_summary;
