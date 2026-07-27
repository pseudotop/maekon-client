//! Pure use-case orchestrators (compose ports only, no implementation dependencies).
//!
//! The services here merely coordinate domain ports and do not depend on
//! network or storage implementations. This is where the #8587 dependency gate
//! — that read-only context collection must not depend on the write transport —
//! is satisfied structurally.

pub mod context_sync;
