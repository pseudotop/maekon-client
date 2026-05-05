//! AutostartCode — Autostart 카테고리 에러 코드. `autostart.*` 접두사.

define_code_enum! {
    /// Autostart 카테고리 에러 코드.
    pub enum AutostartCode {
        /// 자동 시작 카운터 증가 실패.
        CounterIncrementFailed => "autostart.counter_increment_failed",
        /// 자동 시작 비활성화 실패.
        DisableFailed => "autostart.disable_failed",
        /// 자동 시작 활성화 실패.
        EnableFailed => "autostart.enable_failed",
        /// autostart Tauri 이벤트 emit 실패.
        EventEmitFailed => "autostart.event_emit_failed",
        /// 자동 시작 상태 조회 실패.
        QueryFailed => "autostart.query_failed",
        /// systemd notify 호출 스킵 (NOTIFY_SOCKET 없음 등).
        SdNotifySkipped => "autostart.sd_notify_skipped",
        /// systemd 서비스 파일 마이그레이션 완료.
        ServiceMigrated => "autostart.service_migrated",
        /// systemd 서비스 파일 마이그레이션 실패 (write/io 에러).
        ServiceMigrationFailed => "autostart.service_migration_failed",
        /// systemd 서비스 파일 마이그레이션 스킵 (사용자 수정 추정).
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
