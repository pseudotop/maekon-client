// Cast safety: SQLite row IDs, byte counts, durations — precision loss acceptable.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
// P2 PR-C: `missing_const_for_fn` accepted crate-wide.
// Rationale: const-viral cascade + nursery false-positive rate outweigh the value.
#![allow(clippy::missing_const_for_fn)]
// P2 remaining-nursery-lints: stylistic/cosmetic nursery lints accepted crate-wide.
#![allow(
    clippy::use_self,
    clippy::option_if_let_else,
    clippy::redundant_pub_crate
)]
// P2 PR-A (B4): `significant_drop_tightening` is accepted crate-wide.
// Rationale: 126 flagged sites across 28 files — sqlite/* implementations,
// keychain, integration_state_store, regime_manager_state_store,
// sync_extractor/merger. The canonical pattern throughout this crate is
// "acquire SQLite connection lock, prepare + bind + execute, drop" —
// clippy flags nearly every such function. Tightening is either
// impossible (the guard must outlive the prepared statement's lifetime)
// or undesirable (mutate-then-save sequences rely on the lock as the
// atomicity guard; see FileIntegrationStateInner). The nursery lint's
// signal-to-noise ratio on this crate is too low to enforce.
// Rationale is embedded here so public source remains self-contained.
#![allow(clippy::significant_drop_tightening)]

//! # maekon-storage

pub mod error;
pub use error::StorageError;

pub mod audit_chain;
pub mod encryption;
pub mod env_secret_store;
pub mod file_secret_store;
pub mod file_transport;
pub mod frame_storage;
pub mod integration_state_store;
pub mod keychain;
pub mod lan_pin_store_adapter;
pub mod migration;
pub mod process_env_projection;
pub mod process_lock;
pub mod regime_manager_state_store;
pub mod sqlite;
pub mod sync_extractor;
pub mod sync_merger;
pub mod temp_file_projection;
