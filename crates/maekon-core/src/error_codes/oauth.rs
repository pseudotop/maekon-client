//! OAuthCode — OAuth category error codes. `oauth.*` prefix.

define_code_enum! {
    /// OAuth category error codes.
    pub enum OAuthCode {
        /// OAuth authentication failed (initial acquisition).
        Failed => "oauth.failed",
        /// OAuth token refresh failed.
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
