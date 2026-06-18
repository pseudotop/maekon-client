use maekon_core::config::PiiFilterLevel;

use super::redaction::{
    mask_api_keys, mask_credit_cards, mask_emails, mask_iban, mask_ip_addresses, mask_korean_id,
    mask_passport, mask_phone_numbers, mask_ssn, mask_user_paths,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PiiMarker {
    Email,
    Phone,
    Card,
    KoreanId,
    Ssn,
    Iban,
    ApiKey,
    Ip,
    UserPath,
    Passport,
}

pub fn sanitize_title_with_level(title: &str, level: PiiFilterLevel) -> String {
    match level {
        PiiFilterLevel::Off => title.to_string(),
        PiiFilterLevel::Basic => {
            let mut result = title.to_string();
            result = mask_emails(&result);
            result = mask_phone_numbers(&result);
            result
        }
        PiiFilterLevel::Standard => {
            // Mask longer / structured numeric PII (IBAN, Korean-ID, SSN, card)
            // BEFORE phone numbers so the broadened phone scanner (review4 V2/V14,
            // which now starts at any digit and accepts separator-less runs) cannot
            // partially consume those values. Email is order-independent. Basic
            // remains {email, phone}; Standard is a strict superset of it.
            let mut result = mask_emails(title);
            result = mask_iban(&result);
            result = mask_korean_id(&result);
            result = mask_ssn(&result);
            result = mask_credit_cards(&result);
            result = mask_phone_numbers(&result);
            result = mask_user_paths(&result);
            result
        }
        PiiFilterLevel::Strict => {
            let mut result = sanitize_title_with_level(title, PiiFilterLevel::Standard);
            result = mask_api_keys(&result);
            result = mask_ip_addresses(&result);
            result = mask_passport(&result);
            result
        }
    }
}

pub fn sanitize_title(title: &str) -> String {
    sanitize_title_with_level(title, PiiFilterLevel::Standard)
}

pub fn detect_pii_markers_with_level(text: &str, level: PiiFilterLevel) -> Vec<PiiMarker> {
    let masked = sanitize_title_with_level(text, level);
    let mut markers = Vec::new();

    if marker_inserted(text, &masked, "[EMAIL]") {
        markers.push(PiiMarker::Email);
    }
    if marker_inserted(text, &masked, "[PHONE]") {
        markers.push(PiiMarker::Phone);
    }
    if marker_inserted(text, &masked, "[CARD]") {
        markers.push(PiiMarker::Card);
    }
    if marker_inserted(text, &masked, "[KR_ID]") {
        markers.push(PiiMarker::KoreanId);
    }
    if marker_inserted(text, &masked, "[SSN]") {
        markers.push(PiiMarker::Ssn);
    }
    if marker_inserted(text, &masked, "[IBAN]") {
        markers.push(PiiMarker::Iban);
    }
    if marker_inserted(text, &masked, "[API_KEY]") {
        markers.push(PiiMarker::ApiKey);
    }
    if marker_inserted(text, &masked, "[IP]") {
        markers.push(PiiMarker::Ip);
    }
    if marker_inserted(text, &masked, "[USER]") {
        markers.push(PiiMarker::UserPath);
    }
    if marker_inserted(text, &masked, "[PASSPORT]") {
        markers.push(PiiMarker::Passport);
    }

    markers
}

pub fn is_sensitive_segment_with_level(text: &str, level: PiiFilterLevel) -> bool {
    !detect_pii_markers_with_level(text, level).is_empty()
}

fn marker_inserted(original: &str, masked: &str, marker: &str) -> bool {
    masked.contains(marker) && (!original.contains(marker) || masked != original)
}

pub const SENSITIVE_APP_KEYWORDS: &[&str] = &[
    // Password managers
    "1password",
    "lastpass",
    "bitwarden",
    "dashlane",
    "keepass",
    "enpass",
    "nordpass",
    // Financial
    "bank",
    "banking",
    "wallet",
    "trading",
    "crypto",
    "coinbase",
    "binance",
    // 2FA / OTP apps
    "authenticator",
    "2fa",
    "otp",
    "authy",
    // Secrets storage
    "vault",
    "keychain",
    // Encrypted-messaging apps
    "signal",
    "telegram",
    "whatsapp",
    "threema",
    "session",
    "wire",
    // Encrypted email clients (desktop apps / standalone windows)
    "protonmail",
    "proton mail",
    "tutanota",
];

/// Window-title substrings that indicate a browser tab is showing a sensitive
/// webmail or security service.  Checked by [`should_exclude`] when
/// `auto_exclude_sensitive` is `true` and the *app* itself is not already
/// flagged (e.g. the app is "Chrome" or "Firefox").
///
/// All entries are lowercase; comparison is done after `to_lowercase()`.
pub const SENSITIVE_TITLE_SUBSTRINGS: &[&str] = &[
    "proton mail",
    "protonmail",
    "tutanota",
    "signal",
    "telegram web",
    "whatsapp web",
    "wire web",
    "keepass",
    "bitwarden",
    "1password",
    "lastpass",
    "authy",
];

pub fn is_sensitive_app(app_name: &str) -> bool {
    let lower = app_name.to_lowercase();
    SENSITIVE_APP_KEYWORDS.iter().any(|kw| lower.contains(kw))
}

pub fn matches_exclusion_pattern(app_name: &str, patterns: &[String]) -> bool {
    let lower = app_name.to_lowercase();
    patterns.iter().any(|pattern| {
        let pat = pattern.to_lowercase();
        if let Some(rest) = pat.strip_prefix('*') {
            if let Some(keyword) = rest.strip_suffix('*') {
                lower.contains(keyword)
            } else {
                lower.ends_with(rest)
            }
        } else if let Some(prefix) = pat.strip_suffix('*') {
            lower.starts_with(prefix)
        } else {
            lower == pat
        }
    })
}

pub fn should_exclude(
    app_name: &str,
    window_title: &str,
    excluded_apps: &[String],
    excluded_app_patterns: &[String],
    excluded_title_patterns: &[String],
    auto_exclude_sensitive: bool,
) -> bool {
    let lower = app_name.to_lowercase();
    if excluded_apps.iter().any(|a| a.to_lowercase() == lower) {
        return true;
    }

    if matches_exclusion_pattern(app_name, excluded_app_patterns) {
        return true;
    }

    if matches_exclusion_pattern(window_title, excluded_title_patterns) {
        return true;
    }

    if auto_exclude_sensitive && is_sensitive_app(app_name) {
        return true;
    }

    // Also exclude when a browser title reveals a sensitive webmail or
    // security service (e.g. "Proton Mail - Inbox — Mozilla Firefox").
    if auto_exclude_sensitive {
        let title_lower = window_title.to_lowercase();
        if SENSITIVE_TITLE_SUBSTRINGS
            .iter()
            .any(|kw| title_lower.contains(kw))
        {
            return true;
        }
    }

    false
}
