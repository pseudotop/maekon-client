//! AudioCode — Audio 카테고리 에러 코드. `audio.*` 접두사.

define_code_enum! {
    /// Audio 카테고리 에러 코드.
    pub enum AudioCode {
        /// 오디오 캡처 실패.
        CaptureFailed => "audio.capture_failed",
        /// 음성→텍스트 변환 실패.
        SttFailed => "audio.stt_failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn as_str_round_trip_unique() {
        let codes: Vec<&str> = AudioCode::all().iter().map(|c| c.as_str()).collect();
        let unique: HashSet<_> = codes.iter().collect();
        assert_eq!(codes.len(), unique.len());
    }

    #[test]
    fn naming_convention() {
        for c in AudioCode::all() {
            let s = c.as_str();
            assert!(s
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch == '.' || ch == '_'));
            assert!(s.contains('.'));
        }
    }

    #[test]
    fn display_matches_as_str() {
        for c in AudioCode::all() {
            assert_eq!(format!("{c}"), c.as_str());
        }
    }
}
