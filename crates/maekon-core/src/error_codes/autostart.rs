//! AutostartCode — Autostart category error codes. `autostart.*` prefix.

define_code_enum! {
    /// Autostart category error codes.
    pub enum AutostartCode {
        /// Failed to increment the autostart counter.
        CounterIncrementFailed => "autostart.counter_increment_failed",
        /// Failed to disable autostart.
        DisableFailed => "autostart.disable_failed",
        /// Failed to enable autostart.
        EnableFailed => "autostart.enable_failed",
        /// Failed to emit the autostart Tauri event.
        EventEmitFailed => "autostart.event_emit_failed",
        /// Failed to query autostart status.
        QueryFailed => "autostart.query_failed",
        /// systemd notify call skipped (e.g. NOTIFY_SOCKET absent).
        SdNotifySkipped => "autostart.sd_notify_skipped",
        /// systemd service file migration completed.
        ServiceMigrated => "autostart.service_migrated",
        /// systemd service file migration failed (write/io error).
        ServiceMigrationFailed => "autostart.service_migration_failed",
        /// systemd service file migration skipped (presumed user-modified).
        ServiceMigrationSkipped => "autostart.service_migration_skipped",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn as_str_round_trip_unique() {
        let codes: Vec<&str> = AutostartCode::all().iter().map(|c| c.as_str()).collect();
        let unique: HashSet<_> = codes.iter().collect();
        assert_eq!(codes.len(), unique.len());
    }

    #[test]
    fn naming_convention() {
        for c in AutostartCode::all() {
            let s = c.as_str();
            assert!(s
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch == '.' || ch == '_'));
            assert!(s.contains('.'));
            assert!(s.starts_with("autostart."));
        }
    }

    #[test]
    fn display_matches_as_str() {
        for c in AutostartCode::all() {
            assert_eq!(format!("{c}"), c.as_str());
        }
    }
}
