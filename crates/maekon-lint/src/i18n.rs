use crate::finding::{Finding, Severity};
use crate::fs_scan::{collect_files, is_ignored, is_locale_file};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::Path;

const FRONTEND_SRC_DIR: &str = "crates/maekon-web/frontend/src";
const FRONTEND_LOCALES_DIR: &str = "crates/maekon-web/frontend/src/i18n/locales";
pub(crate) const SUPPORTED_LOCALES: [&str; 5] = ["en", "ko", "ja", "zh-CN", "es"];
const UI_ATTRS: [&str; 7] = [
    "placeholder",
    "title",
    "aria-label",
    "label",
    "helperText",
    "alt",
    "tooltip",
];

pub(crate) fn scan_i18n(repo_root: &Path, ignore_paths: &[String]) -> Vec<Finding> {
    let mut findings = Vec::new();
    let locale_root = repo_root.join(FRONTEND_LOCALES_DIR);

    let mut locale_keys: HashMap<String, BTreeSet<String>> = HashMap::new();
    for locale in SUPPORTED_LOCALES {
        let locale_path = locale_root.join(format!("{locale}.json"));
        match load_locale_keys(&locale_path) {
            Ok(keys) => {
                locale_keys.insert(locale.to_string(), keys);
            }
            Err(err) => findings.push(Finding::new(
                Severity::Error,
                "locale-load",
                locale_path.clone(),
                1,
                1,
                err,
                String::new(),
            )),
        }
    }

    let en_keys = locale_keys.get("en").cloned().unwrap_or_default();
    for locale in SUPPORTED_LOCALES {
        if locale == "en" {
            continue;
        }

        let Some(keys) = locale_keys.get(locale) else {
            continue;
        };

        for missing in en_keys.difference(keys) {
            findings.push(Finding::new(
                Severity::Error,
                "missing-locale-key",
                locale_root.join(format!("{locale}.json")),
                1,
                1,
                format!("Missing translation key in {locale}: {missing}"),
                String::new(),
            ));
        }

        for extra in keys.difference(&en_keys) {
            findings.push(Finding::new(
                Severity::Warning,
                "extra-locale-key",
                locale_root.join(format!("{locale}.json")),
                1,
                1,
                format!("Extra key present only in {locale}: {extra}"),
                String::new(),
            ));
        }
    }

    let frontend_src = repo_root.join(FRONTEND_SRC_DIR);
    let files = collect_files(&frontend_src, &["ts", "tsx"]);
    for file in files {
        if is_ignored(&file, ignore_paths) {
            continue;
        }
        if is_locale_file(&file) {
            continue;
        }

        let Ok(content) = fs::read_to_string(&file) else {
            continue;
        };

        for (line_idx, line) in content.lines().enumerate() {
            for key in extract_translation_keys(line) {
                if !has_translation_key(&en_keys, &key) {
                    findings.push(Finding::new(
                        Severity::Error,
                        "missing-i18n-key",
                        file.clone(),
                        line_idx + 1,
                        1,
                        format!("Unknown i18n key used: {key}"),
                        line.trim().to_string(),
                    ));
                }
            }

            if file.extension().and_then(|ext| ext.to_str()) == Some("tsx")
                && !is_hardcoded_ui_copy_fixture(&file)
            {
                for (column, message) in detect_hardcoded_ui_literals(line) {
                    findings.push(Finding::new(
                        Severity::Warning,
                        "hardcoded-ui-copy",
                        file.clone(),
                        line_idx + 1,
                        column,
                        message,
                        line.trim().to_string(),
                    ));
                }
            }
        }
    }

    findings
}

fn is_hardcoded_ui_copy_fixture(path: &Path) -> bool {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if file_name.ends_with(".stories.tsx")
        || file_name.ends_with(".story.tsx")
        || file_name.ends_with(".test.tsx")
        || file_name.ends_with(".spec.tsx")
    {
        return true;
    }

    let mut previous_was_src = false;
    for component in path.components() {
        let Some(name) = component.as_os_str().to_str() else {
            previous_was_src = false;
            continue;
        };

        if name == "__tests__" {
            return true;
        }
        if previous_was_src && name == "stories" {
            return true;
        }
        previous_was_src = name == "src";
    }

    false
}

pub(crate) fn load_locale_keys(path: &Path) -> Result<BTreeSet<String>, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read locale file {}: {e}", path.display()))?;
    let value: Value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse locale JSON {}: {e}", path.display()))?;

    let mut keys = BTreeSet::new();
    flatten_json_keys("", &value, &mut keys);
    Ok(keys)
}

pub(crate) fn has_translation_key(keys: &BTreeSet<String>, key: &str) -> bool {
    keys.contains(key)
        || keys.contains(&format!("{key}_one"))
        || keys.contains(&format!("{key}_other"))
        || keys
            .range(format!("{key}_")..)
            .next()
            .is_some_and(|candidate| candidate.starts_with(&format!("{key}_")))
}

pub(crate) fn flatten_json_keys(prefix: &str, value: &Value, keys: &mut BTreeSet<String>) {
    if let Value::Object(map) = value {
        for (key, nested) in map {
            let full_key = if prefix.is_empty() {
                key.to_string()
            } else {
                format!("{prefix}.{key}")
            };

            match nested {
                Value::Object(_) => flatten_json_keys(&full_key, nested, keys),
                _ => {
                    keys.insert(full_key);
                }
            }
        }
    }
}

pub(crate) fn extract_translation_keys(line: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut search_from = 0usize;

    while search_from < line.len() {
        let Some(rel_pos) = line[search_from..].find("t(") else {
            break;
        };
        let pos = search_from + rel_pos;

        if pos > 0 {
            let prev = line[..pos].chars().next_back().unwrap_or(' ');
            if prev.is_ascii_alphanumeric() || prev == '_' {
                search_from = pos + 2;
                continue;
            }
        }

        let mut idx = pos + 2;
        while idx < line.len() {
            let ch = line[idx..].chars().next().unwrap_or(' ');
            if ch.is_whitespace() {
                idx += ch.len_utf8();
            } else {
                break;
            }
        }

        if idx >= line.len() {
            break;
        }

        let quote = line[idx..].chars().next().unwrap_or(' ');
        if quote != '"' && quote != '\'' {
            search_from = pos + 2;
            continue;
        }

        idx += quote.len_utf8();
        let start = idx;
        let mut escaped = false;

        while idx < line.len() {
            let ch = line[idx..].chars().next().unwrap_or(' ');
            if escaped {
                escaped = false;
                idx += ch.len_utf8();
                continue;
            }
            if ch == '\\' {
                escaped = true;
                idx += ch.len_utf8();
                continue;
            }
            if ch == quote {
                keys.push(line[start..idx].to_string());
                idx += ch.len_utf8();
                break;
            }
            idx += ch.len_utf8();
        }

        search_from = idx;
    }

    keys
}

pub(crate) fn detect_hardcoded_ui_literals(line: &str) -> Vec<(usize, String)> {
    let mut hits = Vec::new();

    for attr in UI_ATTRS {
        let marker = format!("{attr}=\"");
        let mut search_from = 0usize;
        while let Some(rel_pos) = line[search_from..].find(&marker) {
            let pos = search_from + rel_pos;
            if pos > 0 {
                let prev = line[..pos].chars().next_back().unwrap_or(' ');
                if prev.is_ascii_alphanumeric() || prev == '_' || prev == '-' {
                    search_from = pos + marker.len();
                    continue;
                }
            }
            let value_start = pos + marker.len();
            let Some(value_end_rel) = line[value_start..].find('"') else {
                break;
            };
            let value_end = value_start + value_end_rel;
            let value = &line[value_start..value_end];
            if contains_human_text(value) {
                hits.push((
                    pos + 1,
                    format!("Hardcoded UI attribute `{attr}` should use i18n"),
                ));
            }
            search_from = value_end + 1;
        }
    }

    let mut segment_start = 0usize;
    while let Some(gt_rel) = line[segment_start..].find('>') {
        let gt = segment_start + gt_rel;
        let Some(lt_rel) = line[gt + 1..].find('<') else {
            break;
        };
        let lt = gt + 1 + lt_rel;
        let segment = line[gt + 1..lt].trim();
        if !segment.starts_with('{') && !segment.ends_with('}') && contains_human_text(segment) {
            hits.push((gt + 2, "Hardcoded UI text node should use i18n".to_string()));
        }
        segment_start = lt + 1;
    }

    hits
}

pub(crate) fn contains_human_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.len() < 2 {
        return false;
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") || trimmed.starts_with('/')
    {
        return false;
    }

    let has_letter = trimmed.chars().any(|c| c.is_alphabetic());
    if !has_letter {
        return false;
    }

    if trimmed.len() <= 4 && trimmed.chars().all(|c| c.is_ascii_uppercase()) {
        return false;
    }

    true
}
