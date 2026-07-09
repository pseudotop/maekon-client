use maekon_core::config::{PiiFilterLevel, PrivacyConfig};

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
            // API tokens (sk-/ghp_/AKIA/Bearer/PEM blocks, …) are a high-risk
            // credential class that commonly leaks through window titles and OCR
            // text. They are masked at Standard — the shipped default level — and
            // not only at Strict, because `mask_api_keys` is prefix-anchored and
            // gated on an >=8-char token, so it does not over-mask ordinary prose.
            // IP addresses and passport numbers stay Strict-only (higher
            // false-positive risk, lower secrecy stakes).
            result = mask_api_keys(&result);
            result
        }
        PiiFilterLevel::Strict => {
            // Standard already masks API keys; Strict adds IP + passport on top.
            let mut result = sanitize_title_with_level(title, PiiFilterLevel::Standard);
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

const SENSITIVE_APP_PRODUCT_NAMES: &[&str] = &[
    // Password managers and encrypted notes outside the original seed list.
    "keeper",
    "keeper security",
    "roboform",
    "sticky password",
    "password safe",
    "strongbox",
    "standard notes",
    "standardnotes",
    // Financial, brokerage, and payment apps that do not necessarily contain
    // the generic bank/trading/crypto keywords.
    "robinhood",
    "fidelity",
    "vanguard",
    "charles schwab",
    "schwab",
    "etrade",
    "e trade",
    "interactive brokers",
    "ibkr",
    "webull",
    "wealthfront",
    "betterment",
    "sofi",
    "paypal",
    "venmo",
    "cash app",
    // Health, legal, and document-signing tools commonly used for sensitive
    // records.
    "epic hyperspace",
    "epic haiku",
    "epic canto",
    "cerner",
    "oracle health",
    "athenahealth",
    "mychart",
    "doxy me",
    "doximity",
    "simplepractice",
    "clio",
    "mycase",
    "rocket matter",
    "netdocuments",
    "docusign",
];

const SENSITIVE_APP_CATEGORY_PHRASES: &[&str] = &[
    "password manager",
    "secure notes",
    "credit union",
    "investment account",
    "brokerage account",
    "wealth management",
    "retirement account",
    "medical record",
    "health record",
    "patient chart",
    "electronic health record",
    "medical chart",
    "tax prep",
    "tax filing",
    "payroll portal",
    "law practice",
    "legal case",
];

const SENSITIVE_APP_CATEGORY_TOKENS: &[&str] = &[
    "emr",
    "ehr",
    "hipaa",
    "patient",
    "clinic",
    "hospital",
    "medical",
    "health",
    "brokerage",
    "broker",
    "investment",
    "wealth",
    "retirement",
    "payroll",
    "tax",
    "legal",
    "attorney",
    "notary",
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
    if SENSITIVE_APP_KEYWORDS.iter().any(|kw| lower.contains(kw)) {
        return true;
    }

    let normalized = normalize_app_name(app_name);
    if normalized.is_empty() {
        return false;
    }
    let compact = compact_app_name(&normalized);

    if SENSITIVE_APP_PRODUCT_NAMES.iter().any(|name| {
        normalized_phrase_matches(&normalized, name) || compact_product_matches(&compact, name)
    }) {
        return true;
    }

    if SENSITIVE_APP_CATEGORY_PHRASES
        .iter()
        .any(|phrase| normalized_phrase_matches(&normalized, phrase))
    {
        return true;
    }

    let tokens: Vec<&str> = normalized.split_whitespace().collect();
    SENSITIVE_APP_CATEGORY_TOKENS
        .iter()
        .any(|category| tokens.contains(category))
}

fn normalize_app_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn compact_app_name(value: &str) -> String {
    normalize_app_name(value).replace(' ', "")
}

fn compact_product_matches(compact_name: &str, product_name: &str) -> bool {
    let normalized_product = normalize_app_name(product_name);
    if normalized_product.split_whitespace().count() < 2 {
        return false;
    }

    compact_name.contains(&compact_app_name(&normalized_product))
}

fn normalized_phrase_matches(normalized_app_name: &str, phrase: &str) -> bool {
    let normalized_phrase = normalize_app_name(phrase);
    if normalized_phrase.is_empty() {
        return false;
    }
    if normalized_app_name == normalized_phrase {
        return true;
    }

    normalized_app_name
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(normalized_phrase.split_whitespace().count())
        .any(|window| window.join(" ") == normalized_phrase)
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

/// Returns `true` when a browser/window *title* reveals a sensitive service
/// even though the host *app* (e.g. "Google Chrome") is not itself flagged.
///
/// Uses high-precision detection only, because titles are free-form prose:
/// - distinctive coined service names (`SENSITIVE_TITLE_SUBSTRINGS`,
///   `coinbase`/`binance`) matched as substrings;
/// - product names ([`SENSITIVE_APP_PRODUCT_NAMES`]) and category *phrases*
///   ([`SENSITIVE_APP_CATEGORY_PHRASES`]) matched **whole-word** only.
///
/// It deliberately omits, versus [`is_sensitive_app`]:
/// - [`SENSITIVE_APP_CATEGORY_TOKENS`] and generic [`SENSITIVE_APP_KEYWORDS`]
///   (bare "tax"/"legal"/"bank"/"session" collide with ordinary titles), and
/// - `compact_product_matches` (its space-removed substring match collides with
///   ordinary titles, e.g. "e trade" → "etrade" would hit "live trade room").
///
/// Common-word product names (e.g. "fidelity") still match whole-word and may
/// over-exclude a title that merely uses the word — acceptable for a privacy
/// feature where over-exclusion never leaks (#6826).
pub fn title_reveals_sensitive_service(window_title: &str) -> bool {
    let title_lower = window_title.to_lowercase();

    // Webmail / security / messaging services (original set).
    if SENSITIVE_TITLE_SUBSTRINGS
        .iter()
        .any(|kw| title_lower.contains(kw))
    {
        return true;
    }

    // Distinctive finance service names that are safe as raw substrings.
    if ["coinbase", "binance"]
        .iter()
        .any(|kw| title_lower.contains(kw))
    {
        return true;
    }

    // Brokerage / health / legal / payment product names and category phrases,
    // matched WHOLE-WORD (no compact-substring path) to keep titles precise.
    let normalized = normalize_app_name(window_title);
    if normalized.is_empty() {
        return false;
    }

    if SENSITIVE_APP_PRODUCT_NAMES
        .iter()
        .any(|name| normalized_phrase_matches(&normalized, name))
    {
        return true;
    }

    SENSITIVE_APP_CATEGORY_PHRASES
        .iter()
        .any(|phrase| normalized_phrase_matches(&normalized, phrase))
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

    // Also exclude when a browser title reveals a sensitive webmail, security,
    // finance, health, or legal service even though the host app (e.g. Chrome)
    // is not itself flagged (#6826).
    if auto_exclude_sensitive && title_reveals_sensitive_service(window_title) {
        return true;
    }

    false
}

/// Convenience wrapper over [`should_exclude`] taking the whole
/// [`PrivacyConfig`].
///
/// Every boundary that enforces the exclusion policy (capture-time gate,
/// egress upload, external OCR, external LLM) must resolve the same five
/// config fields in the same order; routing them all through this single
/// seam keeps the wiring from drifting between call sites (#7909).
pub fn should_exclude_by_policy(
    privacy: &PrivacyConfig,
    app_name: &str,
    window_title: &str,
) -> bool {
    should_exclude(
        app_name,
        window_title,
        &privacy.excluded_apps,
        &privacy.excluded_app_patterns,
        &privacy.excluded_title_patterns,
        privacy.auto_exclude_sensitive,
    )
}
