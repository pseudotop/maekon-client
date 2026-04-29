//! ProviderCode — AI provider 카테고리 에러 코드. `provider.*` 접두사.

define_code_enum! {
    /// Provider 카테고리 에러 코드.
    pub enum ProviderCode {
        /// OCR 요청 실패.
        OcrFailed => "provider.ocr_failed",
        /// Analysis 요청 실패.
        AnalysisFailed => "provider.analysis_failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn as_str_round_trip_unique() {
        let codes: Vec<&str> = ProviderCode::all().iter().map(|c| c.as_str()).collect();
        let unique: HashSet<_> = codes.iter().collect();
        assert_eq!(codes.len(), unique.len());
    }

    #[test]
    fn naming_convention() {
        for c in ProviderCode::all() {
            let s = c.as_str();
            assert!(s
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch == '.' || ch == '_'));
            assert!(s.contains('.'));
        }
    }

    #[test]
    fn display_matches_as_str() {
        for c in ProviderCode::all() {
            assert_eq!(format!("{c}"), c.as_str());
        }
    }
}
