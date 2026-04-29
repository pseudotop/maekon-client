//! File-based SKILL.md loader — discovers and parses skill definitions.
//!
//! Scans `.agents/skills/` directories for Markdown files with YAML
//! frontmatter containing `name` and `description` fields.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use oneshim_core::error::CoreError;
use oneshim_core::models::skill::{Skill, SkillMeta};
use oneshim_core::ports::skill_loader::SkillLoader;
use tracing::{debug, warn};

/// Default directory name for skill definitions.
const SKILLS_DIR: &str = ".agents/skills";

/// File-based skill loader that reads SKILL.md files from disk.
pub struct FileSkillLoader {
    /// Root directories to search for `.agents/skills/`.
    search_roots: Vec<PathBuf>,
    /// Cached skills keyed by name.
    cache: RwLock<HashMap<String, Skill>>,
}

impl FileSkillLoader {
    /// Create a new loader and immediately scan for skills.
    pub fn new(search_roots: Vec<PathBuf>) -> Self {
        let loader = Self {
            search_roots,
            cache: RwLock::new(HashMap::new()),
        };
        if let Err(e) = loader.reload() {
            warn!("Initial skill scan failed: {e}");
        }
        loader
    }

    /// Parse YAML frontmatter and body from a Markdown file's content.
    fn parse_frontmatter(content: &str) -> Option<(SkillMeta, String)> {
        let trimmed = content.trim_start();
        if !trimmed.starts_with("---") {
            return None;
        }

        // Find the closing `---` delimiter (skip the opening one).
        let after_open = &trimmed[3..];
        let close_idx = after_open.find("\n---")?;
        let yaml_block = &after_open[..close_idx];
        let body_start = 3 + close_idx + 4; // "---" + "\n---"
        let body = trimmed[body_start..].trim_start_matches('\n').to_string();

        // Minimal YAML parsing — only need `name:` and `description:`.
        let mut name = None;
        let mut description = None;
        for line in yaml_block.lines() {
            let line = line.trim();
            if let Some(val) = line.strip_prefix("name:") {
                name = Some(val.trim().trim_matches('"').trim_matches('\'').to_string());
            } else if let Some(val) = line.strip_prefix("description:") {
                description = Some(val.trim().trim_matches('"').trim_matches('\'').to_string());
            }
        }

        Some((
            SkillMeta {
                name: name?,
                description: description.unwrap_or_default(),
            },
            body,
        ))
    }

    /// Scan a single directory for .md files and return parsed skills.
    fn scan_directory(dir: &Path) -> Vec<Skill> {
        let mut skills = Vec::new();
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return skills,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "md") {
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        if let Some((meta, body)) = Self::parse_frontmatter(&content) {
                            let source_path = path.to_string_lossy().to_string();
                            debug!(name = %meta.name, path = %source_path, "Loaded skill");
                            skills.push(Skill {
                                meta,
                                body,
                                source_path,
                            });
                        }
                    }
                    Err(e) => {
                        warn!(path = %path.display(), "Failed to read skill file: {e}");
                    }
                }
            }
        }
        skills
    }
}

impl SkillLoader for FileSkillLoader {
    fn list_skills(&self) -> Vec<SkillMeta> {
        self.cache
            .read()
            .unwrap_or_else(|e| {
                warn!("skill cache read lock poisoned — recovering inner data");
                e.into_inner()
            })
            .values()
            .map(|s| s.meta.clone())
            .collect()
    }

    fn get_skill(&self, name: &str) -> Result<Skill, CoreError> {
        self.cache
            .read()
            .unwrap_or_else(|e| {
                warn!("skill cache read lock poisoned — recovering inner data");
                e.into_inner()
            })
            .get(name)
            .cloned()
            .ok_or_else(|| CoreError::NotFound {
                code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                resource_type: "Skill".into(),
                id: name.into(),
            })
    }

    fn reload(&self) -> Result<(), CoreError> {
        let mut all_skills = HashMap::new();

        for root in &self.search_roots {
            let skills_dir = root.join(SKILLS_DIR);
            if skills_dir.is_dir() {
                for skill in Self::scan_directory(&skills_dir) {
                    all_skills.insert(skill.meta.name.clone(), skill);
                }
            }
        }

        debug!(count = all_skills.len(), "Skill scan complete");

        let mut cache = self.cache.write().unwrap_or_else(|e| {
            warn!("skill cache write lock poisoned — recovering inner data");
            e.into_inner()
        });
        *cache = all_skills;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_skill_file(dir: &Path, filename: &str, content: &str) {
        let skills_dir = dir.join(SKILLS_DIR);
        fs::create_dir_all(&skills_dir).unwrap();
        fs::write(skills_dir.join(filename), content).unwrap();
    }

    #[test]
    fn parse_frontmatter_valid() {
        let content = r#"---
name: ui-automation
description: Automate UI actions based on user intent
---

# Instructions

Do the thing."#;
        let (meta, body) = FileSkillLoader::parse_frontmatter(content).unwrap();
        assert_eq!(meta.name, "ui-automation");
        assert_eq!(meta.description, "Automate UI actions based on user intent");
        assert!(body.starts_with("# Instructions"));
    }

    #[test]
    fn parse_frontmatter_quoted_values() {
        let content = "---\nname: \"my-skill\"\ndescription: 'A quoted desc'\n---\nBody here.";
        let (meta, body) = FileSkillLoader::parse_frontmatter(content).unwrap();
        assert_eq!(meta.name, "my-skill");
        assert_eq!(meta.description, "A quoted desc");
        assert_eq!(body, "Body here.");
    }

    #[test]
    fn parse_frontmatter_missing_delimiter() {
        let content = "---\nname: test\ndescription: x\nNo closing delimiter.";
        assert!(FileSkillLoader::parse_frontmatter(content).is_none());
    }

    #[test]
    fn parse_frontmatter_no_name() {
        let content = "---\ndescription: has no name\n---\nBody.";
        assert!(FileSkillLoader::parse_frontmatter(content).is_none());
    }

    #[test]
    fn loader_discovers_skills() {
        let tmp = TempDir::new().unwrap();
        create_skill_file(
            tmp.path(),
            "coding.md",
            "---\nname: coding\ndescription: Code helper\n---\nWrite code.",
        );
        create_skill_file(
            tmp.path(),
            "review.md",
            "---\nname: review\ndescription: Code reviewer\n---\nReview code.",
        );

        let loader = FileSkillLoader::new(vec![tmp.path().to_path_buf()]);
        let skills = loader.list_skills();
        assert_eq!(skills.len(), 2);

        let coding = loader.get_skill("coding").unwrap();
        assert_eq!(coding.body, "Write code.");
    }

    #[test]
    fn loader_ignores_non_md_files() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join(SKILLS_DIR);
        fs::create_dir_all(&skills_dir).unwrap();
        fs::write(
            skills_dir.join("not-skill.txt"),
            "---\nname: x\ndescription: y\n---\nbody",
        )
        .unwrap();

        let loader = FileSkillLoader::new(vec![tmp.path().to_path_buf()]);
        assert!(loader.list_skills().is_empty());
    }

    #[test]
    fn loader_returns_not_found_for_missing_skill() {
        let loader = FileSkillLoader::new(vec![]);
        let result = loader.get_skill("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn loader_reload_picks_up_new_files() {
        let tmp = TempDir::new().unwrap();
        let loader = FileSkillLoader::new(vec![tmp.path().to_path_buf()]);
        assert!(loader.list_skills().is_empty());

        create_skill_file(
            tmp.path(),
            "new.md",
            "---\nname: new-skill\ndescription: Added later\n---\nNew body.",
        );
        loader.reload().unwrap();
        assert_eq!(loader.list_skills().len(), 1);
    }
}
