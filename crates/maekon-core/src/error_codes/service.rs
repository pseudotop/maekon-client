//! ServiceCode — Service availability category error codes. `service.*` prefix.

define_code_enum! {
    /// Service category error codes.
    pub enum ServiceCode {
        /// Local circuit breaker is open, so fast-fail state (distinct from server-side failure).
        CircuitOpen => "service.circuit_open",
        /// Service temporarily unavailable.
        Unavailable => "service.unavailable",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn as_str_round_trip_unique() {
        let codes: Vec<&str> = ServiceCode::all().iter().map(|c| c.as_str()).collect();
        let unique: HashSet<_> = codes.iter().collect();
        assert_eq!(codes.len(), unique.len());
    }

    #[test]
    fn naming_convention() {
        for c in ServiceCode::all() {
            let s = c.as_str();
            assert!(s
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch == '.' || ch == '_'));
            assert!(s.contains('.'));
        }
    }

    #[test]
    fn display_matches_as_str() {
        for c in ServiceCode::all() {
            assert_eq!(format!("{c}"), c.as_str());
        }
    }
}
