//! SandboxCode - Sandbox category error codes. `sandbox.*` prefix.

define_code_enum! {
    /// Sandbox category error codes.
    pub enum SandboxCode {
        /// Sandbox initialization failed.
        InitFailed => "sandbox.init_failed",
        /// Sandbox execution failed.
        ExecutionFailed => "sandbox.execution_failed",
        /// Platform not supported.
        UnsupportedPlatform => "sandbox.unsupported_platform",
        /// Execution timed out.
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
