use serde::Serialize;

/// Bug report identifier for support correlation.
/// Format: `BUG-{12_hex_chars}` (16 chars total).
/// Custom `Deserialize` validates the format on deserialization.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BugId(String);

impl BugId {
    pub fn new(id: String) -> Result<Self, &'static str> {
        if id.starts_with("BUG-")
            && id.len() == 16
            && id[4..].chars().all(|c| c.is_ascii_hexdigit())
        {
            Ok(Self(id))
        } else {
            Err("Bug ID must match format BUG-{12_hex_chars}")
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> serde::Deserialize<'de> for BugId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        BugId::new(s).map_err(serde::de::Error::custom)
    }
}

impl std::fmt::Display for BugId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_bug_id() {
        // Collapse: is_ok() is immediately followed by unwrap() + value assertion.
        let id = BugId::new("BUG-a1b2c3d4e5f6".to_string())
            .expect("BUG- prefix + 12 lowercase hex chars is the valid BugId format");
        assert_eq!(
            id.as_str(),
            "BUG-a1b2c3d4e5f6",
            "as_str() must round-trip the original input"
        );
    }

    #[test]
    fn rejects_short_id() {
        let err = BugId::new("BUG-abc".to_string()).unwrap_err();
        assert!(
            err.contains("BUG-{12_hex_chars}"),
            "short ID must produce format error, got: {err:?}"
        );
    }

    #[test]
    fn rejects_wrong_prefix() {
        let err = BugId::new("ERR-a1b2c3d4e5f6".to_string()).unwrap_err();
        assert!(
            err.contains("BUG-{12_hex_chars}"),
            "wrong prefix must produce format error, got: {err:?}"
        );
    }

    #[test]
    fn rejects_non_hex_chars() {
        let err1 = BugId::new("BUG-ghijklmnopqr".to_string()).unwrap_err();
        assert!(
            err1.contains("BUG-{12_hex_chars}"),
            "non-hex chars must produce format error, got: {err1:?}"
        );
        let err2 = BugId::new("BUG-<script>aaaa".to_string()).unwrap_err();
        assert!(
            err2.contains("BUG-{12_hex_chars}"),
            "script injection must produce format error, got: {err2:?}"
        );
    }

    #[test]
    fn rejects_too_long() {
        let err = BugId::new("BUG-a1b2c3d4e5f6aa".to_string()).unwrap_err();
        assert!(
            err.contains("BUG-{12_hex_chars}"),
            "too-long ID must produce format error, got: {err:?}"
        );
    }

    #[test]
    fn serializes_as_string() {
        let id = BugId::new("BUG-a1b2c3d4e5f6".to_string()).unwrap();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"BUG-a1b2c3d4e5f6\"");
    }

    #[test]
    fn deserializes_valid_id() {
        let id: BugId = serde_json::from_str("\"BUG-a1b2c3d4e5f6\"").unwrap();
        assert_eq!(id.as_str(), "BUG-a1b2c3d4e5f6");
    }

    #[test]
    fn deserialize_rejects_invalid() {
        let err1 = serde_json::from_str::<BugId>("\"INVALID\"").unwrap_err();
        assert!(
            err1.to_string().contains("BUG-{12_hex_chars}"),
            "INVALID BugId deserialization must propagate format message, got: {err1}"
        );

        let err2 = serde_json::from_str::<BugId>("\"BUG-ghijklmnopqr\"").unwrap_err();
        assert!(
            err2.to_string().contains("BUG-{12_hex_chars}"),
            "non-hex BugId deserialization must propagate format message, got: {err2}"
        );
    }

    #[test]
    fn display_impl() {
        let id = BugId::new("BUG-a1b2c3d4e5f6".to_string()).unwrap();
        assert_eq!(format!("{id}"), "BUG-a1b2c3d4e5f6");
    }
}

/// Runtime log snapshot for bug report inclusion.
#[derive(Debug, Clone)]
pub struct RuntimeLogSnapshot {
    pub log_dir: String,
    pub log_file: Option<String>,
    pub line_count: usize,
    pub recent_text: String,
}
