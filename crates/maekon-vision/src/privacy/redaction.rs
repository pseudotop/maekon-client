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
                result.extend(&chars[i..start]);
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
    let mut result = text.to_string();
    let chars: Vec<char> = result.chars().collect();
    let len = chars.len();

    let mut masked = String::new();
    let mut i = 0;
    while i < len {
        if (chars[i] == '+' || chars[i].is_ascii_digit()) && is_phone_number_start(&chars, i, len) {
            if let Some(end) = find_phone_number_end(&chars, i, len) {
                masked.push_str("[PHONE]");
                i = end;
                continue;
            }
        }
        masked.push(chars[i]);
        i += 1;
    }
    result = masked;
    result
}

fn is_phone_number_start(chars: &[char], pos: usize, _len: usize) -> bool {
    if chars[pos] == '+' {
        return true;
    }
    if chars[pos] == '0' {
        return true;
    }
    false
}

fn find_phone_number_end(chars: &[char], start: usize, len: usize) -> Option<usize> {
    let mut i = start;
    let mut digit_count = 0;
    let mut separator_count = 0;

    if i < len && chars[i] == '+' {
        i += 1;
    }

    while i < len {
        if chars[i].is_ascii_digit() {
            digit_count += 1;
            i += 1;
        } else if chars[i] == '-' || chars[i] == ' ' || chars[i] == '.' {
            separator_count += 1;
            if separator_count > 4 {
                break;
            }
            i += 1;
        } else {
            break;
        }
    }

    if digit_count >= 9 && separator_count >= 1 {
        Some(i)
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
    let mut result = text.to_string();
    if result.contains('-') {
        let parts: Vec<String> = result.split('-').map(|s| s.to_string()).collect();
        for window in parts.windows(2) {
            if window[0].len() >= 6
                && window[0].chars().rev().take(6).all(|c| c.is_ascii_digit())
                && window[1].len() >= 7
                && window[1].chars().take(7).all(|c| c.is_ascii_digit())
            {
                let needle = format!("{}-{}", &window[0][window[0].len() - 6..], &window[1][..7]);
                result = result.replace(&needle, "[KR_ID]");
            }
        }
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
    let mut result = text.to_string();
    let prefixes = [
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

    for prefix in &prefixes {
        while let Some(pos) = result.find(prefix) {
            let start = pos;
            let after = &result[pos + prefix.len()..];
            let end_offset = after
                .find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ',' || c == ';')
                .unwrap_or(after.len());

            if end_offset >= 8 {
                let key_end = pos + prefix.len() + end_offset;
                result = format!("{}[API_KEY]{}", &result[..start], &result[key_end..]);
            } else {
                break;
            }
        }
    }

    // Mask bearer tokens: "Bearer <token>" (case-insensitive)
    result = mask_bearer_tokens(&result);

    // Mask PEM private key blocks: "-----BEGIN * PRIVATE KEY-----"
    result = mask_private_key_blocks(&result);

    result
}

fn mask_bearer_tokens(text: &str) -> String {
    // Rebuild `lower` from `result` on every replacement to keep byte offsets
    // in sync with `result`.  This avoids the stale-offset bug that would
    // occur when successive replacements change the string length.
    let mut result = text.to_string();
    let needle = "bearer ";
    let mut search_from = 0usize;

    loop {
        // Always derive `lower` from the current `result` so offsets are correct.
        let lower = result.to_lowercase();
        if search_from >= lower.len() {
            break;
        }
        let Some(rel_pos) = lower[search_from..].find(needle) else {
            break;
        };
        let pos = search_from + rel_pos;
        let token_start = pos + needle.len();
        let after = &result[token_start..];
        let end_offset = after
            .find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ',' || c == ';')
            .unwrap_or(after.len());

        if end_offset >= 8 {
            let tail = result[token_start + end_offset..].to_string();
            result = format!("{}Bearer [API_KEY]{}", &result[..pos], tail);
            // Advance past the replacement so we do not re-examine it.
            search_from = pos + "bearer [api_key]".len();
        } else {
            search_from = token_start + end_offset.max(1);
        }
    }

    result
}

fn mask_private_key_blocks(text: &str) -> String {
    // Mask PEM-style private key headers: -----BEGIN * PRIVATE KEY-----
    if !text.contains("-----BEGIN ") || !text.contains("PRIVATE KEY-----") {
        return text.to_string();
    }

    let mut result = text.to_string();
    let mut search_from = 0;
    while let Some(rel_begin) = result[search_from..].find("-----BEGIN ") {
        let begin_pos = search_from + rel_begin;
        let header_after = &result[begin_pos + 11..];
        // Find the closing dashes of the header line
        let Some(header_end_rel) = header_after.find("-----") else {
            break;
        };
        let label = &header_after[..header_end_rel];
        if !label.contains("PRIVATE KEY") {
            // Not a private key block — skip past this marker and keep searching
            search_from = begin_pos + 11;
            continue;
        }
        let label = label.to_string();
        // Find the matching END marker
        let end_marker = format!("-----END {}-----", label);
        let block_end = result[begin_pos..].find(&end_marker);
        let replace_end = if let Some(rel) = block_end {
            begin_pos + rel + end_marker.len()
        } else {
            // No closing marker found — mask to end of string
            result.len()
        };
        let tail = result[replace_end..].to_string();
        result = format!("{}[PRIVATE_KEY]{}", &result[..begin_pos], tail);
        // After replacement, search_from stays at begin_pos (now points at [PRIVATE_KEY])
        search_from = begin_pos + "[PRIVATE_KEY]".len();
    }

    result
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
        // IPv6 주소는 16진수(0-9a-fA-F) 또는 압축 표기(`::`)로 시작할 수 있다.
        // IPv4 매칭에 실패한 경우에만 IPv6 파싱을 시도한다 (per-pattern 접근).
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

/// IPv6 주소를 `start` 위치부터 파싱한다. 매칭 시 끝 인덱스를 반환한다.
///
/// `try_parse_ipv4`의 per-pattern 방식을 그대로 따른다. RFC 4291 표기를 폭넓게
/// 수용하되 false positive 를 줄이기 위해 다음을 요구한다:
/// - 최소 2개의 콜론(`:`)을 포함 (단일 콜론 토큰은 IPv6 가 아님)
/// - `::` 압축 표기는 1회만 허용
/// - 각 그룹은 1~4 자리 16진수
/// - 시작 직전이 16진수/콜론이면 단어 중간으로 보고 거부
fn try_parse_ipv6(chars: &[char], start: usize, len: usize) -> Option<usize> {
    // 단어 중간 매칭 방지: 직전 문자가 16진수/콜론이면 거부.
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
            // `::` 압축 표기 처리
            if i + 1 < len && chars[i + 1] == ':' {
                if double_colon_seen {
                    return None; // `::` 는 1회만 허용
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

    // 콜론 2개 이상 + 16진수 그룹 1개 이상이어야 IPv6 로 간주.
    if colon_count >= 2 && group_count >= 1 {
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

    // Consume alphanumeric chars and optional spaces/dashes (formatted IBAN)
    while i < len {
        if chars[i].is_ascii_alphanumeric() {
            alnum_count += 1;
            i += 1;
        } else if chars[i] == ' ' || chars[i] == '-' {
            // Allow spaces/dashes in formatted IBANs
            i += 1;
        } else {
            break;
        }
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

pub fn mask_user_paths(text: &str) -> String {
    let result = mask_user_paths_os(text, "/Users/", 7, '/');
    let result = mask_user_paths_os(&result, "/home/", 6, '/');
    mask_user_paths_os(&result, r"C:\Users\", 9, '\\')
}

/// Replace OS-specific user path prefixes with `[USER]`, tracking offset to avoid
/// re-matching replacement text.
fn mask_user_paths_os(text: &str, prefix: &str, prefix_len: usize, sep: char) -> String {
    let mut result = String::with_capacity(text.len());
    let mut search_from = 0;

    while search_from < text.len() {
        match text[search_from..].find(prefix) {
            Some(rel_start) => {
                let abs_start = search_from + rel_start;
                let after = &text[abs_start + prefix_len..];
                if after.is_empty() {
                    // prefix at end of string with no username — leave as-is
                    result.push_str(&text[search_from..]);
                    return result;
                }
                let end = after
                    .find(|c: char| c == sep || c.is_whitespace())
                    .unwrap_or(after.len());
                if end == 0 {
                    // prefix followed by separator or whitespace — no username
                    result.push_str(&text[search_from..abs_start + prefix_len]);
                    search_from = abs_start + prefix_len;
                    continue;
                }
                // Emit text before the match + masked replacement
                result.push_str(&text[search_from..abs_start]);
                result.push_str(&prefix[..prefix_len]);
                result.push_str("[USER]");
                search_from = abs_start + prefix_len + end;
            }
            None => {
                result.push_str(&text[search_from..]);
                break;
            }
        }
    }

    result
}
