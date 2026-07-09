use serde::Serialize;
use std::sync::OnceLock;

const STATUS_REGISTERED: &str = "registered";
const STATUS_FAILED: &str = "failed";
const COLLISION_NONE: &str = "none";
const COLLISION_FALLBACK: &str = "fallback";
const COLLISION_UNHANDLED: &str = "unhandled";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ShortcutRegistrationRecord {
    pub(crate) id: String,
    pub(crate) purpose: String,
    pub(crate) primary_accelerator: String,
    pub(crate) primary_status: String,
    pub(crate) fallback_accelerator: Option<String>,
    pub(crate) fallback_status: Option<String>,
    pub(crate) collision_handled: String,
    pub(crate) user_notice_required: bool,
    pub(crate) error_message: Option<String>,
}

static SHORTCUT_REGISTRY: OnceLock<parking_lot::Mutex<Vec<ShortcutRegistrationRecord>>> =
    OnceLock::new();

fn registry() -> &'static parking_lot::Mutex<Vec<ShortcutRegistrationRecord>> {
    SHORTCUT_REGISTRY.get_or_init(|| parking_lot::Mutex::new(Vec::new()))
}

pub(crate) fn reset() {
    registry().lock().clear();
}

// Only consumed by this module's own test assertions today (no production
// caller since the diagnostics IPC surface was never wired up); scoped to
// `cfg(test)` so it doesn't trip the production-build dead_code lint.
#[cfg(test)]
pub(crate) fn records() -> Vec<ShortcutRegistrationRecord> {
    registry().lock().clone()
}

pub(crate) fn record_registered(id: &str, purpose: &str, primary_accelerator: &str) {
    upsert(ShortcutRegistrationRecord {
        id: id.to_string(),
        purpose: purpose.to_string(),
        primary_accelerator: primary_accelerator.to_string(),
        primary_status: STATUS_REGISTERED.to_string(),
        fallback_accelerator: None,
        fallback_status: None,
        collision_handled: COLLISION_NONE.to_string(),
        user_notice_required: false,
        error_message: None,
    });
}

pub(crate) fn record_fallback_registered(
    id: &str,
    purpose: &str,
    primary_accelerator: &str,
    fallback_accelerator: &str,
    error_message: String,
) {
    upsert(ShortcutRegistrationRecord {
        id: id.to_string(),
        purpose: purpose.to_string(),
        primary_accelerator: primary_accelerator.to_string(),
        primary_status: STATUS_FAILED.to_string(),
        fallback_accelerator: Some(fallback_accelerator.to_string()),
        fallback_status: Some(STATUS_REGISTERED.to_string()),
        collision_handled: COLLISION_FALLBACK.to_string(),
        user_notice_required: true,
        error_message: Some(error_message),
    });
}

pub(crate) fn record_fallback_failed(
    id: &str,
    purpose: &str,
    primary_accelerator: &str,
    fallback_accelerator: &str,
    primary_error: String,
    fallback_error: String,
) {
    upsert(ShortcutRegistrationRecord {
        id: id.to_string(),
        purpose: purpose.to_string(),
        primary_accelerator: primary_accelerator.to_string(),
        primary_status: STATUS_FAILED.to_string(),
        fallback_accelerator: Some(fallback_accelerator.to_string()),
        fallback_status: Some(STATUS_FAILED.to_string()),
        collision_handled: COLLISION_UNHANDLED.to_string(),
        user_notice_required: true,
        error_message: Some(format!(
            "{primary_error}; fallback failed: {fallback_error}"
        )),
    });
}

fn upsert(record: ShortcutRegistrationRecord) {
    let mut records = registry().lock();
    if let Some(existing) = records.iter_mut().find(|item| item.id == record.id) {
        *existing = record;
    } else {
        records.push(record);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_record_surfaces_collision_and_notice_requirement() {
        reset();

        record_fallback_registered(
            "overlay-toggle",
            "overlay toggle",
            "CmdOrCtrl+Shift+O",
            "CmdOrCtrl+Alt+O",
            "forced shortcut collision".to_string(),
        );

        let records = records();
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.primary_status, STATUS_FAILED);
        assert_eq!(record.fallback_status.as_deref(), Some(STATUS_REGISTERED));
        assert_eq!(record.collision_handled, COLLISION_FALLBACK);
        assert!(record.user_notice_required);
        assert_eq!(
            record.fallback_accelerator.as_deref(),
            Some("CmdOrCtrl+Alt+O")
        );
    }
}
