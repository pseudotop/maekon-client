//! GUI automation domain models — runtime session state, readiness/capability
//! reporting, real-input execution contracts, session acceptance matrix,
//! permission/evidence policy, and benchmark harness/report types.
//!
//! Split from a single 2,162-line `gui.rs` that mixed 5 concerns (runtime
//! session DTOs, readiness, execution-contract, acceptance-spec,
//! permission-policy, benchmark catalogs) plus 4 separate
//! `validate_contract_coverage` impls, per issue #7721 (F4). Pure move — no
//! signature/behavior change. Public API is unchanged: every type is
//! re-exported here at the original `models::gui::*` path.

mod acceptance;
mod benchmark;
mod execution_contract;
mod permission_policy;
mod readiness;
mod session;

pub use acceptance::*;
pub use benchmark::*;
pub use execution_contract::*;
pub use permission_policy::*;
pub use readiness::*;
pub use session::*;
