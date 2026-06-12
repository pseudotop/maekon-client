use crate::i18n::{
    contains_human_text, detect_hardcoded_ui_literals, extract_translation_keys, flatten_json_keys,
    has_translation_key, load_locale_keys, scan_i18n,
};
use std::collections::BTreeSet;
use std::io::Write;

#[test]
fn extract_translation_keys_works() {
    let line = r#"const x = t('dashboard.title', 'Dashboard'); const y = i18n.t("common.save")"#;
    let keys = extract_translation_keys(line);
    assert_eq!(keys, vec!["dashboard.title", "common.save"]);
}

#[test]
fn extract_translation_keys_unicode() {
    let keys = extract_translation_keys("t('settings.édition')");
    assert_eq!(keys, vec!["settings.édition"]);
}

#[test]
fn extract_translation_keys_ignores_non_i18n_calls() {
    let line = r#"const value = set("x"); const n = get(1);"#;
    let keys = extract_translation_keys(line);
    assert!(keys.is_empty());
}

#[test]
fn contains_human_text_heuristic() {
    assert!(contains_human_text("Click to continue"));
    assert!(contains_human_text("사용자 설정"));
    assert!(!contains_human_text("12345"));
    assert!(!contains_human_text("OK"));
}

#[test]
fn contains_human_text_urls_rejected() {
    assert!(!contains_human_text("http://example.com"));
    assert!(!contains_human_text("https://example.com/path"));
    assert!(!contains_human_text("/api/v1/users"));
}

#[test]
fn contains_human_text_short_strings_rejected() {
    assert!(!contains_human_text(""));
    assert!(!contains_human_text("X"));
    assert!(!contains_human_text(" "));
}

#[test]
fn contains_human_text_short_uppercase_acronyms_rejected() {
    assert!(!contains_human_text("ID"));
    assert!(!contains_human_text("URL"));
    assert!(!contains_human_text("HTTP"));
}

#[test]
fn contains_human_text_long_uppercase_accepted() {
    assert!(contains_human_text("HELLO"));
    assert!(contains_human_text("SETTINGS"));
}

#[test]
fn contains_human_text_whitespace_trimmed() {
    assert!(!contains_human_text("  "));
    assert!(!contains_human_text("  1  "));
    assert!(contains_human_text("  Hello World  "));
}

#[test]
fn contains_human_text_mixed_content() {
    assert!(contains_human_text("Item #42"));
    assert!(contains_human_text("v2.0 release"));
    assert!(!contains_human_text("123-456"));
    assert!(!contains_human_text("###"));
}

#[test]
fn flatten_json_keys_works() {
    let value = serde_json::json!({
        "common": {
            "save": "Save",
            "cancel": "Cancel"
        },
        "dashboard": {
            "title": "Dashboard"
        }
    });
    let mut keys = BTreeSet::new();
    flatten_json_keys("", &value, &mut keys);
    assert!(keys.contains("common.save"));
    assert!(keys.contains("common.cancel"));
    assert!(keys.contains("dashboard.title"));
}

#[test]
fn i18n_key_lookup_accepts_plural_resource_forms() {
    let mut keys = BTreeSet::new();
    keys.insert("timeline.selectedCount_one".to_string());
    keys.insert("timeline.selectedCount_other".to_string());

    assert!(has_translation_key(&keys, "timeline.selectedCount"));
}

#[test]
fn i18n_key_lookup_accepts_context_resource_forms() {
    let mut keys = BTreeSet::new();
    keys.insert("settings.autostart.unsupported_snap_sandbox".to_string());

    assert!(has_translation_key(&keys, "settings.autostart.unsupported"));
}

#[test]
fn detect_hardcoded_placeholder_attribute() {
    let line = r#"<Input placeholder="Enter your name" />"#;
    let hits = detect_hardcoded_ui_literals(line);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].1.contains("placeholder"));
}

#[test]
fn detect_hardcoded_title_attribute() {
    let line = r#"<div title="Click to expand details">"#;
    let hits = detect_hardcoded_ui_literals(line);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].1.contains("title"));
}

#[test]
fn detect_hardcoded_aria_label() {
    let line = r#"<button aria-label="Close dialog">"#;
    let hits = detect_hardcoded_ui_literals(line);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].1.contains("aria-label"));
}

#[test]
fn detect_hardcoded_label_attribute() {
    let line = r#"<Field label="Username" />"#;
    let hits = detect_hardcoded_ui_literals(line);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].1.contains("label"));
}

#[test]
fn detect_hardcoded_helper_text() {
    let line = r#"<TextField helperText="Must be at least 8 characters" />"#;
    let hits = detect_hardcoded_ui_literals(line);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].1.contains("helperText"));
}

#[test]
fn detect_hardcoded_alt_attribute() {
    let line = r#"<img alt="User avatar" src="pic.png" />"#;
    let hits = detect_hardcoded_ui_literals(line);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].1.contains("alt"));
}

#[test]
fn detect_hardcoded_tooltip_attribute() {
    let line = r#"<Icon tooltip="Show more options" />"#;
    let hits = detect_hardcoded_ui_literals(line);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].1.contains("tooltip"));
}

#[test]
fn detect_hardcoded_multiple_attrs_on_same_line() {
    let line = r#"<Input placeholder="Enter name" title="Name field" />"#;
    let hits = detect_hardcoded_ui_literals(line);
    assert_eq!(hits.len(), 2);
}

#[test]
fn detect_hardcoded_skips_non_human_values() {
    let line = r#"<img alt="/icons/logo.svg" />"#;
    let hits = detect_hardcoded_ui_literals(line);
    assert!(hits.is_empty());
}

#[test]
fn detect_hardcoded_skips_short_acronym_values() {
    let line = r#"<span title="ID" />"#;
    let hits = detect_hardcoded_ui_literals(line);
    assert!(hits.is_empty());
}

#[test]
fn detect_hardcoded_text_node_between_tags() {
    let line = r#"<span>Submit form</span>"#;
    let hits = detect_hardcoded_ui_literals(line);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].1.contains("text node"));
}

#[test]
fn detect_hardcoded_text_node_skips_jsx_expression() {
    let line = r#"<span>{userName}</span>"#;
    let hits = detect_hardcoded_ui_literals(line);
    assert!(hits.is_empty());
}

#[test]
fn detect_hardcoded_text_node_skips_empty_content() {
    let line = r#"<span></span>"#;
    let hits = detect_hardcoded_ui_literals(line);
    assert!(hits.is_empty());
}

#[test]
fn detect_hardcoded_text_node_skips_numeric_content() {
    let line = r#"<span>42</span>"#;
    let hits = detect_hardcoded_ui_literals(line);
    assert!(hits.is_empty());
}

#[test]
fn detect_hardcoded_no_false_positive_on_data_attr() {
    let line = r#"<div data-placeholder="internal value" />"#;
    let hits = detect_hardcoded_ui_literals(line);
    assert!(hits.is_empty());
}

#[test]
fn detect_nested_jsx_tags() {
    let hits = detect_hardcoded_ui_literals("<div><span>Submit</span></div>");
    assert!(!hits.is_empty(), "should detect 'Submit' in nested tags");
}

#[test]
fn detect_hardcoded_attr_preceded_by_alnum_skipped() {
    let line = r#"<Input myplaceholder="Enter name" />"#;
    let hits = detect_hardcoded_ui_literals(line);
    assert!(hits.is_empty());
}

#[test]
fn load_locale_keys_valid_flat_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("en.json");
    let mut f = std::fs::File::create(&path).unwrap();
    write!(f, r#"{{"save":"Save","cancel":"Cancel"}}"#).unwrap();

    let keys = load_locale_keys(&path).unwrap();
    assert_eq!(keys.len(), 2);
    assert!(keys.contains("save"));
    assert!(keys.contains("cancel"));
}

#[test]
fn load_locale_keys_valid_nested_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("en.json");
    let mut f = std::fs::File::create(&path).unwrap();
    write!(
        f,
        r#"{{"common":{{"save":"Save","cancel":"Cancel"}},"dashboard":{{"title":"Dashboard"}}}}"#
    )
    .unwrap();

    let keys = load_locale_keys(&path).unwrap();
    assert_eq!(keys.len(), 3);
    assert!(keys.contains("common.save"));
    assert!(keys.contains("common.cancel"));
    assert!(keys.contains("dashboard.title"));
}

#[test]
fn load_locale_keys_deeply_nested_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("en.json");
    let mut f = std::fs::File::create(&path).unwrap();
    write!(f, r#"{{"a":{{"b":{{"c":"deep"}}}}}}"#).unwrap();

    let keys = load_locale_keys(&path).unwrap();
    assert_eq!(keys.len(), 1);
    assert!(keys.contains("a.b.c"));
}

#[test]
fn load_locale_keys_file_not_found() {
    let err =
        load_locale_keys(&std::env::temp_dir().join("nonexistent-locale-file.json")).unwrap_err();
    assert!(err.contains("Failed to read locale file"));
}

#[test]
fn load_locale_keys_invalid_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.json");
    let mut f = std::fs::File::create(&path).unwrap();
    write!(f, "{{not valid json}}").unwrap();

    let err = load_locale_keys(&path).unwrap_err();
    assert!(err.contains("Failed to parse locale JSON"));
}

#[test]
fn load_locale_keys_empty_object() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.json");
    let mut f = std::fs::File::create(&path).unwrap();
    write!(f, "{{}}").unwrap();

    let keys = load_locale_keys(&path).unwrap();
    assert!(keys.is_empty());
}

#[test]
fn scan_i18n_excludes_storybook_and_test_fixture_hardcoded_copy() {
    let dir = tempfile::tempdir().unwrap();
    let locale_dir = dir
        .path()
        .join("crates/maekon-web/frontend/src/i18n/locales");
    std::fs::create_dir_all(&locale_dir).unwrap();
    for locale in ["en", "ko", "ja", "zh-CN", "es"] {
        let mut file = std::fs::File::create(locale_dir.join(format!("{locale}.json"))).unwrap();
        write!(file, "{{}}").unwrap();
    }

    let story_path = dir
        .path()
        .join("crates/maekon-web/frontend/src/components/ui/Card.stories.tsx");
    std::fs::create_dir_all(story_path.parent().unwrap()).unwrap();
    let mut story = std::fs::File::create(&story_path).unwrap();
    writeln!(story, r#"<CardTitle>Card Title</CardTitle>"#).unwrap();

    let story_helper_path = dir
        .path()
        .join("crates/maekon-web/frontend/src/stories/storybook-helpers.tsx");
    std::fs::create_dir_all(story_helper_path.parent().unwrap()).unwrap();
    let mut story_helper = std::fs::File::create(&story_helper_path).unwrap();
    writeln!(story_helper, r#"<span>Storybook review frame</span>"#).unwrap();

    let test_path = dir
        .path()
        .join("crates/maekon-web/frontend/src/pages/chat/ChatInput.test.tsx");
    std::fs::create_dir_all(test_path.parent().unwrap()).unwrap();
    let mut test = std::fs::File::create(&test_path).unwrap();
    writeln!(test, r#"<button aria-label="Send message" />"#).unwrap();

    let product_path = dir
        .path()
        .join("crates/maekon-web/frontend/src/pages/chat/index.tsx");
    let mut product = std::fs::File::create(&product_path).unwrap();
    writeln!(product, r#"<button aria-label="Send message" />"#).unwrap();

    let findings = scan_i18n(dir.path(), &[]);
    let hardcoded: Vec<_> = findings
        .iter()
        .filter(|finding| finding.category == "hardcoded-ui-copy")
        .collect();

    assert_eq!(hardcoded.len(), 1);
    assert_eq!(hardcoded[0].path, product_path);
}
