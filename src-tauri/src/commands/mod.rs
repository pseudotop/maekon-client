pub(crate) mod ai_session;
pub(crate) mod analysis;
pub(crate) mod audio;
pub(crate) mod audit;
// #9492: `pub` (like `consent` / `extension` / `task` below) so the
// `auth_status_mapping` integration test can drive `auth_status_inner` against a
// live login round trip. Every item inside the module was already `pub`; only
// the module gate was crate-private.
pub mod assignment_email_draft;
pub mod auth;
pub(crate) mod automation;
pub(crate) mod autostart;
pub(crate) mod bug_report;
pub(crate) mod build_info;
pub(crate) mod capture;
pub(crate) mod capture_status;
pub(crate) mod coaching;
pub mod consent;
// #9625: `pub` so the context-home slot semantics can be exercised from
// integration tests — the "unwired means unavailable, not empty" distinction is
// the acceptance surface.
pub mod context_home;
pub(crate) mod detection;
pub(crate) mod error_report;
pub mod extension;
pub(crate) mod focus;
pub(crate) mod generate_external_cert;
pub(crate) mod integration;
pub(crate) mod notification;
pub(crate) mod onboarding;
// #9707: `pub` so the OS-handoff policy can be exercised from integration tests
// without a desktop session — `validate` is the whole acceptance surface.
pub mod os_handoff;
pub(crate) mod permissions;
pub(crate) mod privacy_audit;
pub(crate) mod qc_upload_spool;
pub(crate) mod reauth;
pub(crate) mod settings;
pub(crate) mod shortcuts;
pub(crate) mod suggestion_parser;
pub(crate) mod suggestions;
pub(crate) mod sync;
pub(crate) mod system;
pub mod task;
pub(crate) mod tray;
pub(crate) mod vault;

/// Recursively merge `patch` into `base`.
/// Objects are merged key-by-key; all other values are replaced.
fn deep_merge(base: &mut serde_json::Value, patch: serde_json::Value) {
    match (base.as_object_mut(), patch) {
        (Some(base_obj), serde_json::Value::Object(patch_obj)) => {
            for (k, v) in patch_obj {
                deep_merge(base_obj.entry(k).or_insert(serde_json::Value::Null), v);
            }
        }
        (_, patch) => *base = patch,
    }
}
