//! SandboxCode — Sandbox 카테고리 에러 코드. `sandbox.*` 접두사.

define_code_enum! {
    /// Sandbox 카테고리 에러 코드.
    pub enum SandboxCode {
        /// 샌드박스 초기화 실패.
        InitFailed => "sandbox.init_failed",
        /// 샌드박스 실행 실패.
        ExecutionFailed => "sandbox.execution_failed",
        /// 플랫폼 미지원.
        UnsupportedPlatform => "sandbox.unsupported_platform",
        /// 실행 시간 초과.
        Timeout => "sandbox.timeout",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn as_str_round_trip_unique() {
        let codes: Vec<&str> = SandboxCode::all().iter().map(|c| c.as_str()).collect();
        let unique: HashSet<_> = codes.iter().collect();
        assert_eq!(codes.len(), unique.len());
    }

    #[test]
    fn naming_convention() {
        for c in SandboxCode::all() {
            let s = c.as_str();
            assert!(s
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch == '.' || ch == '_'));
            assert!(s.contains('.'));
        }
    }

    #[test]
    fn display_matches_as_str() {
        for c in SandboxCode::all() {
            assert_eq!(format!("{c}"), c.as_str());
        }
    }
}
