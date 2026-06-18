//! AudioCode — Audio category error codes. `audio.*` prefix.

define_code_enum! {
    /// Audio category error codes.
    pub enum AudioCode {
        /// Audio capture failed.
        CaptureFailed => "audio.capture_failed",
        /// Speech-to-text conversion failed.
        SttFailed => "audio.stt_failed",
        /// Downloaded file integrity check failed (SHA-256 mismatch). F-RC-C22-03.
        IntegrityCheckFailed => "audio.integrity_check_failed",
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
