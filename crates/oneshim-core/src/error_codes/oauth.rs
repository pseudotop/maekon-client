//! OAuthCode — OAuth 카테고리 에러 코드. `oauth.*` 접두사.

define_code_enum! {
    /// OAuth 카테고리 에러 코드.
    pub enum OAuthCode {
        /// OAuth 인증 실패 (초기 획득).
        Failed => "oauth.failed",
        /// OAuth 토큰 리프레시 실패.
        RefreshFailed => "oauth.refresh_failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn as_str_round_trip_unique() {
        let codes: Vec<&str> = OAuthCode::all().iter().map(|c| c.as_str()).collect();
        let unique: HashSet<_> = codes.iter().collect();
        assert_eq!(codes.len(), unique.len());
    }

    #[test]
    fn naming_convention() {
        for c in OAuthCode::all() {
            let s = c.as_str();
            assert!(s
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch == '.' || ch == '_'));
            assert!(s.contains('.'));
        }
    }

    #[test]
    fn display_matches_as_str() {
        for c in OAuthCode::all() {
            assert_eq!(format!("{c}"), c.as_str());
        }
    }
}
