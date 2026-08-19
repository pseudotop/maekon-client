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
// P2 PR-A nursery-hardening. (Enforced workspace-wide via
// `[workspace.lints.clippy]`, #7719.)
#![cfg_attr(test, allow(clippy::significant_drop_tightening))]

//! # maekon-suggestion

pub mod error;
pub use error::SuggestionError;

pub mod feedback;
pub mod history;
pub mod presenter;
pub mod queue;
pub mod receiver;
pub mod scorer;

pub mod deferred;
pub mod feedback_retry;
