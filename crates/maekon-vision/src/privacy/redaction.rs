pub fn mask_emails(text: &str) -> String {
    let mut result = String::new();
    let mut i = 0;
    let chars: Vec<char> = text.chars().collect();

    while i < chars.len() {
        if let Some(at_pos) = chars[i..].iter().position(|&c| c == '@') {
            let at_abs = i + at_pos;
            let start = chars[..at_abs]
                .iter()
                .rposition(|c| c.is_whitespace() || *c == '<' || *c == '(')
                .map(|p| p + 1)
                .unwrap_or(i);

            let end = chars[at_abs + 1..]
                .iter()
                .position(|c| c.is_whitespace() || *c == '>' || *c == ')')
                .map(|p| at_abs + 1 + p)
                .unwrap_or(chars.len());

            if at_abs > start && end > at_abs + 1 {
                // `start` (the position just after the preceding `<`/`(`/whitespace
                // delimiter) can land *before* the current cursor `i` when an
                // adjacent bracketed/parenthesized email precedes another at-token
                // (e.g. `<a@b.com>x@y.com`). Emitting `chars[i..start]` with
                // `start < i` is a reversed slice range and panics. Clamp the
                // lower bound to `i` so the prefix slice is always non-reversed;
                // any already-emitted prefix is simply not re-copied.
                let pre_start = start.max(i);
                result.extend(&chars[i..pre_start]);
                result.push_str("[EMAIL]");
                i = end;
                continue;
            }
        }

        if i < chars.len() {
            result.push(chars[i]);
            i += 1;
        } else {
            break;
        }
    }

    result
}

pub fn mask_phone_numbers(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();

    let mut masked = String::with_capacity(text.len());
    let mut i = 0;
    while i < len {
        if (chars[i] == '+' || chars[i].is_ascii_digit()) && is_phone_number_start(&chars, i) {
            if let Some(end) = find_phone_number_end(&chars, i, len) {
                masked.push_str("[PHONE]");
                i = end;
                continue;
            }
        }
        masked.push(chars[i]);
        i += 1;
    }
    masked
}

/// A phone match may begin at `+` or any ASCII digit (review4 V2 — the old code
/// only accepted `+`/`0`, leaking US and most international numbers). A leading
/// word-boundary guard rejects a start that is glued to a preceding token so we
/// never begin a match mid-number (IP octet, card/IBAN group, version) or mid-word.
fn is_phone_number_start(chars: &[char], pos: usize) -> bool {
    if pos > 0 {
        let prev = chars[pos - 1];
        if prev.is_ascii_digit() || prev.is_ascii_alphabetic() || prev == '.' || prev == '+' {
            return false;
        }
    }
    chars[pos] == '+' || chars[pos].is_ascii_digit()
}

fn find_phone_number_end(chars: &[char], start: usize, len: usize) -> Option<usize> {
    let mut i = start;
    let mut digit_count = 0usize;
    let mut separator_count = 0usize;
    // Index just past the last DIGIT consumed — the match ends here so a trailing
    // separator is never absorbed into [PHONE] (review4 V7: prevents merging with
    // the following token and the adjacent-phone residual leak).
    let mut last_digit_end = start;

    if i < len && chars[i] == '+' {
        i += 1;
    }

    while i < len {
        let c = chars[i];
        if c.is_ascii_digit() {
            digit_count += 1;
            i += 1;
            last_digit_end = i;
        } else if c == '-' || c == ' ' {
            // A separator is internal only when it sits between digit groups AND we
            // do not yet hold a complete (>=9 digit) number. Once 9+ digits are
            // accumulated, treat the separator as a boundary so two space/dash-
            // adjacent numbers do not merge into one match. '.' is intentionally NOT
            // a separator (avoids swallowing IPv4 octets / version strings).
            if digit_count >= 9 || i + 1 >= len || !chars[i + 1].is_ascii_digit() {
                break;
            }
            separator_count += 1;
            i += 1;
        } else {
            break;
        }
    }

    // E.164 caps a phone at 15 digits. Separator-bearing numbers need >=9 digits;
    // separator-less runs need >=10 (review4 V14) to avoid masking short numeric IDs.
    let valid = (separator_count >= 1 && (9..=15).contains(&digit_count))
        || (separator_count == 0 && (10..=15).contains(&digit_count));
    if valid {
        Some(last_digit_end)
    } else {
        None
    }
}

pub fn mask_credit_cards(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut result = String::with_capacity(text.len());
    let mut i = 0;

    while i < len {
        if chars[i].is_ascii_digit() {
            let start = i;
            let mut digit_count = 0;

            while i < len {
                if chars[i].is_ascii_digit() {
                    digit_count += 1;
                    i += 1;
                } else if (chars[i] == ' ' || chars[i] == '-')
                    && i + 1 < len
                    && chars[i + 1].is_ascii_digit()
                {
                    i += 1;
                } else {
                    break;
                }
            }

            if (13..=19).contains(&digit_count) {
                result.push_str("[CARD]");
            } else {
                for ch in &chars[start..i] {
                    result.push(*ch);
                }
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

pub fn mask_korean_id(text: &str) -> String {
    // Korean RRN: 6 digits '-' 7 digits. Single O(N) left-to-right pass (review4
    // V1/V5 sibling: the previous split + result.replace(per-window) rebuilt the
    // whole string for each match — O(K*N) on uncapped input). Mask the last-6 +
    // '-' + first-7 core when a '-' has >=6 digits before and >=7 after, mirroring
    // the previous needle construction. Digit runs are bounded by separators, so the
    // back-scan is amortized O(N).
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut result = String::with_capacity(text.len());
    let mut i = 0;
    while i < len {
        if chars[i] == '-' {
            let digits_before = chars[..i]
                .iter()
                .rev()
                .take_while(|c| c.is_ascii_digit())
                .count();
            let digits_after = chars[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_digit())
                .count();
            if digits_before >= 6 && digits_after >= 7 {
                // The 6 ASCII digits before the '-' are already in `result`; drop
                // them (1 byte each) and emit the marker, then skip '-' + 7 digits.
                result.truncate(result.len() - 6);
                result.push_str("[KR_ID]");
                i += 1 + 7;
                continue;
            }
        }
        result.push(chars[i]);
        i += 1;
    }
    result
}

/// Mask US Social Security Numbers (SSN): `\d{3}-\d{2}-\d{4}` → `[SSN]`
pub fn mask_ssn(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // SSN pattern: exactly 3 digits, dash, 2 digits, dash, 4 digits
        if i + 10 < len
            && chars[i].is_ascii_digit()
            && chars[i + 1].is_ascii_digit()
            && chars[i + 2].is_ascii_digit()
            && chars[i + 3] == '-'
            && chars[i + 4].is_ascii_digit()
            && chars[i + 5].is_ascii_digit()
            && chars[i + 6] == '-'
            && chars[i + 7].is_ascii_digit()
            && chars[i + 8].is_ascii_digit()
            && chars[i + 9].is_ascii_digit()
            && chars[i + 10].is_ascii_digit()
        {
            // Ensure not preceded by a digit (avoid matching inside longer numbers)
            let preceded_by_digit = i > 0 && chars[i - 1].is_ascii_digit();
            // Ensure not followed by a digit
            let followed_by_digit = i + 11 < len && chars[i + 11].is_ascii_digit();
            if !preceded_by_digit && !followed_by_digit {
                result.push_str("[SSN]");
                i += 11;
                continue;
            }
        }
        result.push(chars[i]);
        i += 1;
    }

    result
}

pub fn mask_api_keys(text: &str) -> String {
    // 15 known key prefixes (case-sensitive). A token is a key when >=8 chars
    // follow the prefix before a terminator. (review4 V5: single O(N) left-to-right
    // pass — the previous per-prefix scan rebuilt the whole tail with format! on
    // every match, giving O(N^2) on inputs dense in key-like tokens.)
    const PREFIXES: [&str; 15] = [
        "sk-",
        "pk-",
        "sk_",
        "pk_",
        "api_",
        "key_",
        "token_",
        "secret_",
        "AKIA",
        "ghp_",
        "gho_",
        "ghs_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
    ];
    let is_terminator =
        |c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ',' || c == ';';

    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < len {
        // Match the longest prefix starting at i (prefixes are pure ASCII).
        let mut matched_prefix_len = 0usize;
        for prefix in PREFIXES {
            let plen = prefix.len(); // ASCII => char count == byte len
            if plen > matched_prefix_len
                && i + plen <= len
                && chars[i..i + plen].iter().copied().eq(prefix.chars())
            {
                matched_prefix_len = plen;
            }
        }
        if matched_prefix_len > 0 {
            let token_start = i + matched_prefix_len;
            let mut j = token_start;
            while j < len && !is_terminator(chars[j]) {
                j += 1;
            }
            if j - token_start >= 8 {
                out.push_str("[API_KEY]");
                i = j;
                continue;
            }
            // Short token (likely a false positive): emit the prefix verbatim and
            // continue scanning from just after it.
            out.extend(&chars[i..token_start]);
            i = token_start;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }

    // Mask bearer tokens: "Bearer <token>" (case-insensitive)
    let result = mask_bearer_tokens(&out);

    // Mask PEM private key blocks: "-----BEGIN * PRIVATE KEY-----"
    mask_private_key_blocks(&result)
}

fn mask_bearer_tokens(text: &str) -> String {
    // Single O(N) left-to-right pass with a case-insensitive "bearer " window match
    // (review4 V1: the previous loop called result.to_lowercase() on EVERY iteration
    // and advanced ~one occurrence at a time, giving O(N^2) — multi-second on large
    // inputs dense in the substring, e.g. an auth-header log dump).
    const NEEDLE: [char; 7] = ['b', 'e', 'a', 'r', 'e', 'r', ' '];
    let is_terminator =
        |c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ',' || c == ';';

    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < len {
        let matches_needle = i + NEEDLE.len() <= len
            && chars[i..i + NEEDLE.len()]
                .iter()
                .zip(NEEDLE.iter())
                .all(|(c, n)| c.eq_ignore_ascii_case(n));
        if matches_needle {
            let token_start = i + NEEDLE.len();
            let mut j = token_start;
            while j < len && !is_terminator(chars[j]) {
                j += 1;
            }
            if j - token_start >= 8 {
                out.push_str("Bearer [API_KEY]");
                i = j;
                continue;
            }
            // Short token: emit the matched "bearer " verbatim (preserving case) and
            // continue scanning from just after it.
            out.extend(&chars[i..token_start]);
            i = token_start;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn mask_private_key_blocks(text: &str) -> String {
    // Mask PEM-style private key headers: -----BEGIN * PRIVATE KEY-----
    if !text.contains("-----BEGIN ") || !text.contains("PRIVATE KEY-----") {
        return text.to_string();
    }

    // Single O(N) pass over the immutable input, accumulating into a fresh String
    // (review4 V1/V5 sibling: the previous code rebuilt the whole accumulator via
    // format! on every BEGIN block — O(M*N) on inputs dense in PEM blocks, and this
    // runs on the Strict mask_api_keys hot path).
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(rel_begin) = text[cursor..].find("-----BEGIN ") {
        let begin_pos = cursor + rel_begin;
        let header_after = &text[begin_pos + 11..];
        // Find the closing dashes of the header line
        let Some(header_end_rel) = header_after.find("-----") else {
            break;
        };
        let label = &header_after[..header_end_rel];
        if !label.contains("PRIVATE KEY") {
            // Not a private key block — emit up to and including this BEGIN marker
            // (unmasked) and keep searching after it.
            out.push_str(&text[cursor..begin_pos + 11]);
            cursor = begin_pos + 11;
            continue;
        }
        // Find the matching END marker; if absent, mask to end of string.
        let end_marker = format!("-----END {label}-----");
        let block_end = text[begin_pos..]
            .find(&end_marker)
            .map(|rel| begin_pos + rel + end_marker.len())
            .unwrap_or(text.len());
        out.push_str(&text[cursor..begin_pos]);
        out.push_str("[PRIVATE_KEY]");
        cursor = block_end;
    }
    out.push_str(&text[cursor..]);

    out
}

pub fn mask_ip_addresses(text: &str) -> String {
    let mut result = text.to_string();
    let chars: Vec<char> = result.chars().collect();
    let len = chars.len();
    let mut masked = String::new();
    let mut i = 0;

    while i < len {
        if chars[i].is_ascii_digit() {
            if let Some((ip_end, is_valid)) = try_parse_ipv4(&chars, i, len) {
                if is_valid {
                    masked.push_str("[IP]");
                    i = ip_end;
                    continue;
                }
            }
        }
        // An IPv6 address can start with a hex digit (0-9a-fA-F) or compressed
        // notation (`::`). Only attempt IPv6 parsing when IPv4 matching failed
        // (per-pattern approach).
        if chars[i].is_ascii_hexdigit() || chars[i] == ':' {
            if let Some(ip_end) = try_parse_ipv6(&chars, i, len) {
                masked.push_str("[IP]");
                i = ip_end;
                continue;
            }
        }
        masked.push(chars[i]);
        i += 1;
    }

    result = masked;
    result
}

/// Try to parse an IPv6 address starting at `start`. Returns the end index on match.
///
/// Follows the same per-pattern approach as `try_parse_ipv4`. Accepts RFC 4291
/// notation broadly but tightens acceptance to cut false positives from git SHAs,
/// ISO timestamps (HH:MM:SS), UUIDs, and k8s names:
///
/// - Each group: 1–4 hex digits.
/// - `::` compressed notation: at most once.
/// - Word-boundary guard: rejects if the preceding character is a hex digit or colon.
/// - **Acceptance gate** (both conditions must hold):
///   1. `colon_count >= 7` **or** a `::` was seen — this alone excludes `HH:MM:SS`
///      (2 colons, no `::`) and short hex tokens.
///   2. `group_count >= 2` **or** a `::` was seen — `::1` (loopback) has only one
///      explicit group but the `::` signals implicit zero groups, so it is accepted.
///
/// Examples that PASS: `::1`, `fe80::1`, `2001:db8::1`, full 8-group addresses.
/// Examples that FAIL: git SHAs (no colons), `HH:MM:SS` timestamps (no `::`,
/// colon_count=2), UUIDs (hyphens terminate the scan before enough colons).
fn try_parse_ipv6(chars: &[char], start: usize, len: usize) -> Option<usize> {
    // Word-boundary guard: reject if the preceding character is a hex digit or colon.
    if start > 0 && (chars[start - 1].is_ascii_hexdigit() || chars[start - 1] == ':') {
        return None;
    }

    let mut i = start;
    let mut colon_count = 0usize;
    let mut double_colon_seen = false;
    let mut group_digits = 0usize;
    let mut group_count = 0usize;

    while i < len {
        let c = chars[i];
        if c.is_ascii_hexdigit() {
            group_digits += 1;
            if group_digits > 4 {
                return None;
            }
            i += 1;
        } else if c == ':' {
            colon_count += 1;
            if group_digits > 0 {
                group_count += 1;
            }
            group_digits = 0;
            // Handle `::` compressed notation (allowed at most once).
            if i + 1 < len && chars[i + 1] == ':' {
                if double_colon_seen {
                    return None; // `::` may appear at most once in an IPv6 address
                }
                double_colon_seen = true;
                colon_count += 1;
                i += 2;
            } else {
                i += 1;
            }
        } else {
            break;
        }
    }

    if group_digits > 0 {
        group_count += 1;
    }

    // Gate 1: enough colons for a real IPv6, or a `::` was present.
    //   - Excludes HH:MM:SS timestamps (colon_count=2, no `::`)
    //   - Excludes short hex-colon tokens
    let enough_colons = colon_count >= 7 || double_colon_seen;

    // Gate 2: enough explicit hex groups, or `::` supplies the implicit zero groups.
    //   - Allows `::1` (group_count=1, double_colon_seen=true)
    //   - Requires >=2 explicit groups when no `::` is present
    let enough_groups = group_count >= 2 || double_colon_seen;

    if enough_colons && enough_groups {
        Some(i)
    } else {
        None
    }
}

fn try_parse_ipv4(chars: &[char], start: usize, len: usize) -> Option<(usize, bool)> {
    let mut i = start;
    let mut octet_count = 0;

    for _ in 0..4 {
        let octet_start = i;
        while i < len && chars[i].is_ascii_digit() {
            i += 1;
        }
        let octet_len = i - octet_start;
        if octet_len == 0 || octet_len > 3 {
            return None;
        }

        let octet_str: String = chars[octet_start..i].iter().collect();
        if let Ok(val) = octet_str.parse::<u32>() {
            if val > 255 {
                return None;
            }
        }

        octet_count += 1;
        if octet_count < 4 {
            if i < len && chars[i] == '.' {
                i += 1;
            } else {
                return None;
            }
        }
    }

    if i < len && chars[i].is_ascii_digit() {
        return None;
    }

    Some((i, octet_count == 4))
}

/// Mask IBAN numbers: 2 uppercase country letters + 2 check digits + 4 bank code + 7-30 alphanumeric
/// characters. Masks as `XX99****...****` preserving the country code and check digits.
pub fn mask_iban(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if let Some((iban_end, prefix_len)) = try_parse_iban(&chars, i, len) {
            // Preserve the country code + check digits (first 4 chars), mask the rest
            let prefix: String = chars[i..i + prefix_len].iter().collect();
            result.push_str(&prefix);
            result.push_str("[IBAN]");
            i = iban_end;
            continue;
        }
        result.push(chars[i]);
        i += 1;
    }

    result
}

/// Try to parse an IBAN starting at `start`. Returns (end_index, prefix_length) if found.
/// Pattern: 2 uppercase letters + 2 digits + 4 alphanumeric bank code + 7-30 more alphanumeric chars.
fn try_parse_iban(chars: &[char], start: usize, len: usize) -> Option<(usize, usize)> {
    // Need at least 15 chars for a minimal IBAN (e.g., NO93 8601 1117 947)
    if start + 15 > len {
        return None;
    }
    // Must not be preceded by an alphanumeric char (avoid matching mid-word)
    if start > 0 && chars[start - 1].is_alphanumeric() {
        return None;
    }
    // First 2 chars: uppercase letters (country code)
    if !chars[start].is_ascii_uppercase() || !chars[start + 1].is_ascii_uppercase() {
        return None;
    }
    // Next 2 chars: digits (check digits)
    if !chars[start + 2].is_ascii_digit() || !chars[start + 3].is_ascii_digit() {
        return None;
    }

    let prefix_len = 4; // country code + check digits
    let mut i = start + 4;
    let mut alnum_count = 0;
    // The 4-char country+check prefix is the first canonical group.
    let mut prev_run_len = 4usize;

    // Consume the IBAN body as canonical groups: alphanumeric runs separated by a
    // SINGLE space/dash, where an internal separator is honored only when the run
    // it follows was exactly 4 chars (canonical grouping). A shorter/longer run
    // terminates the IBAN, so trailing plain words after a formatted IBAN are no
    // longer greedily absorbed and deleted. (review4 V6) The compact (separator-
    // free) form is consumed as a single long run.
    loop {
        if i < len && (chars[i] == ' ' || chars[i] == '-') {
            if prev_run_len == 4 && i + 1 < len && chars[i + 1].is_ascii_alphanumeric() {
                i += 1; // internal separator between two canonical groups
            } else {
                break; // separator after a non-full group => boundary, stop
            }
        }
        let run_start = i;
        while i < len && chars[i].is_ascii_alphanumeric() {
            alnum_count += 1;
            i += 1;
        }
        let run_len = i - run_start;
        if run_len == 0 {
            break;
        }
        prev_run_len = run_len;
    }

    // IBAN body (after country+check) must be 11-30 alphanumeric chars
    if (11..=30).contains(&alnum_count) {
        Some((i, prefix_len))
    } else {
        None
    }
}

/// Mask passport numbers: a letter followed by 7-8 digits (common format across many countries).
/// Only applied at Strict level due to high false positive risk.
pub fn mask_passport(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if chars[i].is_ascii_alphabetic() && i + 8 <= len {
            // Must not be preceded by alphanumeric (avoid matching mid-word)
            let preceded_by_alnum = i > 0 && chars[i - 1].is_alphanumeric();
            if !preceded_by_alnum {
                let mut j = i + 1;
                let mut digit_count = 0;
                while j < len && chars[j].is_ascii_digit() {
                    digit_count += 1;
                    j += 1;
                }
                // Must not be followed by alphanumeric
                let followed_by_alnum = j < len && chars[j].is_alphanumeric();
                if (7..=8).contains(&digit_count) && !followed_by_alnum {
                    result.push_str("[PASSPORT]");
                    i = j;
                    continue;
                }
            }
        }
        result.push(chars[i]);
        i += 1;
    }

    result
}

/// Replace OS-specific user-home path prefixes (`/Users/`, `/home/`, `C:\Users\`)
/// with `[USER]`. Delegates to the canonical masker in `maekon-core` so there is
/// a single implementation shared with `maekon-monitor` (which cannot depend on
/// `maekon-vision` per the hexagonal rule). Kept as a thin re-export so existing
/// `redaction::mask_user_paths` call sites and tests are unchanged.
pub use maekon_core::path_redaction::mask_user_paths;

#[cfg(test)]
mod tests {
    use super::*;

    // --- try_parse_ipv6 acceptance tests ---

    fn ipv6_accepts(input: &str) -> bool {
        let chars: Vec<char> = input.chars().collect();
        let len = chars.len();
        try_parse_ipv6(&chars, 0, len).is_some()
    }

    #[test]
    fn test_ipv6_accept_loopback() {
        // ::1 — compressed loopback, only one explicit group
        assert!(ipv6_accepts("::1"), "::1 must be accepted as IPv6");
    }

    #[test]
    fn test_ipv6_accept_link_local() {
        // fe80::1 — link-local with compressed suffix
        assert!(ipv6_accepts("fe80::1"), "fe80::1 must be accepted as IPv6");
    }

    #[test]
    fn test_ipv6_accept_documentation_prefix() {
        // 2001:db8::1 — documentation prefix (RFC 3849) with compressed suffix
        assert!(
            ipv6_accepts("2001:db8::1"),
            "2001:db8::1 must be accepted as IPv6"
        );
    }

    #[test]
    fn test_ipv6_accept_full_eight_groups() {
        // Full 8-group address without compression
        assert!(
            ipv6_accepts("2001:0db8:85a3:0000:0000:8a2e:0370:7334"),
            "full 8-group IPv6 must be accepted"
        );
    }

    // --- try_parse_ipv6 rejection tests ---

    #[test]
    fn test_ipv6_reject_git_sha() {
        // 40-char git SHA — all hex, no colons
        let sha = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
        assert!(!ipv6_accepts(sha), "git SHA must not be accepted as IPv6");
    }

    #[test]
    fn test_ipv6_reject_uuid() {
        // UUID with hyphens — hyphens terminate the scan before enough colons accumulate
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        assert!(!ipv6_accepts(uuid), "UUID must not be accepted as IPv6");
    }

    #[test]
    fn test_ipv6_reject_iso_timestamp() {
        // ISO 8601 time component HH:MM:SS — 2 colons, no ::, colon_count < 7
        assert!(
            !ipv6_accepts("14:30:00"),
            "ISO time HH:MM:SS must not be accepted as IPv6"
        );
    }

    // --- mask_api_keys regression tests ---

    #[test]
    fn test_mask_api_keys_short_prefix_does_not_skip_later_real_key() {
        // A short false-positive prefix occurrence (`sk-x`, <8 chars after the
        // prefix) must not abort the scan for a later real key sharing the same
        // prefix. Regression for #6087: the old `break` on the short-token case
        // leaked the real key.
        let input = "sk-x then sk-abcdefghijklmnop here";
        let masked = mask_api_keys(input);
        assert!(
            masked.contains("[API_KEY]"),
            "later real key sharing the prefix must be masked: {masked}"
        );
        assert!(
            !masked.contains("sk-abcdefghijklmnop"),
            "the real key must not be left in the output: {masked}"
        );
        // The short false positive is too short to be a key and is left intact.
        assert!(
            masked.contains("sk-x"),
            "short false-positive prefix should be preserved: {masked}"
        );
    }

    // --- mask_emails regression tests (#6116) ---
    //
    // The reversed-slice panic occurred when `start` (the position just after a
    // `<`/`(`/whitespace delimiter preceding an email) landed before the current
    // cursor `i`, which happens with adjacent bracketed/parenthesized emails
    // immediately followed by another at-token. Each case below asserts the call
    // does NOT panic, both addresses are masked, and no residual at-sign leaks.

    fn assert_both_masked_no_at(input: &str) -> String {
        let masked = mask_emails(input);
        assert!(
            !masked.contains('@'),
            "no residual at-sign should remain: {masked}"
        );
        assert!(
            masked.contains("[EMAIL]"),
            "emails must be masked: {masked}"
        );
        masked
    }

    #[test]
    fn test_mask_emails_bracketed_adjacent_at_token_no_panic() {
        // Bracketed `a@b.com` immediately followed by `x@y.com` with no gap —
        // the second at-token's backward delimiter search lands before `i`.
        let masked = assert_both_masked_no_at("<a@b.com>x@y.com");
        assert!(
            !masked.contains("a@b.com") && !masked.contains("x@y.com"),
            "both addresses must be masked: {masked}"
        );
    }

    #[test]
    fn test_mask_emails_parenthesized_adjacent_at_token_no_panic() {
        // Parenthesized `a@b.com` immediately followed by `c@d.com`.
        let masked = assert_both_masked_no_at("(a@b.com)c@d.com");
        assert!(
            !masked.contains("a@b.com") && !masked.contains("c@d.com"),
            "both addresses must be masked: {masked}"
        );
    }

    #[test]
    fn test_mask_emails_bracketed_space_then_at_token() {
        // Bracketed `a@b.com`, a space, then `x@y.com`.
        let masked = assert_both_masked_no_at("<a@b.com> x@y.com");
        assert!(
            !masked.contains("a@b.com") && !masked.contains("x@y.com"),
            "both addresses must be masked: {masked}"
        );
    }

    #[test]
    fn test_mask_emails_login_title_case() {
        // The existing window-title case must keep working after the clamp fix.
        let masked = mask_emails("Login - user@example.com");
        assert!(masked.contains("[EMAIL]"), "email must be masked: {masked}");
        assert!(
            !masked.contains("user@example.com"),
            "the address must not be left in the output: {masked}"
        );
        assert!(
            !masked.contains('@'),
            "no residual at-sign should remain: {masked}"
        );
        assert!(
            masked.starts_with("Login - "),
            "the non-email prefix must be preserved: {masked}"
        );
    }
}
