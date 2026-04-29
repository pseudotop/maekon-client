//! Consumer Contract proto types (oneshim.client.v1).

/// All Consumer Contract types live in a single package `oneshim.client.v1`.
/// Services: ClientAuth, ClientSession, ClientContext, ClientSuggestion, ClientHealth.
#[cfg(feature = "grpc")]
pub mod client_v1 {
    #![allow(clippy::all)]
    #![allow(warnings)]
    #![allow(clippy::derive_partial_eq_without_eq)]
    include!("generated/oneshim.client.v1.rs");
}
