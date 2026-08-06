// P2 PR-A (B2), inherited with the modules ADR-034 moved here:
// `significant_drop_tightening` is accepted crate-wide. The flagged sites are
// parking_lot guards held across in-memory state transitions (circuit_breaker)
// and mockito servers in tests — the nursery lint's "tighten via single-usage"
// heuristic cannot rewrite them (produces invalid Rust, confirmed on similar
// sites in PR #468), so its false-positive rate outweighs its value here.
#![allow(clippy::significant_drop_tightening)]

//! Outbound HTTP substrate shared by every adapter that talks to a remote host
//! (ADR-034).
//!
//! # Why this crate exists
//!
//! `CLAUDE.md` forbids one adapter crate from depending on another. Third-party
//! integrations therefore cannot reach into `maekon-network` for its hardened
//! client and retry primitives, and duplicating them is worse than either
//! option: `read_text_capped` bounds response-body reads and
//! `hardened_client_builder` fixes redirect policy, so a second copy is a
//! security control that drifts.
//!
//! Putting these below the adapters lets `maekon-network` and a future
//! `maekon-integration` share one copy while depending only on
//! `maekon-core` and this crate — never on each other.
//!
//! # What belongs here
//!
//! Outbound HTTP *mechanics* with no domain knowledge and no transport
//! opinions: how to build a safe client, how long to wait after a failure, when
//! to stop calling a sick endpoint. Anything that knows about sessions,
//! suggestions, events, or a specific provider's wire format belongs in an
//! adapter, not here.

pub mod circuit_breaker;
pub mod outbound;
pub mod provider_error_body;
pub mod resilience;
