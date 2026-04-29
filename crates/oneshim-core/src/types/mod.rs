//! Domain primitive types shared across the workspace.
//!
//! Currently contains:
//! - `TimeWindow` — closed-bounded absolute time window for SQL/REST/domain
//!   model time-range needs. See `time_window.rs` for full documentation.

pub mod time_window;

pub use time_window::{TimeWindow, TimeWindowError};
