#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
// P2 PR-C: `missing_const_for_fn` accepted crate-wide. See
// docs/reviews/2026-04-21-p2-missing-const-for-fn-decision.md.
#![allow(clippy::missing_const_for_fn)]
// P2 remaining-nursery-lints: see decision doc.
#![allow(
    clippy::use_self,
    clippy::option_if_let_else,
    clippy::redundant_pub_crate
)]
// P2 PR-A (B3): `significant_drop_tightening` is accepted crate-wide.
// Rationale: 27 flagged sites across 7 files — controller/gate,
// controller/intent, controller/preset, controller/port_impl,
// gui_interaction/service. All are tokio::sync or std::sync locks held
// across fast state transitions where the nursery lint's suggested rewrite
// (`merge-with-single-usage`) either produces invalid code or trades one
// atomicity guarantee for another. Clippy false-positive rate here is high.
// See docs/reviews/2026-04-21-p2-significant-drop-tightening-spec.md §Category B.
#![allow(clippy::significant_drop_tightening)]

//! # oneshim-automation

pub mod error;
pub use error::AutomationError;

pub mod action_dispatcher;
pub mod audit;
pub mod controller;
pub mod gui_interaction;
pub mod input_driver;
pub mod intent_planner;
pub mod intent_resolver;
pub mod local_llm;
pub mod overlay;
pub mod policy;
pub mod presets;
pub mod resolver;
pub mod sandbox;
