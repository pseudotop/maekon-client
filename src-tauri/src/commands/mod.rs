pub(crate) mod ai_session;
pub(crate) mod analysis;
pub(crate) mod audio;
pub(crate) mod automation;
pub(crate) mod autostart;
pub(crate) mod bug_report;
pub(crate) mod capture;
pub(crate) mod capture_status;
pub(crate) mod coaching;
pub(crate) mod dashboard;
pub(crate) mod detection;
pub(crate) mod error_report;
pub(crate) mod focus;
pub(crate) mod generate_external_cert;
pub(crate) mod integration;
pub(crate) mod onboarding;
pub(crate) mod permissions;
pub(crate) mod settings;
pub(crate) mod suggestion_parser;
pub(crate) mod suggestions;
pub(crate) mod sync;
pub(crate) mod system;
pub(crate) mod tracking_schedule;

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
