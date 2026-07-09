//! Integration tests for SKILL.md file discovery and frontmatter parsing.
//!
//! #7734: imports the real `FileSkillLoader::parse_frontmatter` from the
//! `maekon_app` library target instead of a hand-maintained
//! re-implementation — a drift class the old duplication was exposed to
//! (the two copies could silently diverge with no compiler signal).

use std::fs;
use std::path::Path;

use maekon_app::skill_loader::FileSkillLoader;
use tempfile::TempDir;

const SKILLS_DIR: &str = ".agents/skills";

fn create_skill_file(root: &Path, filename: &str, content: &str) {
    let dir = root.join(SKILLS_DIR);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(filename), content).unwrap();
}

#[test]
fn frontmatter_parsing_valid() {
    let content = "---\nname: test-skill\ndescription: A test\n---\n# Body\nHello.";
    let (meta, body) = FileSkillLoader::parse_frontmatter(content).unwrap();
    assert_eq!(meta.name, "test-skill");
    assert_eq!(meta.description, "A test");
    assert!(body.contains("# Body"));
}

#[test]
fn frontmatter_rejects_missing_name() {
    let content = "---\ndescription: no name field\n---\nBody.";
    assert!(FileSkillLoader::parse_frontmatter(content).is_none());
}

#[test]
fn frontmatter_rejects_no_closing_delimiter() {
    let content = "---\nname: broken\ndescription: x\nNo closing.";
    assert!(FileSkillLoader::parse_frontmatter(content).is_none());
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
            if let Some((meta, _)) = FileSkillLoader::parse_frontmatter(&content) {
                found.push(meta.name);
            }
        }
    }
    found.sort();
    assert_eq!(found, vec!["coding", "review"]);
}
