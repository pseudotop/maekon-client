//! Keep the English and Korean ADR registry statuses aligned with each
//! canonical English ADR document.
//!
//! ADR-035 was accepted in #11330 while both registry rows still said Draft.
//! Reviewing the ADR alone therefore produced a false closeout. This gate
//! reads the source documents and both public indexes so another status change
//! cannot leave either locale behind.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("maekon-lint sits two levels under workspace root")
        .to_path_buf()
}

#[test]
fn adr_document_statuses_match_both_registry_indexes() {
    let architecture = workspace_root().join("docs/architecture");
    let documents = collect_document_statuses(&architecture);
    let english = parse_registry(&read(&architecture.join("README.md")));
    let korean = parse_registry(&read(&architecture.join("README.ko.md")));

    let violations = find_violations(&documents, &english, &korean);
    assert!(
        violations.is_empty(),
        "ADR registry status drift; update both README.md and README.ko.md with the canonical document status:\n{}",
        violations.join("\n")
    );
}

fn collect_document_statuses(architecture: &Path) -> BTreeMap<String, String> {
    let mut statuses = BTreeMap::new();
    for entry in std::fs::read_dir(architecture)
        .expect("read ADR directory")
        .flatten()
    {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(id) = canonical_adr_id(name) else {
            continue;
        };
        let source = read(&path);
        let status = parse_document_status(&source)
            .unwrap_or_else(|| panic!("{} has no **Status** header", path.display()));
        statuses.insert(id, status);
    }
    statuses
}

fn canonical_adr_id(name: &str) -> Option<String> {
    let rest = name.strip_prefix("ADR-")?;
    let id = rest.get(..3)?;
    if !id.chars().all(|character| character.is_ascii_digit())
        || !rest.get(3..)?.starts_with('-')
        || !name.ends_with(".md")
        || name.ends_with(".ko.md")
    {
        return None;
    }
    Some(id.to_owned())
}

fn parse_document_status(source: &str) -> Option<String> {
    source.lines().find_map(|line| {
        let trimmed = line.trim();
        let inline = trimmed
            .strip_prefix("**Status**:")
            .or_else(|| trimmed.strip_prefix("- **Status**:"))
            .or_else(|| trimmed.strip_prefix("Status:"));
        if let Some(value) = inline {
            return value.split_whitespace().next().map(str::to_owned);
        }

        let columns: Vec<&str> = trimmed.split('|').map(str::trim).collect();
        if columns.len() >= 4 && columns[1] == "Status" {
            return columns[2].split_whitespace().next().map(str::to_owned);
        }
        None
    })
}

fn parse_registry(source: &str) -> BTreeMap<String, String> {
    let mut statuses = BTreeMap::new();
    for line in source.lines() {
        let columns: Vec<&str> = line.split('|').map(str::trim).collect();
        if columns.len() < 5 {
            continue;
        }
        let Some(id) = columns[1]
            .strip_prefix('[')
            .and_then(|value| value.split_once(']'))
            .map(|(id, _)| id)
            .filter(|id| id.len() == 3 && id.chars().all(|character| character.is_ascii_digit()))
        else {
            continue;
        };
        statuses.insert(id.to_owned(), columns[3].trim_matches('`').to_owned());
    }
    statuses
}

fn find_violations(
    documents: &BTreeMap<String, String>,
    english: &BTreeMap<String, String>,
    korean: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut violations = Vec::new();
    for (id, expected) in documents {
        for (locale, registry) in [("English", english), ("Korean", korean)] {
            match registry.get(id) {
                Some(actual) if actual == expected => {}
                Some(actual) => violations.push(format!(
                    "ADR-{id}: {locale} registry={actual}, document={expected}"
                )),
                None => violations.push(format!(
                    "ADR-{id}: {locale} registry row missing, document={expected}"
                )),
            }
        }
    }
    for (locale, registry) in [("English", english), ("Korean", korean)] {
        for id in registry.keys().filter(|id| !documents.contains_key(*id)) {
            violations.push(format!(
                "ADR-{id}: {locale} registry has no canonical document"
            ));
        }
    }
    violations
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

#[test]
fn parser_ignores_status_detail_after_the_keyword() {
    assert_eq!(
        parse_document_status("**Status**: Accepted — 2026-08-24\n"),
        Some("Accepted".to_owned())
    );
}

#[test]
fn parser_supports_legacy_plain_list_and_table_headers() {
    for source in [
        "Status: Accepted\n",
        "- **Status**: Accepted (2026-04-19)\n",
        "| Field | Value |\n| Status | Accepted |\n",
    ] {
        assert_eq!(parse_document_status(source), Some("Accepted".to_owned()));
    }
}

#[test]
fn mismatch_is_reported_for_each_stale_locale() {
    let documents = BTreeMap::from([("035".to_owned(), "Accepted".to_owned())]);
    let english = BTreeMap::from([("035".to_owned(), "Draft".to_owned())]);
    let korean = BTreeMap::from([("035".to_owned(), "Proposed".to_owned())]);

    assert_eq!(
        find_violations(&documents, &english, &korean),
        vec![
            "ADR-035: English registry=Draft, document=Accepted".to_owned(),
            "ADR-035: Korean registry=Proposed, document=Accepted".to_owned(),
        ]
    );
}
