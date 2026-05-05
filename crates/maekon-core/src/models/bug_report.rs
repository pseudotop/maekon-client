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
        let id = BugId::new("BUG-a1b2c3d4e5f6".to_string());
        assert!(id.is_ok());
        assert_eq!(id.unwrap().as_str(), "BUG-a1b2c3d4e5f6");
    }

    #[test]
    fn rejects_short_id() {
        assert!(BugId::new("BUG-abc".to_string()).is_err());
    }

    #[test]
    fn rejects_wrong_prefix() {
        assert!(BugId::new("ERR-a1b2c3d4e5f6".to_string()).is_err());
    }

    #[test]
    fn rejects_non_hex_chars() {
        assert!(BugId::new("BUG-ghijklmnopqr".to_string()).is_err());
        assert!(BugId::new("BUG-<script>aaaa".to_string()).is_err());
    }

    #[test]
    fn rejects_too_long() {
        assert!(BugId::new("BUG-a1b2c3d4e5f6aa".to_string()).is_err());
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
        let result: Result<BugId, _> = serde_json::from_str("\"INVALID\"");
        assert!(result.is_err());

        let result: Result<BugId, _> = serde_json::from_str("\"BUG-ghijklmnopqr\"");
        assert!(result.is_err());
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
