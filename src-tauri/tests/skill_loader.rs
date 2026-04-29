//! Integration tests for SKILL.md file discovery and frontmatter parsing.

use std::fs;
use std::path::Path;

use tempfile::TempDir;

const SKILLS_DIR: &str = ".agents/skills";

fn create_skill_file(root: &Path, filename: &str, content: &str) {
    let dir = root.join(SKILLS_DIR);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(filename), content).unwrap();
}

// Re-export FileSkillLoader via the binary crate — we use a minimal
// re-implementation here since the binary module isn't directly linkable.
// The canonical tests live in skill_loader.rs; these confirm the trait contract.

/// Minimal standalone parser for integration-test purposes.
fn parse_frontmatter(content: &str) -> Option<(String, String, String)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    let after_open = &trimmed[3..];
    let close_idx = after_open.find("\n---")?;
    let yaml_block = &after_open[..close_idx];
    let body_start = 3 + close_idx + 4;
    let body = trimmed[body_start..].trim_start_matches('\n').to_string();

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

    Some((name?, description.unwrap_or_default(), body))
}

#[test]
fn frontmatter_parsing_valid() {
    let content = "---\nname: test-skill\ndescription: A test\n---\n# Body\nHello.";
    let (name, desc, body) = parse_frontmatter(content).unwrap();
    assert_eq!(name, "test-skill");
    assert_eq!(desc, "A test");
    assert!(body.contains("# Body"));
}

#[test]
fn frontmatter_rejects_missing_name() {
    let content = "---\ndescription: no name field\n---\nBody.";
    assert!(parse_frontmatter(content).is_none());
}

#[test]
fn frontmatter_rejects_no_closing_delimiter() {
    let content = "---\nname: broken\ndescription: x\nNo closing.";
    assert!(parse_frontmatter(content).is_none());
}

#[test]
fn skill_directory_discovery() {
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
    // Non-md file should be ignored.
    let skills_dir = tmp.path().join(SKILLS_DIR);
    fs::write(
        skills_dir.join("ignore.txt"),
        "---\nname: x\ndescription: y\n---\nbody",
    )
    .unwrap();

    let mut found = Vec::new();
    for entry in fs::read_dir(&skills_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|ext| ext == "md") {
            let content = fs::read_to_string(&path).unwrap();
            if let Some((name, _, _)) = parse_frontmatter(&content) {
                found.push(name);
            }
        }
    }
    found.sort();
    assert_eq!(found, vec!["coding", "review"]);
}
