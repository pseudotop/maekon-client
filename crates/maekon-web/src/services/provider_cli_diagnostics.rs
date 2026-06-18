//! Re-export shim for the provider CLI diagnostics port.
//!
//! The canonical trait + future alias now live in
//! `maekon-core::ports::provider_cli_diagnostics` (ADR-001 §7 — inbound ports
//! originate in core). This module is kept only as a thin re-export shim for
//! source compatibility, mirroring the `WebStorage` relocation.

pub use maekon_core::ports::provider_cli_diagnostics::*;
