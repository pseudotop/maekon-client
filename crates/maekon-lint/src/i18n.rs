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

        // MLINT-C1: cross-line block-comment state, threaded across the whole file
        // (one bool per file, advanced one line at a time). Without this, a
        // continuation line inside a multi-line `/* ... */` or `/** ... */` block
        // that does not itself start with a comment marker (`//`, `*`, `/*`) is
        // parsed as LIVE code by every per-line scanner below — see
        // `scan_block_comment_line` for the state machine.
        let mut in_block_comment = false;

        for (line_idx, line) in content.lines().enumerate() {
            // MLINT-C2 (PR #7577 re-review follow-up): scan the MASKED line —
            // every block-comment byte span (delimiters included) replaced with
            // equal-byte-width spaces — instead of whole-line-skipping every
            // line that STARTED inside a block comment. MLINT-C1's original
            // `if advance_block_comment_state(...) { continue; }` over-corrected:
            // a line that CLOSES a multi-line block comment mid-line and then
            // carries LIVE code after `*/` (e.g. `*/ const label =
            // t('missing.key');`) was discarded in its ENTIRETY, so a genuinely
            // missing i18n key or a hardcoded UI literal after the close
            // silently escaped the gate. Masking preserves `line`'s original
            // byte length (so `column` stays accurate) while only blanking the
            // parts that are actually inside `/* ... */`; a line fully inside a
            // block comment naturally masks to all spaces and therefore still
            // yields zero findings, so MLINT-C1's "fully skip" behavior for
            // that case is preserved without a special-cased early `continue`.
            let (_, masked_line) = scan_block_comment_line(line, &mut in_block_comment);

            for key in extract_translation_keys(&masked_line) {
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
            for (column, prefix, full_call) in extract_template_literal_t_calls(&masked_line) {
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
                for (column, message) in detect_hardcoded_ui_literals(&masked_line) {
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

/// Advances the per-file `in_block_comment` cross-line state past `line` and
/// returns `(entering, masked_line)`.
///
/// `entering` is the ENTERING state (whether `line` started already inside a
/// block comment). A block comment is entered when a line opens `/*` (or
/// `/**`) without a matching `*/` later on the SAME line, and the state
/// remains "inside" through every subsequent line up to and including the
/// line bearing the closing `*/`.
///
/// `masked_line` is `line` with every block-comment byte span — the `/*`/`*/`
/// delimiters themselves plus everything between them — replaced by ASCII
/// spaces of the SAME byte width as the character(s) they replace. This makes
/// `masked_line` byte-length-IDENTICAL to `line`, so any column/byte offset a
/// scanner (`extract_translation_keys`, `extract_template_literal_t_calls`,
/// `detect_hardcoded_ui_literals`) computes against `masked_line` is directly
/// comparable to the equivalent offset against `line`. Live code — including
/// live code BEFORE an opening `/*` on the same line, and live code AFTER a
/// closing `*/` on the same line — is copied through unmasked.
///
/// MLINT-C2 (PR #7577 re-review follow-up): this replaces the previous
/// whole-line skip that `scan_i18n` applied whenever `entering == true`. That
/// approach over-corrected — a line CLOSING a multi-line block comment
/// mid-line and then carrying LIVE code after `*/` was discarded in its
/// ENTIRETY, so a genuinely missing i18n key or hardcoded UI literal after the
/// close silently escaped validation (a false negative that defeated the
/// gate's purpose). A line fully inside a block comment naturally masks to
/// all spaces, which still yields zero findings from every scanner, so the
/// original MLINT-C1 "fully skip" behavior is preserved for that case without
/// a special-cased early `continue`.
///
/// A `//` line comment masks nothing past its `//` marker — the remainder is
/// copied through verbatim — because `extract_translation_keys` and
/// `extract_template_literal_t_calls` each independently stop scanning at
/// `//` via their own per-line logic (matching pre-existing, out-of-scope
/// behavior for line comments; only block-comment masking is this function's
/// job).
///
/// String/template-literal delimiters (`"`, `'`, `` ` ``) are tracked while
/// NOT inside a block comment so a `/*` embedded in a runtime string is never
/// mistaken for a comment opener. Quote state does not itself persist across
/// lines (TS/TSX single/double-quoted strings cannot span lines; a multi-line
/// template literal is a pre-existing, out-of-scope approximation shared with
/// `extract_translation_keys`'s own single-line quote tracking).
///
/// Mirrors the sibling cross-line state machine in
/// `non_english::first_non_english_in_comment_with_state` (MLINT-C1: the i18n
/// scanners lacked the equivalent cross-line comment-state extension that
/// `non_english.rs` already has).
fn scan_block_comment_line(line: &str, in_block_comment: &mut bool) -> (bool, String) {
    let entering_in_block_comment = *in_block_comment;
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let mut masked = String::with_capacity(line.len());
    let mut i = 0usize;
    let mut string_delim: Option<char> = None;
    let mut escaped = false;

    while i < chars.len() {
        let (_, ch) = chars[i];

        if *in_block_comment {
            masked.push_str(&" ".repeat(ch.len_utf8()));
            if ch == '*' && chars.get(i + 1).is_some_and(|(_, next)| *next == '/') {
                masked.push(' '); // the closing '/' is always 1-byte ASCII
                *in_block_comment = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }

        if let Some(delim) = string_delim {
            masked.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == delim {
                string_delim = None;
            }
            i += 1;
            continue;
        }

        // Line comment: the rest of the line is a comment — nothing further to
        // track (no block-comment opener can meaningfully follow on this line).
        // Copied through unmasked: `extract_translation_keys` /
        // `extract_template_literal_t_calls` independently stop at `//`.
        if ch == '/' && chars.get(i + 1).is_some_and(|(_, next)| *next == '/') {
            let (byte_pos, _) = chars[i];
            masked.push_str(&line[byte_pos..]);
            break;
        }
        if ch == '/' && chars.get(i + 1).is_some_and(|(_, next)| *next == '*') {
            *in_block_comment = true;
            masked.push_str("  "); // '/' and '*' are both 1-byte ASCII
            i += 2;
            continue;
        }
        if matches!(ch, '"' | '\'' | '`') {
            string_delim = Some(ch);
            escaped = false;
            masked.push(ch);
            i += 1;
            continue;
        }

        masked.push(ch);
        i += 1;
    }

    (entering_in_block_comment, masked)
}

/// Thin backward-compatible wrapper preserving the pre-MLINT-C2 signature
/// (bare ENTERING boolean, no masked line) for direct unit-test callers.
#[cfg(test)]
fn advance_block_comment_state(line: &str, in_block_comment: &mut bool) -> bool {
    scan_block_comment_line(line, in_block_comment).0
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
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let mut i = 0usize;
    let mut string_delim: Option<char> = None;
    let mut escaped = false;
    let mut block_comment = false;

    while i < chars.len() {
        let ch = chars[i].1;

        if block_comment {
            if ch == '*' && chars.get(i + 1).is_some_and(|(_, next)| *next == '/') {
                block_comment = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }

        if let Some(delim) = string_delim {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == delim {
                string_delim = None;
            }
            i += 1;
            continue;
        }

        if ch == '/' && chars.get(i + 1).is_some_and(|(_, next)| *next == '/') {
            break;
        }
        if ch == '/' && chars.get(i + 1).is_some_and(|(_, next)| *next == '*') {
            block_comment = true;
            i += 2;
            continue;
        }
        if matches!(ch, '"' | '\'' | '`') {
            string_delim = Some(ch);
            escaped = false;
            i += 1;
            continue;
        }

        if ch == 't' && chars.get(i + 1).is_some_and(|(_, next)| *next == '(') {
            if i > 0 {
                let prev = chars[i - 1].1;
                if prev.is_ascii_alphanumeric() || prev == '_' {
                    i += 1;
                    continue;
                }
            }

            let mut j = i + 2;
            while j < chars.len() && chars[j].1.is_whitespace() {
                j += 1;
            }
            let Some((quote_byte, quote)) = chars.get(j).copied() else {
                break;
            };
            if quote != '"' && quote != '\'' {
                i += 1;
                continue;
            }

            let start = quote_byte + quote.len_utf8();
            j += 1;
            let mut key_escaped = false;
            while j < chars.len() {
                let (byte, current) = chars[j];
                if key_escaped {
                    key_escaped = false;
                    j += 1;
                    continue;
                }
                if current == '\\' {
                    key_escaped = true;
                    j += 1;
                    continue;
                }
                if current == quote {
                    keys.push(line[start..byte].to_string());
                    break;
                }
                j += 1;
            }
            i = j.saturating_add(1);
            continue;
        }

        i += 1;
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

#[cfg(test)]
mod tests {
    use super::{advance_block_comment_state, extract_translation_keys};

    #[test]
    fn extract_translation_keys_ignores_comments_and_string_literals() {
        assert!(extract_translation_keys("// t('missing.key')").is_empty());
        assert!(extract_translation_keys("const sample = \"t('missing.key')\";").is_empty());
        assert_eq!(
            extract_translation_keys("const label = t('real.key');"),
            vec!["real.key".to_string()]
        );
    }

    // ── MLINT-C1: cross-line block-comment state ─────────────────────────────

    /// A line that opens `/*` without closing it on the same line must report
    /// "not currently inside a comment" (nothing before it was skipped) while
    /// leaving `in_block_comment` set to `true` for the NEXT line.
    #[test]
    fn advance_block_comment_state_opens_unterminated_block() {
        let mut in_block_comment = false;
        let entering = advance_block_comment_state("/**", &mut in_block_comment);
        assert!(!entering, "line opening the comment is itself live code");
        assert!(
            in_block_comment,
            "state must carry over as 'inside' for the next line"
        );
    }

    /// A continuation line with NO comment-marker prefix (the exact MLINT-C1
    /// gap) is reported as "entering already inside a comment" so callers skip
    /// it, and state remains `true` because it does not close the block.
    #[test]
    fn advance_block_comment_state_continuation_line_without_prefix_stays_inside() {
        let mut in_block_comment = true;
        let entering =
            advance_block_comment_state("   <div title=\"Hello\">", &mut in_block_comment);
        assert!(entering, "continuation line must be reported as in-comment");
        assert!(in_block_comment, "block comment is still open");
    }

    /// The line bearing the closing `*/` is ALSO reported as "entering inside a
    /// comment" (so it is excluded like every other continuation line), and
    /// state flips to `false` for the line AFTER it.
    #[test]
    fn advance_block_comment_state_closing_line_still_skipped_then_exits() {
        let mut in_block_comment = true;
        let entering = advance_block_comment_state(" */", &mut in_block_comment);
        assert!(entering, "the closing line itself is still comment content");
        assert!(!in_block_comment, "state clears for subsequent lines");
    }

    /// A `/*` embedded inside a runtime string literal must NOT be mistaken
    /// for a comment opener (string-literal awareness, mirrors the existing
    /// quote tracking already used by `extract_translation_keys`).
    #[test]
    fn advance_block_comment_state_ignores_slash_star_inside_string() {
        let mut in_block_comment = false;
        let entering = advance_block_comment_state(
            "const pattern = \"a /* not a comment */ b\";",
            &mut in_block_comment,
        );
        assert!(!entering);
        assert!(
            !in_block_comment,
            "the /* and */ inside the string cancel out; the trailing `;` is live code, \
             so the line must not leave the scanner stuck inside a comment"
        );
    }

    /// A normal live line with no comment markers at all leaves state
    /// untouched (`false` in, `false` out).
    #[test]
    fn advance_block_comment_state_live_line_untouched() {
        let mut in_block_comment = false;
        let entering =
            advance_block_comment_state("const label = t('real.key');", &mut in_block_comment);
        assert!(!entering);
        assert!(!in_block_comment);
    }
}
