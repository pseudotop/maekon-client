//! ConfigCode — Config 카테고리 에러 코드.
//!
//! 네이밍: `config.*` 접두사가 기본. 신규 코드 추가 시 ADR-019 §2 컨벤션 준수.
//!
//! 예외: `UnsupportedProviderBedrock`은 의도적으로 `provider.bedrock.unsupported`
//! 와이어 코드를 사용 — 관측성 대시보드가 provider-unsupported를 단일 `provider.*`
//! 네임스페이스로 그룹핑할 수 있게 함. 이 exception은 ADR-019 §3에서 명시.

// `define_code_enum!` is re-exported at crate root via `#[macro_export]` in
// `error_codes/macros.rs`; no explicit `use` needed within the same crate.

define_code_enum! {
    /// Config 카테고리 에러 코드.
    pub enum ConfigCode {
        /// 설정 파일 파싱 실패 또는 스키마 불일치.
        Invalid => "config.invalid",
        /// 필수 설정 필드 누락.
        Missing => "config.missing",
        /// 설정 값이 허용 범위 밖.
        OutOfRange => "config.out_of_range",
        /// AWS Bedrock 의도적 미지원 (ADR-019 §5 재도입 체크리스트 참조).
        UnsupportedProviderBedrock => "provider.bedrock.unsupported",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn as_str_round_trip_unique() {
        let codes: Vec<&str> = ConfigCode::all().iter().map(|c| c.as_str()).collect();
        let unique: HashSet<_> = codes.iter().collect();
        assert_eq!(codes.len(), unique.len(), "duplicate codes: {codes:?}");
    }

    #[test]
    fn naming_convention() {
        for c in ConfigCode::all() {
            let s = c.as_str();
            assert!(
                s.chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch == '.' || ch == '_'),
                "non-conforming character in {s:?}"
            );
            assert!(s.contains('.'), "missing dot in {s:?}");
        }
    }

    #[test]
    fn display_matches_as_str() {
        for c in ConfigCode::all() {
            assert_eq!(format!("{c}"), c.as_str());
        }
    }
}
