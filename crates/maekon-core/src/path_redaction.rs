//! Canonical user-path redaction.
//!
//! Replaces OS-specific user-home path prefixes (`/Users/<user>/`,
//! `/home/<user>/`, `C:\Users\<user>\`) with a `[USER]` placeholder so that
//! local usernames are never egressed in telemetry, logs, or process metadata.
//!
//! This is the single source of truth for username masking. Both
//! `maekon-monitor` (process executable paths) and `maekon-vision`
//! (PII redaction pipeline) delegate here so the behaviour cannot drift apart.
//! `maekon-monitor` cannot depend on `maekon-vision` (hexagonal rule), so the
//! shared implementation lives in `maekon-core`.

/// Mask user-home path prefixes on all three platforms, replacing the username
/// segment with `[USER]` while preserving the surrounding text and prefix.
///
/// Cross-platform: a single call handles `/Users/`, `/home/`, and `C:\Users\`
/// so the caller does not need to branch on `target_os`. Already-masked text is
/// left untouched (idempotent), and multiple occurrences are all masked.
///
/// # Examples
///
/// ```
/// use maekon_core::path_redaction::mask_user_paths;
///
/// assert_eq!(mask_user_paths("/home/alice/bin/app"), "/home/[USER]/bin/app");
/// assert_eq!(mask_user_paths("/Users/bob/file"), "/Users/[USER]/file");
/// assert_eq!(mask_user_paths(r"C:\Users\carol\app.exe"), r"C:\Users\[USER]\app.exe");
/// ```
#[must_use]
pub fn mask_user_paths(text: &str) -> String {
    // Unix home prefixes are matched CASE-INSENSITIVELY because macOS APFS is
    // case-preserving but case-INsensitive by default, so a process/telemetry
    // path may surface as lowercased `/users/alice/...` even though the on-disk
    // segment is `/Users/`. A case-sensitive `/Users/` pass would then leak the
    // username unmasked (#6173). `/home/` is matched the same way for
    // consistency and to cover case-insensitive Linux mounts (e.g. ext4
    // casefold); the case-insensitive branch is byte-length-preserving and
    // panic-safe (proven by the `\Users\` sibling below), and the ORIGINAL-case
    // prefix is re-emitted from the source text so output case is preserved.
    let result = mask_user_paths_os(text, "/Users/", '/', true);
    let result = mask_user_paths_os(&result, "/home/", '/', true);
    // Windows: match `\Users\` DRIVE-AGNOSTICALLY (C:, D:, relocated profiles, UNC
    // `\\server\Users\`) and CASE-INSENSITIVELY (NTFS is case-insensitive), so a
    // non-C-drive process path like `D:\Users\bob\app.exe` is still masked.
    // (#6100 regression: the prior masker only matched the literal `C:\Users\`.)
    mask_user_paths_os(&result, r"\Users\", '\\', true)
}

/// Replace a single user-path prefix's username segment with `[USER]`, tracking
/// the offset to avoid re-matching replacement text. When `case_insensitive` is
/// set, matching ignores ASCII case (the original case of the prefix is preserved
/// in the output).
fn mask_user_paths_os(text: &str, prefix: &str, sep: char, case_insensitive: bool) -> String {
    let prefix_len = prefix.len();
    // For case-insensitive matching, search a lowercased copy. ASCII lowercasing
    // never changes byte length, so byte offsets map 1:1 back onto `text`.
    let hay_lower;
    let needle_lower;
    let (hay, needle): (&str, &str) = if case_insensitive {
        hay_lower = text.to_ascii_lowercase();
        needle_lower = prefix.to_ascii_lowercase();
        (hay_lower.as_str(), needle_lower.as_str())
    } else {
        (text, prefix)
    };

    let mut result = String::with_capacity(text.len());
    let mut search_from = 0;

    while search_from < text.len() {
        match hay[search_from..].find(needle) {
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
                // Emit text before the match + the ORIGINAL-case prefix + mask.
                result.push_str(&text[search_from..abs_start]);
                result.push_str(&text[abs_start..abs_start + prefix_len]);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_linux_home_path() {
        // Regression: Linux `/home/<user>/...` paths previously leaked usernames
        // unredacted from process telemetry (2nd-pass #61).
        assert_eq!(
            mask_user_paths("/home/alice/bin/app"),
            "/home/[USER]/bin/app"
        );
    }

    #[test]
    fn masks_macos_users_path() {
        assert_eq!(
            mask_user_paths("/Users/bob/Applications/X.app"),
            "/Users/[USER]/Applications/X.app"
        );
    }

    #[test]
    fn masks_windows_users_path() {
        assert_eq!(
            mask_user_paths(r"C:\Users\carol\app.exe"),
            r"C:\Users\[USER]\app.exe"
        );
    }

    #[test]
    fn masks_windows_users_path_any_drive() {
        // #6100 regression: drive-agnostic — non-C drives / relocated profiles
        // must also be masked (the prior masker only matched literal `C:\Users\`).
        assert_eq!(
            mask_user_paths(r"D:\Users\bob\app.exe"),
            r"D:\Users\[USER]\app.exe"
        );
        // UNC profile path.
        assert_eq!(
            mask_user_paths(r"\\server\Users\dave\x.exe"),
            r"\\server\Users\[USER]\x.exe"
        );
    }

    #[test]
    fn masks_windows_users_path_case_insensitive() {
        // NTFS is case-insensitive; a lowercased `\users\` must still be masked,
        // with the original case preserved in the output.
        assert_eq!(
            mask_user_paths(r"c:\users\erin\app.exe"),
            r"c:\users\[USER]\app.exe"
        );
    }

    #[test]
    fn masks_unix_users_path_case_insensitive() {
        // #6173 regression: macOS APFS is case-preserving but case-INsensitive,
        // so a path may surface lowercased as `/users/alice/...`. The masker must
        // redact it, and must also redact a mixed-case `/Users/Alice/...`, with
        // the original prefix case preserved in the output.
        assert_eq!(
            mask_user_paths("/users/alice/secret"),
            "/users/[USER]/secret"
        );
        assert_eq!(
            mask_user_paths("/Users/Alice/secret"),
            "/Users/[USER]/secret"
        );
    }

    #[test]
    fn masks_linux_home_path_case_insensitive() {
        // #6173: `/home/` is masked case-insensitively too (case-insensitive
        // Linux mounts), preserving the original prefix case.
        assert_eq!(mask_user_paths("/HOME/bob/bin/app"), "/HOME/[USER]/bin/app");
    }

    #[test]
    fn masks_prefix_with_no_trailing_separator() {
        assert_eq!(mask_user_paths("/Users/alice"), "/Users/[USER]");
        assert_eq!(mask_user_paths("/home/bob"), "/home/[USER]");
        assert_eq!(mask_user_paths(r"C:\Users\carol"), r"C:\Users\[USER]");
    }

    #[test]
    fn masks_prefix_followed_by_space() {
        assert_eq!(
            mask_user_paths("path /Users/alice next"),
            "path /Users/[USER] next"
        );
    }

    #[test]
    fn masks_multiple_occurrences() {
        assert_eq!(
            mask_user_paths("/Users/alice/docs and /Users/bob/file"),
            "/Users/[USER]/docs and /Users/[USER]/file"
        );
    }

    #[test]
    fn idempotent_on_already_masked() {
        let input = "/Users/[USER]/already-masked";
        assert_eq!(mask_user_paths(input), input);
    }

    #[test]
    fn non_user_path_is_unchanged() {
        // Linux system binaries (no home prefix) must pass through verbatim.
        assert_eq!(mask_user_paths("/usr/bin/firefox"), "/usr/bin/firefox");
    }
}
