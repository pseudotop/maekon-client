//! NetworkCode — Network category error codes. `network.*` prefix.

define_code_enum! {
    /// Network category error codes.
    pub enum NetworkCode {
        /// Request timeout exceeded.
        Timeout => "network.timeout",
        /// Server rate limit reached (429).
        RateLimit => "network.rate_limit",
        /// Not yet subdivided.
        Generic => "network.generic",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn as_str_round_trip_unique() {
        let codes: Vec<&str> = NetworkCode::all().iter().map(|c| c.as_str()).collect();
        let unique: HashSet<_> = codes.iter().collect();
        assert_eq!(codes.len(), unique.len());
    }

    #[test]
    fn naming_convention() {
        for c in NetworkCode::all() {
            let s = c.as_str();
            assert!(s
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch == '.' || ch == '_'));
            assert!(s.contains('.'));
        }
    }

    #[test]
    fn display_matches_as_str() {
        for c in NetworkCode::all() {
            assert_eq!(format!("{c}"), c.as_str());
        }
    }
}
