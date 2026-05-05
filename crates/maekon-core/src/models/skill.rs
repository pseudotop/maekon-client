//! Skill definition model — loaded from SKILL.md files.

use serde::{Deserialize, Serialize};

/// Metadata extracted from YAML frontmatter of a SKILL.md file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMeta {
    pub name: String,
    pub description: String,
}

/// A fully loaded skill with metadata and body content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub meta: SkillMeta,
    /// Full markdown body (everything after the YAML frontmatter).
    pub body: String,
    /// Relative path to the source file (for diagnostics).
    pub source_path: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_meta_serde_roundtrip() {
        let meta = SkillMeta {
            name: "ui-automation".to_string(),
            description: "Automate UI actions based on intent".to_string(),
        };
        let json = serde_json::to_string(&meta).unwrap();
        let deser: SkillMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.name, "ui-automation");
    }

    #[test]
    fn skill_serde_roundtrip() {
        let skill = Skill {
            meta: SkillMeta {
                name: "test-skill".to_string(),
                description: "A test skill".to_string(),
            },
            body: "# Instructions\nDo the thing.".to_string(),
            source_path: ".agents/skills/test.md".to_string(),
        };
        let json = serde_json::to_string(&skill).unwrap();
        let deser: Skill = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.meta.name, "test-skill");
        assert!(deser.body.contains("Instructions"));
    }
}
