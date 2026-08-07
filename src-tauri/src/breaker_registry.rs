//! #5032: crate-local alias for the workspace-wide circuit-breaker registry.
//!
//! The single shared `CircuitBreakerRegistry` (D7 / #4812 / E20-20) is threaded
//! as `Arc<…>` through many composition-root function signatures and builder
//! struct fields that themselves do NOT touch the network (the registry just
//! flows through them as an opaque handle; the only code that actually
//! *consumes* it by calling a network constructor — `AnalysisClient`,
//! `RemoteOcrProvider`, `RemoteLlmProvider`, `RemoteEmbeddingProvider`, … — is
//! gated behind the `analysis`/`server` features).
//!
//! History (ADR-034 P2): this module used to carry a `#[cfg]`-branched
//! stand-in — the real type under `analysis`, an inert stub otherwise —
//! because the registry lived in `maekon-network`, which is an OPTIONAL
//! dependency here, and referencing `maekon_network::CircuitBreakerRegistry`
//! unconditionally in the pass-through signatures broke
//! `--no-default-features` (E0433, #7743 ctd-W3 A2b). The registry now lives
//! in `maekon-http-core`, an unconditional dependency (every third-party crate
//! it uses — reqwest, parking_lot, rand, url — is already unconditional in
//! this manifest), so the real type exists in every feature-matrix cell and
//! the stub machinery is gone. Under `--no-default-features` the registry is
//! constructed but never consumed, exactly as the stub was.
//!
//! The alias itself is kept (rather than repointing ~a dozen pass-through
//! signatures) so the import graph still names ONE place where the registry
//! type comes from.

pub(crate) use maekon_http_core::circuit_breaker::CircuitBreakerRegistry;
