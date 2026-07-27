//! Debug-only storage-pressure fault injection for isolated QC profiles.
//!
//! This module is compiled out when `debug_assertions` are disabled. Runtime
//! gates deliberately require both an isolated `qc-*`/`tc-*` flavor and exact
//! opt-in values so ordinary debug sessions cannot activate the fault by
//! accident.

use maekon_core::error::CoreError;
use maekon_core::error_codes::StorageCode;

const DEBUG_GATE_ENV: &str = "MAEKON_DEBUG_QC_FIXTURE_CLI";
const ISOLATED_GATE_ENV: &str = "MAEKON_TC_ISOLATED_PROFILE";
const FLAVOR_ENV: &str = "MAEKON_APP_FLAVOR";
const STORAGE_PRESSURE_GATE_ENV: &str = "MAEKON_DEBUG_QC_STORAGE_PRESSURE_FIXTURE";
const STORAGE_PRESSURE_MODE_ENV: &str = "MAEKON_QC_STORAGE_PRESSURE_MODE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExportStorageFault {
    LowDisk,
    Locked,
}

impl ExportStorageFault {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::LowDisk => "low-disk-export",
            Self::Locked => "locked-export",
        }
    }

    pub(super) fn into_core_error(self) -> CoreError {
        CoreError::Storage {
            code: StorageCode::Failed,
            message: format!(
                "isolated QC storage-pressure fault injected: {}",
                self.as_str()
            ),
        }
    }
}

pub(super) fn export_fault_from_env() -> Option<ExportStorageFault> {
    export_fault_from_values(
        std::env::var(DEBUG_GATE_ENV).ok().as_deref(),
        std::env::var(ISOLATED_GATE_ENV).ok().as_deref(),
        std::env::var(FLAVOR_ENV).ok().as_deref(),
        std::env::var(STORAGE_PRESSURE_GATE_ENV).ok().as_deref(),
        std::env::var(STORAGE_PRESSURE_MODE_ENV).ok().as_deref(),
    )
}

fn export_fault_from_values(
    debug_gate: Option<&str>,
    isolated_gate: Option<&str>,
    flavor: Option<&str>,
    pressure_gate: Option<&str>,
    mode: Option<&str>,
) -> Option<ExportStorageFault> {
    if debug_gate != Some("1")
        || isolated_gate != Some("1")
        || pressure_gate != Some("1")
        || !flavor.is_some_and(is_isolated_flavor)
    {
        return None;
    }

    match mode {
        Some("low-disk-export") => Some(ExportStorageFault::LowDisk),
        Some("locked-export") => Some(ExportStorageFault::Locked),
        _ => None,
    }
}

fn is_isolated_flavor(flavor: &str) -> bool {
    let trimmed = flavor.trim();
    (trimmed.starts_with("qc-") || trimmed.starts_with("tc-"))
        && trimmed.len() > 3
        && trimmed
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_fault_requires_every_isolation_gate() {
        let valid = [
            Some("1"),
            Some("1"),
            Some("qc-storage-pressure"),
            Some("1"),
            Some("locked-export"),
        ];

        for missing_index in 0..valid.len() {
            let mut values = valid;
            values[missing_index] = None;
            assert_eq!(
                export_fault_from_values(values[0], values[1], values[2], values[3], values[4]),
                None,
                "gate {missing_index} must fail closed"
            );
        }
    }

    #[test]
    fn export_fault_accepts_only_bounded_modes_and_flavors() {
        assert_eq!(
            export_fault_from_values(
                Some("1"),
                Some("1"),
                Some("qc-storage-pressure"),
                Some("1"),
                Some("low-disk-export"),
            ),
            Some(ExportStorageFault::LowDisk)
        );
        assert_eq!(
            export_fault_from_values(
                Some("1"),
                Some("1"),
                Some("tc-storage_pressure"),
                Some("1"),
                Some("locked-export"),
            ),
            Some(ExportStorageFault::Locked)
        );
        assert_eq!(
            export_fault_from_values(
                Some("1"),
                Some("1"),
                Some("production"),
                Some("1"),
                Some("locked-export"),
            ),
            None
        );
        assert_eq!(
            export_fault_from_values(
                Some("1"),
                Some("1"),
                Some("qc-storage-pressure"),
                Some("1"),
                Some("disk-full-host"),
            ),
            None
        );
    }

    #[test]
    fn export_fault_uses_typed_storage_error_without_host_details() {
        let error = ExportStorageFault::Locked.into_core_error();
        assert_eq!(error.code(), "storage.failed");
        assert!(error.to_string().contains("locked-export"));
        assert!(!error.to_string().contains("Users"));
    }
}
