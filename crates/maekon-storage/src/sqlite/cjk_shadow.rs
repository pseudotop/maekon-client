//! CJK bigram shadow — Option F tokenization seam.
//!
//! Converts CJK character runs (Hangul syllables/jamo, Hiragana/Katakana, CJK Unified)
//! into overlapping 2-character bigrams joined by spaces, while passing non-CJK text
//! through unchanged. The resulting "shadow" text is indexed in FTS5 with the
//! `unicode61` tokenizer; BM25 ranking operates over bigram tokens.
//!
//! # NFKC
//!
//! This implementation does NOT apply NFKC normalization before bigram expansion.
//! The `unicode-normalization` crate is not in the `maekon-storage` dependency tree,
//! and adding it is explicitly prohibited (#5758 rules). Practical impact: fullwidth
//! ASCII and compatibility forms (e.g. U+FF41 FULLWIDTH LATIN SMALL LETTER A) are
//! treated as non-CJK and pass through as-is. The spike's NFKC step was minor for the
//! tested corpus (0 Recall@3 difference); the validated test-case expected values in
//! this file omit normalization and match the bigram logic alone.
//!
//! # Design rationale
//!
//! FTS5 `porter unicode61` (V11 schema) cannot segment CJK text because it relies on
//! whitespace boundaries. For a Japanese document "月次売上レポートの集計", no query
//! token can ever match because the entire string is a single whitespace-delimited
//! "word". The bigram shadow decomposes CJK runs into overlapping 2-grams so that any
//! 2-char substring query can match. Porter stemming is intentionally removed from the
//! V41 schema: applying porter to bigrams would corrupt the bigram structure (e.g.
//! "売上" → unpredictable stem). The unicode61 BM25 baseline from the spike validates
//! the same R@3=0.611 as lindera at zero native-dependency cost.

// CJK Unicode range check. Identical to the Python spike harness `is_cjk` function
// so that Rust and Python outputs are guaranteed identical.
#[inline]
fn is_cjk(c: char) -> bool {
    let cp = c as u32;
    matches!(cp,
        0xAC00..=0xD7AF   // Hangul syllables
        | 0x1100..=0x11FF // Hangul jamo
        | 0x3040..=0x30FF // Hiragana / Katakana
        | 0x4E00..=0x9FFF // CJK Unified Ideographs
        | 0x3400..=0x4DBF // CJK Ext-A
        | 0xF900..=0xFAFF // CJK Compatibility Ideographs
    )
}

/// Transform `text` into its CJK bigram shadow.
///
/// Algorithm (mirrors `cjk_bigram_shadow` in the Python spike harness):
/// 1. Scan characters left-to-right, accumulating contiguous CJK runs.
/// 2. On flush: a single-char CJK run emits the char itself; a longer run
///    emits overlapping bigrams, each separated by a space.
/// 3. Non-CJK characters are emitted literally.
/// 4. Consecutive space tokens are collapsed by splitting on whitespace and
///    re-joining with single spaces.
///
/// Examples (validated against Python harness output):
/// - `"月次売上レポートの集計"` → `"月次 次売 売上 上レ レポ ポー ート トの の集 集計"`
/// - `"월별 급여 지급"` → `"월별 급여 지급"` (space-separated ≤2-char runs pass through)
/// - `"fix: WebSocket 재연결"` → `"fix: WebSocket 재연 연결"`
/// - `""` → `""`
/// - `"月"` → `"月"`
pub(crate) fn cjk_bigram_shadow(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    // Pre-allocate a buffer somewhat larger than the input to hold bigrams + spaces.
    let mut buf = String::with_capacity(text.len() * 2);
    let mut run: Vec<char> = Vec::new();

    let flush = |run: &mut Vec<char>, buf: &mut String| {
        if run.is_empty() {
            return;
        }
        if run.len() == 1 {
            buf.push(run[0]);
        } else {
            for i in 0..run.len() - 1 {
                if !buf.is_empty() && !buf.ends_with(' ') {
                    buf.push(' ');
                }
                buf.push(run[i]);
                buf.push(run[i + 1]);
            }
        }
        run.clear();
    };

    for ch in text.chars() {
        if is_cjk(ch) {
            run.push(ch);
        } else {
            flush(&mut run, &mut buf);
            buf.push(ch);
        }
    }
    flush(&mut run, &mut buf);

    // Collapse runs of whitespace to single spaces and trim, matching Python's
    // `" ".join(...split())` idiom.
    buf.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Build an FTS5 MATCH expression for `user_query` using bigram-shadow tokens.
///
/// Steps:
/// 1. Apply `cjk_bigram_shadow` to `user_query`.
/// 2. Split on whitespace.
/// 3. Strip any literal `"` characters from tokens (FTS5 syntax chars would
///    cause a parse error if left unescaped).
/// 4. Drop empty tokens.
/// 5. Wrap each token in FTS5 phrase quotes: `"tok"`.
/// 6. Join with ` OR `.
///
/// Returns `None` when no usable tokens remain (caller should return empty results).
///
/// # Why quoted OR rather than AND
///
/// FTS5 implicit AND collapses recall catastrophically on bigram queries: a 10-bigram
/// ja query requires ALL 10 bigrams to appear in a single document. The spike measured
/// Recall@3 = 0.167 for AND vs 0.611 for OR. Phrase-quoting each token also neutralises
/// FTS5 operator characters (`OR`, `NOT`, `-`, `*`, `^`, `"`) that could otherwise
/// cause sqlite parse errors.
pub(crate) fn build_fts_match_query(user_query: &str) -> Option<String> {
    let shadow = cjk_bigram_shadow(user_query);
    let tokens: Vec<String> = shadow
        .split_whitespace()
        .map(|t| t.replace('"', ""))
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\""))
        .collect();

    if tokens.is_empty() {
        return None;
    }

    Some(tokens.join(" OR "))
}

/// Build a single FTS5 phrase-MATCH expression from `user_query`.
///
/// Unlike `build_fts_match_query` (which joins bigram tokens with OR), this
/// function joins all tokens with a single space inside one pair of FTS5 phrase
/// quotes: `"tok1 tok2 tok3 ..."`. FTS5 treats a multi-word quoted string as a
/// contiguous phrase — the tokens must appear adjacent and in order in the shadow
/// column.
///
/// This is the tier-0 signal used by hybrid search: documents that contain the
/// full phrase rank above documents that only satisfy the OR query.
///
/// Returns `None` when no usable tokens remain (empty query, whitespace-only,
/// or quotes-only after stripping).
///
/// # Example
/// `"売上レポート"` → shadow tokens `["売上", "上レ", "レポ", "ポー", "ート"]`
/// → phrase expression `"\"売上 上レ レポ ポー ート\""`
pub(crate) fn build_fts_phrase_query(user_query: &str) -> Option<String> {
    let shadow = cjk_bigram_shadow(user_query);
    let tokens: Vec<String> = shadow
        .split_whitespace()
        .map(|t| t.replace('"', ""))
        .filter(|t| !t.is_empty())
        .collect();

    if tokens.is_empty() {
        return None;
    }

    // Join all tokens into one FTS5 quoted phrase: "tok1 tok2 tok3"
    Some(format!("\"{}\"", tokens.join(" ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── cjk_bigram_shadow — output parity with Python harness ──────────────────

    #[test]
    fn ja_full_run() {
        // Python: cjk_bigram_shadow("月次売上レポートの集計")
        //       = "月次 次売 売上 上レ レポ ポー ート トの の集 集計"
        assert_eq!(
            cjk_bigram_shadow("月次売上レポートの集計"),
            "月次 次売 売上 上レ レポ ポー ート トの の集 集計"
        );
    }

    #[test]
    fn ko_space_separated_short_syllables() {
        // Korean words are already whitespace-separated in normal writing.
        // Each ≤2-char word is a single CJK run → single bigram or single char.
        // Python: "월별 급여 지급" → "월별 급여 지급"
        assert_eq!(cjk_bigram_shadow("월별 급여 지급"), "월별 급여 지급");
    }

    #[test]
    fn mixed_ascii_and_cjk() {
        // Python: "fix: WebSocket 재연결" → "fix: WebSocket 재연 연결"
        assert_eq!(
            cjk_bigram_shadow("fix: WebSocket 재연결"),
            "fix: WebSocket 재연 연결"
        );
    }

    #[test]
    fn empty_string() {
        // Python: "" → ""
        assert_eq!(cjk_bigram_shadow(""), "");
    }

    #[test]
    fn single_cjk_char() {
        // Python: "月" → "月"
        assert_eq!(cjk_bigram_shadow("月"), "月");
    }

    #[test]
    fn whitespace_only() {
        // Python: "   " → "" (split_whitespace collapses)
        assert_eq!(cjk_bigram_shadow("   "), "");
    }

    // ── cjk_bigram_shadow — additional coverage ────────────────────────────────

    #[test]
    fn pure_ascii_unchanged() {
        // Non-CJK text passes through (only whitespace normalisation applied).
        assert_eq!(cjk_bigram_shadow("hello world"), "hello world");
    }

    #[test]
    fn two_char_cjk_emits_single_bigram() {
        // "日本" has exactly 2 chars → 1 bigram "日本"
        assert_eq!(cjk_bigram_shadow("日本"), "日本");
    }

    #[test]
    fn three_char_cjk_emits_two_bigrams() {
        // "日本語" → "日本 本語"
        assert_eq!(cjk_bigram_shadow("日本語"), "日本 本語");
    }

    // ── build_fts_match_query ──────────────────────────────────────────────────

    #[test]
    fn ja_query_produces_quoted_or() {
        // "売上" is a 2-char CJK run → single bigram "売上" → '"売上"'
        let result = build_fts_match_query("売上");
        assert_eq!(result, Some("\"売上\"".to_string()));
    }

    #[test]
    fn ko_query_produces_quoted_or() {
        // "업무 보고서":
        //   shadow = "업무 보고 고서"  (업무=2-char bigram; 보고서=3-char → "보고 고서")
        //   tokens after split_whitespace = ["업무", "보고", "고서"]
        //   → '"업무" OR "보고" OR "고서"'
        // Validated against Python harness output.
        let result = build_fts_match_query("업무 보고서");
        assert_eq!(result, Some("\"업무\" OR \"보고\" OR \"고서\"".to_string()));
    }

    #[test]
    fn en_query_unchanged_tokens() {
        // English words are non-CJK, pass through as-is.
        let result = build_fts_match_query("authentication module");
        assert_eq!(result, Some("\"authentication\" OR \"module\"".to_string()));
    }

    #[test]
    fn query_with_literal_quotes_no_syntax_error() {
        // A query containing " characters must not produce FTS5 syntax errors.
        // The quotes are stripped from tokens before re-quoting.
        let result = build_fts_match_query("\"売上\" report");
        // "売上" stripped of quotes → bigram token "売上"; report passes through
        assert_eq!(result, Some("\"売上\" OR \"report\"".to_string()));
    }

    #[test]
    fn query_with_fts5_operators_neutralised() {
        // Tokens like "NOT", "-a", "OR" are wrapped in quotes so they are literal.
        let result = build_fts_match_query("NOT a");
        assert_eq!(result, Some("\"NOT\" OR \"a\"".to_string()));
    }

    #[test]
    fn empty_query_returns_none() {
        assert_eq!(build_fts_match_query(""), None);
    }

    #[test]
    fn whitespace_only_query_returns_none() {
        assert_eq!(build_fts_match_query("   "), None);
    }

    #[test]
    fn query_only_quotes_returns_none() {
        // After stripping " chars the remaining text is empty.
        assert_eq!(build_fts_match_query("\"\""), None);
    }

    // ── build_fts_phrase_query ─────────────────────────────────────────────────

    #[test]
    fn phrase_query_ja_full_run() {
        // "売上レポート" → shadow tokens ["売上", "上レ", "レポ", "ポー", "ート"]
        // → single phrase "\"売上 上レ レポ ポー ート\""
        let result = build_fts_phrase_query("売上レポート");
        assert_eq!(result, Some("\"売上 上レ レポ ポー ート\"".to_string()));
    }

    #[test]
    fn phrase_query_ko_multi_word() {
        // "업무 보고서" → shadow = "업무 보고 고서" → phrase "\"업무 보고 고서\""
        let result = build_fts_phrase_query("업무 보고서");
        assert_eq!(result, Some("\"업무 보고 고서\"".to_string()));
    }

    #[test]
    fn phrase_query_en_becomes_single_phrase() {
        // English tokens join into a single phrase string
        let result = build_fts_phrase_query("authentication module");
        assert_eq!(result, Some("\"authentication module\"".to_string()));
    }

    #[test]
    fn phrase_query_empty_returns_none() {
        assert_eq!(build_fts_phrase_query(""), None);
    }

    #[test]
    fn phrase_query_whitespace_only_returns_none() {
        assert_eq!(build_fts_phrase_query("   "), None);
    }

    #[test]
    fn phrase_query_quotes_only_returns_none() {
        // Only double-quote chars — stripped tokens are empty.
        assert_eq!(build_fts_phrase_query("\"\""), None);
    }

    #[test]
    fn phrase_query_single_token_no_spaces() {
        // "認証" (2-char CJK) → single bigram "認証" → phrase "\"認証\""
        let result = build_fts_phrase_query("認証");
        assert_eq!(result, Some("\"認証\"".to_string()));
    }
}
