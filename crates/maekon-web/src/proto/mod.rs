//! Generated proto bindings for the D13 Dashboard gRPC service.
//!
//! Regenerate via `./scripts/regenerate-dashboard-protos.sh` — the output file
//! (`oneshim.dashboard.v1.rs`) is committed so production builds don't require
//! `protoc` to be installed.
//!
//! The `#[cfg(feature = "grpc-dashboard")]` gate lives on `pub mod proto;` in
//! `lib.rs` (clippy `duplicated_attributes` trips on an inner-attr dupe).

#![allow(clippy::all, missing_docs, clippy::derive_partial_eq_without_eq)]

pub mod dashboard {
    pub mod v1 {
        include!("generated/oneshim.dashboard.v1.rs");
    }
}
