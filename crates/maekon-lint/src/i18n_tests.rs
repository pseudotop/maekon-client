use crate::i18n::{
    contains_human_text, detect_hardcoded_ui_literals, extract_template_literal_t_calls,
    extract_translation_keys, flatten_json_keys, has_translation_key, load_locale_keys, scan_i18n,
    strip_jsx_expressions,
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

// ── Fix #15: template-literal t() extraction ──────────────────────────────────

/// A template literal with an interpolation should yield a notice with the
/// static prefix before `${`.
#[test]
fn extract_template_literal_t_calls_dynamic_prefix() {
    // e.g. t(`heatmap.${key}`)  — static prefix is "heatmap."
    let results = extract_template_literal_t_calls("t(`heatmap.${key}`)");
    assert_eq!(results.len(), 1);
    let (col, prefix, call) = &results[0];
    assert_eq!(*col, 1);
    assert_eq!(prefix, "heatmap.");
    assert!(call.contains("heatmap.${key}"));
}

/// A fully static template literal (no `${`) should also be captured — it is
/// invisible to `extract_translation_keys` which only handles quote strings.
#[test]
fn extract_template_literal_t_calls_fully_static() {
    let results = extract_template_literal_t_calls("t(`common.save`)");
    assert_eq!(results.len(), 1);
    let (_col, prefix, call) = &results[0];
    assert_eq!(prefix, "common.save");
    assert!(call.contains("common.save"));
}

/// A line with no template-literal t() call produces no results.
#[test]
fn extract_template_literal_t_calls_none_present() {
    let results = extract_template_literal_t_calls(r#"const x = t('common.save');"#);
    assert!(
        results.is_empty(),
        "single-quote t() should not be picked up by the template-literal extractor"
    );
}

/// Alphanumeric prefix before `t(` (e.g. `set(`) must be ignored.
#[test]
fn extract_template_literal_t_calls_alnum_prefix_rejected() {
    let results = extract_template_literal_t_calls("reset(`my.key`)");
    assert!(
        results.is_empty(),
        "set(`...`) is not a standalone t() call"
    );
}

/// Multiple template-literal t() calls on one line.
#[test]
fn extract_template_literal_t_calls_multiple_on_line() {
    let line = "t(`a.${x}`) + t(`b.${y}`)";
    let results = extract_template_literal_t_calls(line);
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].1, "a.");
    assert_eq!(results[1].1, "b.");
}

/// Unclosed backtick on the same line: skip without panicking.
#[test]
fn extract_template_literal_t_calls_unclosed_backtick_skipped() {
    let results = extract_template_literal_t_calls("t(`unclosed");
    assert!(
        results.is_empty(),
        "unclosed backtick should not panic or produce a result"
    );
}

// ── Fix #8: JSON array expansion ──────────────────────────────────────────────

/// An array value should expand to indexed keys `[0]`, `[1]`, etc.
#[test]
fn flatten_json_keys_array_expanded_per_element() {
    let value = serde_json::json!({
        "heatmap": {
            "days": ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
        }
    });
    let mut keys = BTreeSet::new();
    flatten_json_keys("", &value, &mut keys);

    assert!(
        keys.contains("heatmap.days[0]"),
        "first element must appear as [0]"
    );
    assert!(
        keys.contains("heatmap.days[6]"),
        "seventh element must appear as [6]"
    );
    assert!(
        keys.contains("heatmap.days"),
        "the bare array key must also appear (supports returnObjects:true retrieval)"
    );
    assert_eq!(
        keys.iter()
            .filter(|k| k.starts_with("heatmap.days["))
            .count(),
        7,
        "all 7 array elements must be expanded"
    );
}

/// An empty array produces only the bare key (no per-element keys). A locale
/// whose array is empty relative to a populated baseline is still caught: the
/// baseline's `[0]`… per-element keys are missing from the empty locale.
#[test]
fn flatten_json_keys_empty_array_produces_only_bare_key() {
    let value = serde_json::json!({ "days": [] });
    let mut keys = BTreeSet::new();
    flatten_json_keys("", &value, &mut keys);
    assert_eq!(
        keys.into_iter().collect::<Vec<_>>(),
        vec!["days".to_string()],
        "an empty array yields only the bare key (returnObjects), no per-element keys"
    );
}

/// Mixed-depth JSON with an array inside a nested object.
#[test]
fn flatten_json_keys_nested_array() {
    let value = serde_json::json!({ "a": { "b": ["x", "y"] } });
    let mut keys = BTreeSet::new();
    flatten_json_keys("", &value, &mut keys);
    assert!(
        keys.contains("a.b"),
        "bare array key present (returnObjects)"
    );
    assert!(keys.contains("a.b[0]"));
    assert!(keys.contains("a.b[1]"));
    assert_eq!(keys.len(), 3);
}

/// Existing behavior for plain object keys is unchanged.
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

// ── Fix #9: baseline en.json load failure ─────────────────────────────────────

/// When en.json is absent the scan must return exactly ONE fatal error (the
/// locale-load error) and the early-abort sentinel, without producing thousands
/// of spurious missing-key or extra-key findings.
#[test]
fn scan_i18n_missing_en_json_emits_fatal_and_aborts() {
    let dir = tempfile::tempdir().unwrap();
    let locale_dir = dir
        .path()
        .join("crates/maekon-web/frontend/src/i18n/locales");
    std::fs::create_dir_all(&locale_dir).unwrap();

    // Write all locales EXCEPT en.json.
    for locale in ["ko", "ja", "zh-CN", "es"] {
        let mut file = std::fs::File::create(locale_dir.join(format!("{locale}.json"))).unwrap();
        write!(file, r#"{{"common":{{"save":"저장"}}}}"#).unwrap();
    }

    // Place a product tsx that uses a key so we can confirm no missing-i18n-key
    // errors are generated.
    let src_dir = dir.path().join("crates/maekon-web/frontend/src/pages");
    std::fs::create_dir_all(&src_dir).unwrap();
    let mut tsx = std::fs::File::create(src_dir.join("index.tsx")).unwrap();
    writeln!(tsx, r#"t('common.save')"#).unwrap();

    let findings = scan_i18n(dir.path(), &[]);

    // Exactly one locale-load error (for en.json) + one baseline-locale-missing.
    let load_errors: Vec<_> = findings
        .iter()
        .filter(|f| f.category == "locale-load")
        .collect();
    let fatal_errors: Vec<_> = findings
        .iter()
        .filter(|f| f.category == "baseline-locale-missing")
        .collect();
    let missing_key_errors: Vec<_> = findings
        .iter()
        .filter(|f| f.category == "missing-i18n-key")
        .collect();
    let missing_locale_errors: Vec<_> = findings
        .iter()
        .filter(|f| f.category == "missing-locale-key")
        .collect();

    assert_eq!(
        load_errors.len(),
        1,
        "exactly one locale-load error expected (for en.json)"
    );
    assert_eq!(
        fatal_errors.len(),
        1,
        "exactly one baseline-locale-missing sentinel expected"
    );
    assert!(
        missing_key_errors.is_empty(),
        "no missing-i18n-key errors should be emitted when en.json is absent"
    );
    assert!(
        missing_locale_errors.is_empty(),
        "no missing-locale-key errors should be emitted when en.json is absent"
    );
}

// ── Fix #10: JSX text-node mixed content ─────────────────────────────────────

/// `strip_jsx_expressions` removes `{...}` spans and collapses whitespace.
#[test]
fn strip_jsx_expressions_removes_expression_spans() {
    assert_eq!(strip_jsx_expressions("Save {count}"), "Save");
    assert_eq!(strip_jsx_expressions("{count} items"), "items");
    assert_eq!(strip_jsx_expressions("Hello {name}!"), "Hello !");
    assert_eq!(strip_jsx_expressions("{expr}"), "");
}

/// Nested braces are handled correctly.
#[test]
fn strip_jsx_expressions_nested_braces() {
    // {fn({ key: val })} — the inner braces must not terminate stripping early.
    assert_eq!(
        strip_jsx_expressions("Result: {fn({ key: val })}"),
        "Result:"
    );
}

/// Segments that are purely static text are unchanged (except whitespace
/// normalisation).
#[test]
fn strip_jsx_expressions_no_expressions_unchanged() {
    assert_eq!(strip_jsx_expressions("Submit form"), "Submit form");
}

/// Mixed segment `Save {count}` used to be silently skipped because it ends
/// with `}`.  After fix #10 the static text `Save` must be detected.
#[test]
fn detect_hardcoded_text_node_mixed_content_trailing_expr_flagged() {
    let line = r#"<button>Save {count}</button>"#;
    let hits = detect_hardcoded_ui_literals(line);
    assert_eq!(
        hits.len(),
        1,
        "mixed 'Save {{count}}' should be flagged — static text 'Save' is hardcoded copy"
    );
    assert!(hits[0].1.contains("text node"));
}

/// Mixed segment with expression FIRST: `{count} items` used to be skipped
/// because it starts with `{`.
#[test]
fn detect_hardcoded_text_node_mixed_content_leading_expr_flagged() {
    let line = r#"<span>{count} items selected</span>"#;
    let hits = detect_hardcoded_ui_literals(line);
    assert_eq!(
        hits.len(),
        1,
        "\"{{count}} items selected\" should be flagged — 'items selected' is hardcoded copy"
    );
}

/// Pure JSX expression (`{userName}`) must still be skipped (no regression).
#[test]
fn detect_hardcoded_text_node_skips_jsx_expression() {
    let line = r#"<span>{userName}</span>"#;
    let hits = detect_hardcoded_ui_literals(line);
    assert!(hits.is_empty());
}

/// #6424: a commented-out line carrying a UI attribute (or JSX text) must NOT be
/// flagged — the comment guard now runs before the attribute scan, so commented-out
/// markup no longer produces a spurious --strict-i18n build failure.
#[test]
fn detect_hardcoded_skips_commented_out_jsx_attribute() {
    assert!(detect_hardcoded_ui_literals(r#"// <Button title="Save changes" />"#).is_empty());
    assert!(detect_hardcoded_ui_literals(r#"  * title="Old copy text""#).is_empty());
    // Sanity: the same attribute on a LIVE (non-comment) line is still flagged.
    assert!(!detect_hardcoded_ui_literals(r#"<Button title="Save changes" />"#).is_empty());
}

// ── Fix #11: restricted plural/context suffix matching ────────────────────────

/// Direct key match still works.
#[test]
fn i18n_key_lookup_direct_match() {
    let mut keys = BTreeSet::new();
    keys.insert("common.save".to_string());
    assert!(has_translation_key(&keys, "common.save"));
}

/// Recognized ICU plural suffixes are accepted.
#[test]
fn i18n_key_lookup_accepts_plural_resource_forms() {
    let mut keys = BTreeSet::new();
    keys.insert("timeline.selectedCount_one".to_string());
    keys.insert("timeline.selectedCount_other".to_string());

    assert!(has_translation_key(&keys, "timeline.selectedCount"));
}

/// All other recognized plural suffixes must also be accepted.
#[test]
fn i18n_key_lookup_accepts_all_plural_suffixes() {
    for suffix in &["_zero", "_two", "_few", "_many", "_plural"] {
        let mut keys = BTreeSet::new();
        keys.insert(format!("count{suffix}"));
        assert!(
            has_translation_key(&keys, "count"),
            "suffix {suffix} should be recognized"
        );
    }
}

/// i18next context forms (`{key}_{context}`) are accepted via the range scan
/// fallback.  This is intentional — context values are arbitrary runtime
/// strings and cannot be enumerated statically.
#[test]
fn i18n_key_lookup_accepts_context_resource_forms() {
    let mut keys = BTreeSet::new();
    keys.insert("settings.autostart.unsupported_snap_sandbox".to_string());

    assert!(has_translation_key(&keys, "settings.autostart.unsupported"));
}

/// A key must NOT be accepted when only an *unrelated* key that happens to
/// start with the same substring (but joined by a DOT, not underscore) exists.
/// e.g. `t('foo')` when only `foo.bar` exists — that is a distinct key, not a
/// plural/context form.
#[test]
fn i18n_key_lookup_rejects_dot_sibling() {
    let mut keys = BTreeSet::new();
    keys.insert("foo.bar".to_string());
    // `foo` does not exist; `foo.bar` is not a prefix match via `_`.
    assert!(
        !has_translation_key(&keys, "foo"),
        "foo.bar is not a plural/context form of foo"
    );
}

/// A missing key with no related forms at all is correctly rejected.
#[test]
fn i18n_key_lookup_rejects_missing_key() {
    let mut keys = BTreeSet::new();
    keys.insert("common.save".to_string());
    assert!(!has_translation_key(&keys, "common.cancel"));
}

// ── Unchanged / regression tests ─────────────────────────────────────────────

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

/// Fix #8 regression: load_locale_keys on a file containing an array value
/// must expand the array elements.
#[test]
fn load_locale_keys_array_value_expanded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("en.json");
    let mut f = std::fs::File::create(&path).unwrap();
    write!(f, r#"{{"heatmap":{{"days":["Mon","Tue","Wed"]}}}}"#).unwrap();

    let keys = load_locale_keys(&path).unwrap();
    assert!(keys.contains("heatmap.days[0]"));
    assert!(keys.contains("heatmap.days[1]"));
    assert!(keys.contains("heatmap.days[2]"));
    assert!(
        keys.contains("heatmap.days"),
        "bare array key must also appear (returnObjects:true retrieval)"
    );
    assert_eq!(keys.len(), 4);
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

// ── MLINT-C1: cross-line block-comment state (i18n.rs partial from #7493) ────
//
// `extract_translation_keys` / `detect_hardcoded_ui_literals` only had a
// PER-LINE comment-prefix skip (trimmed start is `//`, `*`, or `/*`), with no
// cross-line block-comment state. A continuation line inside a multi-line
// `/* ... */` block that does not itself start with a comment marker was
// parsed as LIVE code. `scan_i18n` now threads an `in_block_comment` boolean
// across lines via `advance_block_comment_state` so both consumers skip every
// line inside an open block comment.

/// (a) A `t('...')` call written on a continuation line of a multi-line
/// `/* ... */` block — with NO leading comment marker on that line — must NOT
/// be extracted as a live translation key, so an absent key inside the
/// comment does not HARD-FAIL the build with `missing-i18n-key`.
///
/// Fails before the fix: `extract_translation_keys` resets its own
/// `block_comment` local to `false` on every call (one call per line), so the
/// continuation line `" * t('doc.example.key')"` is scanned fresh and its
/// `t(` call site is found — `doc.example.key` (absent from en.json) then
/// produces a `missing-i18n-key` Error, verified via the verify-first scratch
/// repro against this branch's base commit before this fix was implemented.
#[test]
fn scan_i18n_skips_translation_key_inside_multiline_block_comment() {
    let dir = tempfile::tempdir().unwrap();
    let locale_dir = dir
        .path()
        .join("crates/maekon-web/frontend/src/i18n/locales");
    std::fs::create_dir_all(&locale_dir).unwrap();
    for locale in ["en", "ko", "ja", "zh-CN", "es"] {
        let mut file = std::fs::File::create(locale_dir.join(format!("{locale}.json"))).unwrap();
        write!(file, "{{}}").unwrap();
    }

    let product_path = dir
        .path()
        .join("crates/maekon-web/frontend/src/pages/example/DocBlock.tsx");
    std::fs::create_dir_all(product_path.parent().unwrap()).unwrap();
    let mut product = std::fs::File::create(&product_path).unwrap();
    writeln!(product, "/*").unwrap();
    writeln!(product, " * Example usage:").unwrap();
    writeln!(product, " * t('doc.example.key')").unwrap();
    writeln!(product, " */").unwrap();

    let findings = scan_i18n(dir.path(), &[]);
    let missing: Vec<_> = findings
        .iter()
        .filter(|f| f.category == "missing-i18n-key")
        .collect();

    assert!(
        missing.is_empty(),
        "a t() call inside a multi-line block comment must not be extracted: {missing:?}"
    );
}

/// (b) A `title="..."` UI attribute (and its enclosing text) written on a
/// continuation line of a multi-line `/** ... */` JSDoc block — with no
/// leading `*` gutter on that specific line — must NOT be flagged as
/// hardcoded UI copy.
///
/// Fails before the fix: `detect_hardcoded_ui_literals`'s comment guard is a
/// PER-LINE `trimmed.starts_with("//" | '*' | "/*")` check with no cross-line
/// state, so a continuation line lacking any of those markers (a realistic
/// shape for pasted example markup inside a `@example` doc block) slips past
/// the guard and is scanned as live JSX, flagging `title="Hello"`.
#[test]
fn scan_i18n_skips_hardcoded_ui_literal_inside_multiline_block_comment() {
    let dir = tempfile::tempdir().unwrap();
    let locale_dir = dir
        .path()
        .join("crates/maekon-web/frontend/src/i18n/locales");
    std::fs::create_dir_all(&locale_dir).unwrap();
    for locale in ["en", "ko", "ja", "zh-CN", "es"] {
        let mut file = std::fs::File::create(locale_dir.join(format!("{locale}.json"))).unwrap();
        write!(file, "{{}}").unwrap();
    }

    let product_path = dir
        .path()
        .join("crates/maekon-web/frontend/src/pages/example/DocBlockAttr.tsx");
    std::fs::create_dir_all(product_path.parent().unwrap()).unwrap();
    let mut product = std::fs::File::create(&product_path).unwrap();
    writeln!(product, "/**").unwrap();
    writeln!(product, " * Renders:").unwrap();
    writeln!(product, "   <div title=\"Hello\">").unwrap();
    writeln!(product, "     Some prose text").unwrap();
    writeln!(product, "   </div>").unwrap();
    writeln!(product, " */").unwrap();

    let findings = scan_i18n(dir.path(), &[]);
    let hardcoded: Vec<_> = findings
        .iter()
        .filter(|f| f.category == "hardcoded-ui-copy")
        .collect();

    assert!(
        hardcoded.is_empty(),
        "UI markup inside a multi-line JSDoc block comment must not be flagged: {hardcoded:?}"
    );
}

/// (c) A LIVE `t('...')` call OUTSIDE any comment — in the same file as a
/// suppressed comment-embedded example — must still be extracted and
/// validated normally (no false-negative regression from the new cross-line
/// skip).
#[test]
fn scan_i18n_still_detects_live_translation_key_outside_comment() {
    let dir = tempfile::tempdir().unwrap();
    let locale_dir = dir
        .path()
        .join("crates/maekon-web/frontend/src/i18n/locales");
    std::fs::create_dir_all(&locale_dir).unwrap();
    for locale in ["en", "ko", "ja", "zh-CN", "es"] {
        let mut file = std::fs::File::create(locale_dir.join(format!("{locale}.json"))).unwrap();
        write!(file, "{{}}").unwrap();
    }

    let product_path = dir
        .path()
        .join("crates/maekon-web/frontend/src/pages/example/LiveKey.tsx");
    std::fs::create_dir_all(product_path.parent().unwrap()).unwrap();
    let mut product = std::fs::File::create(&product_path).unwrap();
    writeln!(product, "/*").unwrap();
    writeln!(product, " * t('doc.example.key')").unwrap();
    writeln!(product, " */").unwrap();
    writeln!(product, "const label = t('doc.live.key');").unwrap();

    let findings = scan_i18n(dir.path(), &[]);
    let missing: Vec<_> = findings
        .iter()
        .filter(|f| f.category == "missing-i18n-key")
        .collect();

    assert_eq!(
        missing.len(),
        1,
        "the live t() call outside the comment must still be extracted and validated: {missing:?}"
    );
    assert!(missing[0].message.contains("doc.live.key"));
    assert_eq!(missing[0].line, 4);
}

// ── MLINT-C2: residual live-code scan on a block-comment CLOSING line ───────
//
// MLINT-C1's whole-line skip (`if advance_block_comment_state(...) { continue;
// }`) over-corrected: it excluded the ENTIRE line whenever the line STARTED
// inside a block comment ("entering" state), including a line that CLOSES the
// comment mid-line and then carries LIVE code after `*/`. `scan_i18n` now
// scans a MASKED line (block-comment spans blanked, live code — including
// live code after a same-line closing `*/` — left intact) via
// `scan_block_comment_line` instead.

/// (a) A line CLOSING a multi-line `/* ... */` block comment mid-line, with a
/// LIVE `t('missing.key')` call after the `*/`, must still emit
/// `missing-i18n-key`.
///
/// Fails before this fix: `scan_i18n`'s whole-line skip only looks at the
/// ENTERING state (`true` for this line, because the comment opened on line 1
/// and is still open when line 3 begins) and discards the line in its
/// ENTIRETY via `continue` — never reaching `extract_translation_keys` for
/// the `t('missing.key')` call that follows `*/` on that same line. Verified
/// directly against this branch's base commit (`d9b0b174ea`, MLINT-C1, before
/// the MLINT-C2 fix below existed): `missing.is_empty()` — the finding never
/// fires and the missing key silently escapes the gate.
#[test]
fn scan_i18n_detects_missing_key_after_block_comment_closes_mid_line() {
    let dir = tempfile::tempdir().unwrap();
    let locale_dir = dir
        .path()
        .join("crates/maekon-web/frontend/src/i18n/locales");
    std::fs::create_dir_all(&locale_dir).unwrap();
    for locale in ["en", "ko", "ja", "zh-CN", "es"] {
        let mut file = std::fs::File::create(locale_dir.join(format!("{locale}.json"))).unwrap();
        write!(file, "{{}}").unwrap();
    }

    let product_path = dir
        .path()
        .join("crates/maekon-web/frontend/src/pages/example/ClosingLineLiveCode.tsx");
    std::fs::create_dir_all(product_path.parent().unwrap()).unwrap();
    let mut product = std::fs::File::create(&product_path).unwrap();
    writeln!(product, "/* old usage").unwrap();
    writeln!(product, " * see the example above").unwrap();
    writeln!(product, " */ const label = t('missing.key');").unwrap();

    let findings = scan_i18n(dir.path(), &[]);
    let missing: Vec<_> = findings
        .iter()
        .filter(|f| f.category == "missing-i18n-key")
        .collect();

    assert_eq!(
        missing.len(),
        1,
        "the live t() call after the closing */ on line 3 must still be \
         extracted and validated: {missing:?}"
    );
    assert!(missing[0].message.contains("missing.key"));
    assert_eq!(missing[0].line, 3);
}

/// (b) A `t('x')` call on a line FULLY inside a multi-line block comment (no
/// closing `*/` on that line) must still NOT emit `missing-i18n-key` — the
/// MLINT-C1 whole-line-skip guarantee must be preserved by MLINT-C2's masking
/// approach (a fully-masked line yields zero findings from every scanner).
#[test]
fn scan_i18n_still_skips_key_fully_inside_multiline_block_comment() {
    let dir = tempfile::tempdir().unwrap();
    let locale_dir = dir
        .path()
        .join("crates/maekon-web/frontend/src/i18n/locales");
    std::fs::create_dir_all(&locale_dir).unwrap();
    for locale in ["en", "ko", "ja", "zh-CN", "es"] {
        let mut file = std::fs::File::create(locale_dir.join(format!("{locale}.json"))).unwrap();
        write!(file, "{{}}").unwrap();
    }

    let product_path = dir
        .path()
        .join("crates/maekon-web/frontend/src/pages/example/FullyInsideComment.tsx");
    std::fs::create_dir_all(product_path.parent().unwrap()).unwrap();
    let mut product = std::fs::File::create(&product_path).unwrap();
    writeln!(product, "/*").unwrap();
    writeln!(product, " * example: t('doc.fully.inside.key')").unwrap();
    writeln!(product, " */").unwrap();

    let findings = scan_i18n(dir.path(), &[]);
    let missing: Vec<_> = findings
        .iter()
        .filter(|f| f.category == "missing-i18n-key")
        .collect();

    assert!(
        missing.is_empty(),
        "a t() call on a line fully inside a block comment must not be extracted: {missing:?}"
    );
}

/// (c) A line with LIVE code, then a comment that both OPENS and CLOSES on
/// the SAME line (`const l = t('x'); /* trailing */`), must still have the
/// live `t('x')` call detected — no false-negative regression from the new
/// masked-line scan.
///
/// This shape already worked correctly before MLINT-C2 for a different
/// reason: `entering` is `false` here (no PRECEDING line left the comment
/// open), so MLINT-C1's whole-line skip never triggered, and
/// `extract_translation_keys` has its own independent per-line
/// block-comment/quote tracking that already stops at `/*` correctly. This
/// test guards the MLINT-C2 refactor — which now ALWAYS routes through
/// `scan_block_comment_line`'s masked output rather than the raw line — from
/// silently breaking that already-working case.
#[test]
fn scan_i18n_still_detects_live_key_before_inline_trailing_comment() {
    let dir = tempfile::tempdir().unwrap();
    let locale_dir = dir
        .path()
        .join("crates/maekon-web/frontend/src/i18n/locales");
    std::fs::create_dir_all(&locale_dir).unwrap();
    for locale in ["en", "ko", "ja", "zh-CN", "es"] {
        let mut file = std::fs::File::create(locale_dir.join(format!("{locale}.json"))).unwrap();
        write!(file, "{{}}").unwrap();
    }

    let product_path = dir
        .path()
        .join("crates/maekon-web/frontend/src/pages/example/LiveThenTrailingComment.tsx");
    std::fs::create_dir_all(product_path.parent().unwrap()).unwrap();
    let mut product = std::fs::File::create(&product_path).unwrap();
    writeln!(
        product,
        "const l = t('missing.trailing.key'); /* trailing */"
    )
    .unwrap();

    let findings = scan_i18n(dir.path(), &[]);
    let missing: Vec<_> = findings
        .iter()
        .filter(|f| f.category == "missing-i18n-key")
        .collect();

    assert_eq!(
        missing.len(),
        1,
        "the live t() call before the trailing inline comment must still be detected: {missing:?}"
    );
    assert!(missing[0].message.contains("missing.trailing.key"));
}

/// (d) A `/*` embedded inside a runtime string literal, on a line that ALSO
/// carries a real `t()` call after the string, must NOT be treated as a
/// comment opener — the live `t()` call after the string must still be
/// detected, and the file-level `in_block_comment` state must NOT get stuck
/// "inside a comment" for subsequent lines.
#[test]
fn scan_i18n_ignores_slash_star_inside_string_literal() {
    let dir = tempfile::tempdir().unwrap();
    let locale_dir = dir
        .path()
        .join("crates/maekon-web/frontend/src/i18n/locales");
    std::fs::create_dir_all(&locale_dir).unwrap();
    for locale in ["en", "ko", "ja", "zh-CN", "es"] {
        let mut file = std::fs::File::create(locale_dir.join(format!("{locale}.json"))).unwrap();
        write!(file, "{{}}").unwrap();
    }

    let product_path = dir
        .path()
        .join("crates/maekon-web/frontend/src/pages/example/SlashStarInString.tsx");
    std::fs::create_dir_all(product_path.parent().unwrap()).unwrap();
    let mut product = std::fs::File::create(&product_path).unwrap();
    writeln!(
        product,
        r#"const pattern = "a /* not a comment */ b"; const l = t('missing.after.string.key');"#
    )
    .unwrap();
    writeln!(product, "const l2 = t('missing.next.line.key');").unwrap();

    let findings = scan_i18n(dir.path(), &[]);
    let missing: Vec<_> = findings
        .iter()
        .filter(|f| f.category == "missing-i18n-key")
        .collect();

    assert_eq!(
        missing.len(),
        2,
        "both t() calls must be detected — the /* inside the string must not \
         open a comment, and state must not leak into the next line: {missing:?}"
    );
    assert!(missing
        .iter()
        .any(|f| f.message.contains("missing.after.string.key")));
    assert!(missing
        .iter()
        .any(|f| f.message.contains("missing.next.line.key")));
}
