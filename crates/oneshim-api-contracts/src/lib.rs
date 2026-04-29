//! # oneshim-api-contracts
//!
//! Shared request/response type contracts between client crates.
//! Ensures API contract consistency across the workspace.

// P2 nursery-hardening (PR-B): derive Eq alongside PartialEq when possible.
#![deny(clippy::derive_partial_eq_without_eq)]
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

pub mod ai_providers;
pub mod annotations;
pub mod audit_export;
pub mod automation;
pub mod automation_gui;
pub mod backup;
pub mod bug_report;
pub mod coaching;
pub mod common;
pub mod dashboard;
pub mod data;
pub mod digests;
pub mod error;
pub mod events;
pub mod export;
pub mod external_grpc;
pub mod focus;
pub mod frames;
pub mod idle;
pub mod integration;
pub mod metrics;
pub mod onboarding;
pub mod playbooks;
pub mod pomodoro;
pub mod processes;
pub mod provider_specs;
pub mod recalibration;
pub mod reports;
pub mod search;
pub mod sessions;
pub mod settings;
pub mod stats;
pub mod stream;
pub mod suggestions;
pub mod support;
pub mod tags;
pub mod timeline;
pub mod tracking_schedule;
pub mod update;
