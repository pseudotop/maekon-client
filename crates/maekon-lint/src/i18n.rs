use crate::finding::{Finding, Severity};
use crate::fs_scan::{collect_files, is_ignored};
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

/// ICU/i18next recognized plural suffix set.  A locale key `{base}_{suffix}`
/// is accepted as a valid plural form of the base key `{base}`.
const PLURAL_SUFFIXES: &[&str] = &[
    "_one", "_other", "_zero", "_two", "_few", "_many", "_plural",
];

pub(crate) fn scan_i18n(repo_root: &Path, ignore_paths: &[String]) -> Vec<Finding> {
    let mut findings = Vec::new();
    let locale_root = repo_root.join(FRONTEND_LOCALES_DIR);

    let mut locale_keys: HashMap<String, BTreeSet<String>> = HashMap::new();
    // Fix #9: track whether en.json (the baseline) failed to load.
    let mut en_load_failed = false;

    for locale in SUPPORTED_LOCALES {
        let locale_path = locale_root.join(format!("{locale}.json"));
        match load_locale_keys(&locale_path) {
            Ok(keys) => {
                locale_keys.insert(locale.to_string(), keys);
            }
            Err(err) => {
                if locale == "en" {
                    en_load_failed = true;
                }
                findings.push(Finding::new(
                    Severity::Error,
                    "locale-load",
                    locale_path.clone(),
                    1,
                    1,
                    err,
                    String::new(),
                ));
            }
        }
    }

    // Fix #9: if the baseline en.json failed to load, proceeding would produce
    // thousands of spurious missing-key and extra-key errors (because en_keys
    // would be empty) that completely bury the real root cause.  Emit one clear
    // fatal error and abort; the caller must fix the load error first.
    if en_load_failed {
        findings.push(Finding::new(
            Severity::Error,
            "baseline-locale-missing",
            locale_root.join("en.json"),
            1,
            1,
            "FATAL: baseline en.json failed to load — aborting key-presence \
             validation to avoid thousands of spurious errors. Fix the locale-load \
             error above first."
                .to_string(),
            String::new(),
        ));
        return findings;
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

        let Ok(content) = fs::read_to_string(&file) else {
            continue;
        };

        // File-level opt-out from the hardcoded-UI-copy check for components whose
        // text is not user-facing product copy (e.g. a dev-only debug toolbar).
        // missing-key / template-literal validation still applies.
        let allow_hardcoded = content.contains("lint:allow-hardcoded-ui");

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

            // Fix #15: emit Warning-severity notices for template-literal t()
            // call sites so they are visible rather than silently skipped.
            for (column, prefix, full_call) in extract_template_literal_t_calls(line) {
                findings.push(Finding::new(
                    Severity::Warning,
                    "template-literal-i18n-key",
                    file.clone(),
                    line_idx + 1,
                    column,
                    format!(
                        "Template-literal i18n key — static prefix \"{prefix}\" \
                         cannot be fully validated ({full_call})"
                    ),
                    line.trim().to_string(),
                ));
            }

            if file.extension().and_then(|ext| ext.to_str()) == Some("tsx")
                && !is_hardcoded_ui_copy_fixture(&file)
                && !allow_hardcoded
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

/// Returns `true` if `key` has a translation in `keys`.
///
/// The following forms are accepted beyond a direct match:
///
/// - **Recognized ICU/i18next plural suffixes** (`_one`, `_other`, `_zero`,
///   `_two`, `_few`, `_many`, `_plural`): checked explicitly via `PLURAL_SUFFIXES`
///   so all recognized forms are enumerated in one place.
///
/// - **i18next context forms** (`{key}_{contextValue}`): i18next resolves
///   `t('key', { context: 'value' })` to the locale entry `key_value`.  Because
///   context values are arbitrary runtime strings they cannot be enumerated
///   statically, so a range scan is used as a final fallback.  This is the
///   intended behavior — see the `i18n_key_lookup_accepts_context_resource_forms`
///   test for the canonical example.
///
/// Fix #11: the previous code had redundant explicit checks for `_one` and
/// `_other` (already covered by the range scan) and the range scan comment did
/// not document why it is correct to retain it.  Those redundant checks are now
/// replaced by the `PLURAL_SUFFIXES` table.  The range scan is retained and
/// documented as the context-form fallback.
pub(crate) fn has_translation_key(keys: &BTreeSet<String>, key: &str) -> bool {
    // Direct match.
    if keys.contains(key) {
        return true;
    }
    // Recognized plural suffixes (ICU / i18next).
    for suffix in PLURAL_SUFFIXES {
        if keys.contains(&format!("{key}{suffix}")) {
            return true;
        }
    }
    // i18next context-form fallback: `{key}_{anyContextValue}` satisfies `key`.
    let prefix_str = format!("{key}_");
    keys.range(prefix_str.clone()..)
        .next()
        .is_some_and(|candidate| candidate.starts_with(&prefix_str))
}

/// Flatten a JSON value into dot-separated key paths, inserting the result into
/// `keys`.
///
/// Fix #8: JSON arrays now produce BOTH the bare key AND per-element keys
/// (`{prefix}[0]`, `{prefix}[1]`, …) instead of a single opaque leaf.
/// A locale entry like `heatmap.days` (7 weekday strings) produces `heatmap.days`
/// (so a `t('heatmap.days', { returnObjects: true })` usage that retrieves the
/// whole array still resolves) PLUS `heatmap.days[0]`…`[6]` (so cross-locale
/// length and per-element coverage can be validated). Keeping the bare key avoids
/// a false-positive missing-key error on the valid returnObjects pattern; the
/// per-element keys still catch a locale whose array is empty/short relative to
/// the baseline (its `[i]` keys are missing).
pub(crate) fn flatten_json_keys(prefix: &str, value: &Value, keys: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (key, nested) in map {
                let full_key = if prefix.is_empty() {
                    key.to_string()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten_json_keys(&full_key, nested, keys);
            }
        }
        Value::Array(elements) => {
            // Bare key: supports `t(key, { returnObjects: true })` array retrieval.
            if !prefix.is_empty() {
                keys.insert(prefix.to_string());
            }
            // Per-element keys: enable cross-locale length / per-element coverage.
            for (idx, elem) in elements.iter().enumerate() {
                let indexed_key = format!("{prefix}[{idx}]");
                flatten_json_keys(&indexed_key, elem, keys);
            }
        }
        _ => {
            // Scalar (string/number/bool/null) — insert the accumulated key.
            if !prefix.is_empty() {
                keys.insert(prefix.to_string());
            }
        }
    }
}

/// Extract static string keys from `t("key")` / `t('key')` call sites on one
/// source line.
///
/// Template-literal calls (`t(\`...\`)`) are intentionally skipped here — they
/// are handled separately by `extract_template_literal_t_calls` so they are
/// visible to the validator rather than silently ignored.
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

/// Fix #15: locate `t(\`...\`)` template-literal call sites on a single source
/// line and return `(column, static_prefix, display_snippet)` for each.
///
/// The static prefix is the portion of the key before the first `${`
/// interpolation — this is the only part that can be statically validated.
/// Callers emit this at `Warning` severity so it is visible without failing CI.
///
/// Fully-static template literals (e.g. `t(\`common.save\`)`) are also
/// captured, because `extract_translation_keys` only scans single/double-quote
/// strings and would miss them entirely.
pub(crate) fn extract_template_literal_t_calls(line: &str) -> Vec<(usize, String, String)> {
    let mut results = Vec::new();
    let mut search_from = 0usize;

    while search_from < line.len() {
        let Some(rel_pos) = line[search_from..].find("t(") else {
            break;
        };
        let pos = search_from + rel_pos;

        // Reject alphanumeric / underscore prefix (not a standalone t() call).
        if pos > 0 {
            let prev = line[..pos].chars().next_back().unwrap_or(' ');
            if prev.is_ascii_alphanumeric() || prev == '_' {
                search_from = pos + 2;
                continue;
            }
        }

        // Skip whitespace after "t(".
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

        // Must open with a backtick — otherwise not a template literal.
        if line.as_bytes().get(idx) != Some(&b'`') {
            search_from = pos + 2;
            continue;
        }
        idx += 1; // skip opening backtick

        // Collect everything up to the closing backtick (same line only).
        let content_start = idx;
        let mut found_close = false;
        while idx < line.len() {
            if line.as_bytes().get(idx) == Some(&b'`') {
                found_close = true;
                break;
            }
            // Advance by one char (UTF-8 safe).
            let ch = line[idx..].chars().next().unwrap_or(' ');
            idx += ch.len_utf8();
        }

        if !found_close {
            // Unclosed backtick on this line — skip.
            search_from = pos + 2;
            continue;
        }

        let content = &line[content_start..idx];
        let closing_pos = idx;

        // Extract the static prefix — everything before the first `${`.
        let static_prefix = match content.find("${") {
            Some(interp_pos) => content[..interp_pos].to_string(),
            None => content.to_string(), // fully static template literal
        };

        let full_call = format!("t(`{content}`)");
        let column = pos + 1; // 1-based column
        results.push((column, static_prefix, full_call));

        search_from = closing_pos + 1;
    }

    results
}

/// Detect hardcoded human-readable text in JSX/TSX attribute values and text
/// nodes on a single source line.
///
/// Returns `(column, message)` pairs for each violation found.
///
/// Fix #10: the JSX text-node check previously used `!starts_with('{') &&
/// !ends_with('}')` as a heuristic to skip JSX expression content.  This
/// silently passed mixed segments like `Save {count}` or `{count} items`
/// because those strings touch a brace at one end.
///
/// The fix strips `{...}` expression spans from the segment first (via
/// `strip_jsx_expressions`) and then tests whether the *remaining* static text
/// is non-trivial hardcoded copy.  Purely dynamic segments (`{expr}`) produce
/// an empty string after stripping and are still skipped correctly.
/// A placeholder value that is a format/example hint rather than translatable copy.
/// Real placeholder copy ("Search messages...", "Name") has internal whitespace or
/// is capitalized prose; format hints are single tokens that are all-lowercase
/// (`git`) or contain a non-letter (`HH:MM`, `sk-...`, `pol-git-status`).
fn is_format_hint(value: &str) -> bool {
    let v = value.trim();
    if v.is_empty() || v.chars().any(char::is_whitespace) {
        return false;
    }
    let all_lower = v
        .chars()
        .filter(|c| c.is_alphabetic())
        .all(|c| c.is_lowercase());
    let has_non_letter = v.chars().any(|c| !c.is_alphabetic());
    all_lower || has_non_letter
}

pub(crate) fn detect_hardcoded_ui_literals(line: &str) -> Vec<(usize, String)> {
    let mut hits = Vec::new();

    // #6424: comment lines are not JSX — skip them BEFORE any scan so a commented-out
    // line carrying `title="..."` etc. (or prose between `>`/`<`) is never flagged as
    // hardcoded UI copy. Previously this guard sat AFTER the attribute scan below, so a
    // commented-out attribute produced a spurious --strict-i18n build failure.
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with('*') || trimmed.starts_with("/*") {
        return hits;
    }

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
            // A `placeholder` is frequently a format/example hint (`HH:MM`, `git`,
            // `sk-...`, `pol-git-status`) rather than translatable copy — skip those.
            if contains_human_text(value) && !(attr == "placeholder" && is_format_hint(value)) {
                hits.push((
                    pos + 1,
                    format!("Hardcoded UI attribute `{attr}` should use i18n"),
                ));
            }
            search_from = value_end + 1;
        }
    }

    let chars: Vec<char> = line.chars().collect();
    let mut segment_start = 0usize;
    while let Some(gt_rel) = line[segment_start..].find('>') {
        let gt = segment_start + gt_rel;
        let Some(lt_rel) = line[gt + 1..].find('<') else {
            break;
        };
        let lt = gt + 1 + lt_rel;

        // Only treat `>...<` as a JSX text node when the `>` actually closes a tag
        // and the `<` opens one — not when they are comparison/arrow/generic
        // operators (`a >= 0 && b < 7`, `() => Promise<void>`, `Array<T>`), which
        // previously produced false positives on the code between them.
        let gt_idx = line[..gt].chars().count();
        let before_gt = if gt_idx > 0 { chars[gt_idx - 1] } else { ' ' };
        let after_gt = chars.get(gt_idx + 1).copied().unwrap_or(' ');
        let lt_idx = line[..lt].chars().count();
        let after_lt = chars.get(lt_idx + 1).copied().unwrap_or(' ');
        let gt_is_operator = matches!(before_gt, '=' | '<' | '-' | '!' | '>') || after_gt == '=';
        let lt_opens_tag = after_lt.is_ascii_alphabetic() || after_lt == '/' || after_lt == '>';
        if gt_is_operator || !lt_opens_tag {
            segment_start = lt; // re-anchor at this `<` and keep scanning
            continue;
        }

        // Skip the textual content of tags whose content is not translatable copy:
        // code snippets, keyboard keys, preformatted text, <option> values (often
        // language names / enum values), and script/style.
        if matches!(
            enclosing_open_tag_name(line, gt).as_deref(),
            Some("code")
                | Some("kbd")
                | Some("pre")
                | Some("option")
                | Some("script")
                | Some("style")
        ) {
            segment_start = lt + 1;
            continue;
        }

        let segment = line[gt + 1..lt].trim();
        // Fix #10: strip JSX expression spans before testing for hardcoded copy, so
        // mixed content like "Save {count}" is caught while purely dynamic `{expr}`
        // segments become empty and are skipped.
        let static_text = strip_jsx_expressions(segment);
        // Skip segments that are still code rather than prose (operators / statement
        // punctuation left over from a non-JSX `>...<` span).
        let alpha_count = static_text.chars().filter(|c| c.is_alphabetic()).count();
        if static_text.contains("&&")
            || static_text.contains("||")
            || static_text.contains("=>")
            || static_text.contains(';')
            // Function-signature / generic leftovers, e.g. a `>...<` span that
            // straddled `invoke<T>(cmd: string, ...): Promise<T>` — the param list
            // (`(cmd: string`) or the return type (`): Promise`) has a `:` next to a
            // paren. No UI text node does.
            || (static_text.contains(':') && (static_text.contains('(') || static_text.contains(')')))
            // Units / short non-prose tokens left over from spans like `+{x}ms` or
            // `<T>` — two or fewer letters is never translatable copy.
            || alpha_count <= 2
        {
            segment_start = lt + 1;
            continue;
        }
        if contains_human_text(&static_text) {
            hits.push((gt + 2, "Hardcoded UI text node should use i18n".to_string()));
        }
        segment_start = lt + 1;
    }

    hits
}

/// Name (lowercased) of the JSX tag whose opening `>` is at byte index `gt`, if the
/// nearest preceding `<` introduces an identifier (e.g. `<code className=...>` →
/// `code`). Used to skip the content of non-translatable tags.
fn enclosing_open_tag_name(line: &str, gt: usize) -> Option<String> {
    let open = line[..gt].rfind('<')?;
    let name: String = line[open + 1..gt]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    if name.is_empty() {
        None
    } else {
        Some(name.to_ascii_lowercase())
    }
}

/// Remove `{...}` JSX expression spans from a text segment and return the
/// remaining static text, with leading/trailing whitespace trimmed and
/// runs of interior whitespace collapsed to a single space.
///
/// Nested braces (e.g. `{fn({ key: val })}`) are handled by tracking depth so
/// that only the outermost span is stripped.
pub(crate) fn strip_jsx_expressions(segment: &str) -> String {
    let mut result = String::with_capacity(segment.len());
    let mut depth: u32 = 0;
    for ch in segment.chars() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                // Do not emit the closing brace itself.
            }
            _ => {
                if depth == 0 {
                    result.push(ch);
                }
            }
        }
    }
    // Collapse whitespace so "Save  " (trailing space left by stripped `{count}`)
    // is treated as "Save" and still triggers contains_human_text.
    result.split_whitespace().collect::<Vec<_>>().join(" ")
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
