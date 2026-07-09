use maekon_core::config::PiiFilterLevel;
use maekon_core::ports::pii_sanitizer::PiiSanitizer;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// Event types are canonical in maekon-core; re-exported here.
pub use maekon_core::models::event::{ClipboardContentType, ClipboardEvent};

const CLIPBOARD_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

/// Clipboard change detection monitor.
///
/// Tracks clipboard text changes by hashing content (never stores raw text
/// for privacy). The monitor is designed to be wrapped in `Arc` and polled
/// from an async loop.
///
/// # Platform support
///
/// `poll_system_clipboard()` reads the system clipboard via platform commands:
/// - **macOS**: `pbpaste`
/// - **Linux**: `xclip -selection clipboard -o`
/// - **Windows**: PowerShell `Get-Clipboard`
///
/// If the clipboard read fails (e.g., no clipboard daemon on headless Linux),
/// the poll silently returns `None`.
///
/// # PII handling for the clipboard preview
///
/// Clipboard content is a high-risk PII source (passwords, API tokens, credit
/// cards, addresses are frequently copied). When `pii_level != Off`, a 50-char
/// `ClipboardEvent.preview` is produced and persisted to the LOCAL SQLite event
/// store. Clipboard events are fail-closed-excluded from server upload (see the
/// `src-tauri` scheduler config), so the preview never leaves the device — but it
/// is still readable by endpoint admins / backups, so it must not contain raw
/// secrets.
///
/// A preview is only produced when `pii_level != Off` AND the preview is not
/// suppressed (see layer 0). When produced, two layers sanitize it BEFORE
/// truncation, in order:
///
/// 0. [`ClipboardMonitor::with_preview_suppressed`] / [`set_preview_suppressed`]
///    (#7100): an opt-in kill switch. When enabled, no preview is generated at
///    any level — the `preview` field is always `None`. This is the only option
///    that closes the residual gap from layers 1+2: the entropy/prefix heuristic
///    (layer 1) deliberately cannot catch a low-entropy single-class passphrase
///    such as `correcthorsebatterystaple`, and the pattern sanitizer (layer 2)
///    has no pattern for it either. The knob defaults to OFF (opt-in) so the
///    default behavior — and all existing previews — are unchanged; a
///    privacy-maximizing deployment turns it on to drop previews entirely.
/// 1. [`redact_secrets`] (local, this module): a high-entropy + known-token-prefix
///    heuristic that runs at EVERY non-Off level. This is required because the
///    injected `PiiSanitizer` is pattern-only (email / phone / card / …): it
///    cannot recognize an arbitrary high-entropy password such as `xQ9!mB2#vL8z`,
///    and historically only masked API tokens (`sk-`, `ghp_`, `AKIA`, …) at the
///    `Strict` level — yet the shipped default is `Standard`. This heuristic
///    closes that gap so copied passwords / tokens are redacted at the default
///    level too. (#7100 also extended the layer-2 sanitizer to mask API tokens at
///    `Standard`, so the two layers now overlap on tokens at the default level.)
/// 2. The injected `Arc<dyn PiiSanitizer>` (production: `VisionPiiSanitizer`),
///    wired via [`ClipboardMonitor::with_pii_sanitizer`], which masks the
///    structured PII patterns it knows about.
///
/// When the sanitizer is `None` (tests / DI not wired), only layer 1 runs and the
/// result is truncated; production MUST wire the sanitizer. Earlier (pre-D5) the
/// preview stored raw truncated text without any filtering — that logic-inversion
/// bug is locked out by the regression tests below.
pub struct ClipboardMonitor {
    last_content_hash: Mutex<u64>,
    pii_filter_level: Mutex<PiiFilterLevel>,
    /// Cumulative clipboard change count since last `take_change_count()`.
    change_count: AtomicU32,
    /// D5 iter-2: injected sanitizer. Routes through `maekon-core::ports::pii_sanitizer::PiiSanitizer`
    /// so `maekon-monitor` doesn't need a direct `maekon-vision` dep (hexagonal arch rule).
    pii_sanitizer: Option<Arc<dyn PiiSanitizer>>,
    /// #7100: opt-in kill switch for the clipboard preview (layer 0 in the module
    /// doc). When `true`, `check_text_change` never emits a preview, regardless of
    /// `pii_filter_level`. Defaults to `false` so the existing preview contract is
    /// preserved. Atomic so it can be toggled at runtime without taking a lock.
    suppress_preview: AtomicBool,
}

impl ClipboardMonitor {
    pub fn new(pii_level: PiiFilterLevel) -> Self {
        Self {
            last_content_hash: Mutex::new(0),
            pii_filter_level: Mutex::new(pii_level),
            change_count: AtomicU32::new(0),
            pii_sanitizer: None,
            suppress_preview: AtomicBool::new(false),
        }
    }

    /// D5 iter-2: attach a `PiiSanitizer` implementation. At DI time,
    /// `src-tauri/src/main.rs` wires `Arc::new(maekon_vision::privacy::VisionPiiSanitizer)`.
    pub fn with_pii_sanitizer(mut self, sanitizer: Arc<dyn PiiSanitizer>) -> Self {
        self.pii_sanitizer = Some(sanitizer);
        self
    }

    /// #7100: builder for the preview kill switch (layer 0). When `suppress` is
    /// `true`, no clipboard preview is ever produced — the only option that closes
    /// the residual low-entropy-passphrase gap the sanitization layers cannot
    /// catch. Opt-in (default `false`) so the preview contract is unchanged.
    #[must_use]
    pub fn with_preview_suppressed(self, suppress: bool) -> Self {
        self.suppress_preview.store(suppress, Ordering::Relaxed);
        self
    }

    /// #7100: toggle the preview kill switch at runtime (e.g. from a settings
    /// change), mirroring [`set_pii_filter_level`](Self::set_pii_filter_level).
    pub fn set_preview_suppressed(&self, suppress: bool) {
        self.suppress_preview.store(suppress, Ordering::Relaxed);
    }

    /// Check if the given text represents a new clipboard value.
    /// Returns a `ClipboardEvent` if the content changed, `None` otherwise.
    ///
    /// P2 PR-A: `last_content_hash` mutex is held across the compare + mutate
    /// sequence (check-if-changed + update) as the atomicity guard. Tightening
    /// would allow two rapid clipboard changes to both emit events with the
    /// same hash.
    #[allow(clippy::significant_drop_tightening)]
    pub fn check_text_change(&self, text: &str) -> Option<ClipboardEvent> {
        let hash = hash_string(text);

        let mut last_hash = self.last_content_hash.lock().ok()?;
        if hash == *last_hash {
            return None;
        }
        *last_hash = hash;

        self.change_count.fetch_add(1, Ordering::Relaxed);

        let pii_level = self
            .pii_filter_level
            .lock()
            .map(|g| *g)
            .unwrap_or(PiiFilterLevel::Standard);

        // Sanitize BEFORE truncate, in two layers (see module doc). The original
        // pre-D5 code was a logic-inversion bug — `pii_level != Off` gated preview
        // generation but the non-Off branch stored raw truncated text without any
        // filter, leaking the first 50 chars of any clipboard content.
        //
        // #7100 layer 0: when the preview is suppressed, drop it entirely before
        // any sanitization runs — this is the only path that also covers
        // low-entropy single-class passphrases the heuristic/pattern layers miss.
        let preview =
            if pii_level != PiiFilterLevel::Off && !self.suppress_preview.load(Ordering::Relaxed) {
                // Layer 1: redact high-entropy secrets / known token prefixes that the
                // pattern-only sanitizer cannot catch, at every non-Off level.
                let secret_redacted = redact_secrets(text);
                // Layer 2: apply the injected pattern-based sanitizer (production:
                // VisionPiiSanitizer). Falls back to the layer-1 output when unwired.
                let sanitized = self
                    .pii_sanitizer
                    .as_ref()
                    .map(|s| s.sanitize_text(&secret_redacted, pii_level))
                    .unwrap_or(secret_redacted);
                Some(truncate(&sanitized, 50))
            } else {
                None
            };

        Some(ClipboardEvent {
            timestamp: chrono::Utc::now(),
            content_type: ClipboardContentType::Text,
            // `chars().count()`, not `len()`: `str::len()` is the UTF-8 byte length,
            // which over-reports 2-4x for CJK/emoji content for a field named char_count.
            char_count: text.chars().count(),
            preview,
        })
    }

    /// Poll the system clipboard for changes.
    ///
    /// Reads clipboard text via platform-native commands and checks whether
    /// it differs from the previously seen content. Returns `Some(event)`
    /// on change, `None` if unchanged or if the clipboard read fails.
    pub fn poll_system_clipboard(&self) -> Option<ClipboardEvent> {
        let text = read_system_clipboard()?;
        if text.is_empty() {
            return None;
        }
        self.check_text_change(&text)
    }

    pub fn set_pii_filter_level(&self, level: PiiFilterLevel) {
        if let Ok(mut pii) = self.pii_filter_level.lock() {
            *pii = level;
        }
    }

    /// Return the number of clipboard changes since the last call and reset
    /// the counter to zero. Designed for periodic snapshot collection.
    pub fn take_change_count(&self) -> u32 {
        self.change_count.swap(0, Ordering::Relaxed)
    }
}

/// Read clipboard text from the system clipboard using platform commands.
///
/// Every helper below is spawned via [`crate::trusted_binary::resolve_trusted_binary`]
/// (SEC-MON-01) rather than by bare name — a bare `Command::new("pbpaste")`
/// is PATH-resolved, so a same-named binary planted ahead of the trusted
/// system dirs on `$PATH` would hijack the spawn under this agent's
/// privilege. A resolution miss fails closed to `None`, the same outcome as
/// a command execution failure.
///
/// Returns `None` if the command fails, is not found under the trusted
/// allowlist, or produces no output.
fn read_system_clipboard() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let pbpaste_path = crate::trusted_binary::resolve_trusted_binary("pbpaste")?;
        let mut command = std::process::Command::new(pbpaste_path);
        let output = command_output_with_timeout(&mut command, CLIPBOARD_COMMAND_TIMEOUT)
            .ok()
            .flatten()?;
        if output.status.success() {
            Some(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            None
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Try xclip first, fall back to xsel
        let xclip_output =
            crate::trusted_binary::resolve_trusted_binary("xclip").and_then(|xclip_path| {
                let mut xclip = std::process::Command::new(xclip_path);
                xclip.args(["-selection", "clipboard", "-o"]);
                command_output_with_timeout(&mut xclip, CLIPBOARD_COMMAND_TIMEOUT)
                    .ok()
                    .flatten()
                    .filter(|output| output.status.success())
            });
        let output = xclip_output.or_else(|| {
            let xsel_path = crate::trusted_binary::resolve_trusted_binary("xsel")?;
            let mut xsel = std::process::Command::new(xsel_path);
            xsel.args(["--clipboard", "--output"]);
            command_output_with_timeout(&mut xsel, CLIPBOARD_COMMAND_TIMEOUT)
                .ok()
                .flatten()
                .filter(|output| output.status.success())
        })?;
        if output.status.success() {
            Some(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            None
        }
    }

    #[cfg(target_os = "windows")]
    {
        let powershell_path = crate::trusted_binary::resolve_trusted_binary("powershell")?;
        let mut command = std::process::Command::new(powershell_path);
        command.args(["-NoProfile", "-NonInteractive", "-Command", "Get-Clipboard"]);
        let output = command_output_with_timeout(&mut command, CLIPBOARD_COMMAND_TIMEOUT)
            .ok()
            .flatten()?;
        if output.status.success() {
            Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            None
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

fn command_output_with_timeout(
    command: &mut std::process::Command,
    timeout: Duration,
) -> std::io::Result<Option<std::process::Output>> {
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn()?;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let stdout_reader = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
        let mut buffer = Vec::new();
        if let Some(ref mut pipe) = stdout {
            pipe.read_to_end(&mut buffer)?;
        }
        Ok(buffer)
    });
    let stderr_reader = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
        let mut buffer = Vec::new();
        if let Some(ref mut pipe) = stderr {
            pipe.read_to_end(&mut buffer)?;
        }
        Ok(buffer)
    });

    let started_at = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            let stdout = stdout_reader
                .join()
                .unwrap_or_else(|_| Err(std::io::Error::other("stdout reader panicked")))?;
            let stderr = stderr_reader
                .join()
                .unwrap_or_else(|_| Err(std::io::Error::other("stderr reader panicked")))?;
            return Ok(Some(std::process::Output {
                status,
                stdout,
                stderr,
            }));
        }

        if started_at.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Ok(None);
        }

        std::thread::sleep(Duration::from_millis(10));
    }
}

fn hash_string(s: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let mut result: String = s.chars().take(max_len).collect();
        result.push_str("...");
        result
    }
}

/// Redaction marker substituted for detected secret tokens in the preview.
const SECRET_MARKER: &str = "[REDACTED_SECRET]";

/// Known credential token prefixes. A whitespace-delimited token starting with
/// one of these (and long enough) is treated as a secret. The cased entries
/// (`AKIA`, `ghp_`, `xoxb-`, …) are matched case-sensitively, mirroring the real
/// token formats so ordinary words like "skill" or "apiserver" are not redacted.
const SECRET_TOKEN_PREFIXES: [&str; 17] = [
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
    "xoxa-",
    "xoxr-",
];

/// Minimum token length for the prefix and high-entropy heuristics. Avoids
/// redacting short false positives (a bare `sk-`, `api_`, or a short word).
const MIN_SECRET_LEN: usize = 12;

/// Minimum Shannon entropy (bits/char) for the high-entropy password heuristic.
/// A 12-char string of distinct characters has ~3.58 bits/char; ordinary words
/// fall well below this once combined with the character-class requirement.
const MIN_SECRET_ENTROPY_BITS: f64 = 3.0;

/// Minimum distinct character classes (lowercase / uppercase / digit / symbol)
/// for the high-entropy heuristic. Prose words rarely mix three or more classes,
/// which keeps the false-positive rate low.
const MIN_SECRET_CHAR_CLASSES: u32 = 3;

/// Redact copied secrets (generic high-entropy passwords and known credential
/// tokens) from `text` before it is truncated into the clipboard preview.
///
/// The wired `PiiSanitizer` is pattern-only and cannot catch arbitrary passwords
/// or — at non-Strict levels — API tokens, so this local heuristic runs first at
/// every non-Off level. Whitespace is preserved so the downstream pattern
/// sanitizer still sees the original structure of the remaining text.
fn redact_secrets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut token = String::new();
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !token.is_empty() {
                out.push_str(&redact_secret_token(&token));
                token.clear();
            }
            out.push(ch);
        } else {
            token.push(ch);
        }
    }
    if !token.is_empty() {
        out.push_str(&redact_secret_token(&token));
    }
    out
}

/// Classify a single whitespace-delimited token, returning the redaction marker
/// when it looks like a secret and the original token otherwise.
fn redact_secret_token(token: &str) -> String {
    if token.chars().count() >= MIN_SECRET_LEN
        && SECRET_TOKEN_PREFIXES
            .iter()
            .any(|prefix| token.starts_with(prefix))
    {
        return SECRET_MARKER.to_string();
    }
    if looks_like_high_entropy_secret(token) {
        return SECRET_MARKER.to_string();
    }
    token.to_string()
}

/// Heuristic for a randomly-generated credential: long enough, mixing several
/// character classes, and with high per-character Shannon entropy.
fn looks_like_high_entropy_secret(token: &str) -> bool {
    token.chars().count() >= MIN_SECRET_LEN
        && character_class_count(token) >= MIN_SECRET_CHAR_CLASSES
        && shannon_entropy_bits(token) >= MIN_SECRET_ENTROPY_BITS
}

/// Count the distinct character classes present in `token`: lowercase,
/// uppercase, digit, and other (symbol / punctuation).
fn character_class_count(token: &str) -> u32 {
    let mut lower = false;
    let mut upper = false;
    let mut digit = false;
    let mut symbol = false;
    for ch in token.chars() {
        if ch.is_lowercase() {
            lower = true;
        } else if ch.is_uppercase() {
            upper = true;
        } else if ch.is_numeric() {
            digit = true;
        } else {
            symbol = true;
        }
    }
    u32::from(lower) + u32::from(upper) + u32::from(digit) + u32::from(symbol)
}

/// Shannon entropy of `token` in bits per character.
fn shannon_entropy_bits(token: &str) -> f64 {
    let chars: Vec<char> = token.chars().collect();
    let len = chars.len();
    if len == 0 {
        return 0.0;
    }
    let mut counts: std::collections::HashMap<char, usize> = std::collections::HashMap::new();
    for ch in &chars {
        *counts.entry(*ch).or_insert(0) += 1;
    }
    let len_f = len as f64;
    counts
        .values()
        .map(|&count| {
            let p = count as f64 / len_f;
            -p * p.log2()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_text_change() {
        let monitor = ClipboardMonitor::new(PiiFilterLevel::Standard);
        let event = monitor.check_text_change("hello world");
        assert!(event.is_some());
        let evt = event.unwrap();
        assert_eq!(evt.content_type, ClipboardContentType::Text);
        assert_eq!(evt.char_count, 11);
    }

    #[test]
    fn no_change_on_same_text() {
        let monitor = ClipboardMonitor::new(PiiFilterLevel::Standard);
        monitor.check_text_change("hello");
        let event = monitor.check_text_change("hello");
        assert!(event.is_none());
    }

    #[test]
    fn preview_included_with_filter() {
        let monitor = ClipboardMonitor::new(PiiFilterLevel::Standard);
        let event = monitor.check_text_change("short").unwrap();
        assert!(event.preview.is_some());
    }

    #[test]
    fn no_preview_when_off() {
        let monitor = ClipboardMonitor::new(PiiFilterLevel::Off);
        let event = monitor.check_text_change("something").unwrap();
        assert!(event.preview.is_none());
    }

    #[test]
    fn change_count_increments() {
        let monitor = ClipboardMonitor::new(PiiFilterLevel::Standard);
        monitor.check_text_change("first");
        monitor.check_text_change("second");
        monitor.check_text_change("second"); // duplicate — no increment
        assert_eq!(monitor.take_change_count(), 2);
        assert_eq!(monitor.take_change_count(), 0); // reset after take
    }

    #[test]
    fn set_pii_level_changes_preview() {
        let monitor = ClipboardMonitor::new(PiiFilterLevel::Off);
        let event = monitor.check_text_change("private data").unwrap();
        assert!(event.preview.is_none());

        monitor.set_pii_filter_level(PiiFilterLevel::Standard);
        let event = monitor.check_text_change("more data").unwrap();
        assert!(event.preview.is_some());
    }

    // ── D5 iter-2 CRITICAL regression tests ──────────────────────────────
    //
    // Prior to D5 iter-2, clipboard preview stored raw truncated text when
    // pii_level != Off, leaking PII (passwords, cards, addresses) through
    // ClipboardEvent → SQLite → server upload. These tests lock the fix.

    use maekon_core::config::PiiFilterLevel as P;
    use maekon_core::ports::pii_sanitizer::PiiSanitizer as S;

    /// Minimal in-test sanitizer — applies [EMAIL] + [API_KEY] markers so we
    /// can verify the injection path without depending on maekon-vision
    /// (adapter crate can't import it per hexagonal arch rule).
    struct TestSanitizer;

    impl S for TestSanitizer {
        fn sanitize_text(&self, text: &str, _level: P) -> String {
            let mut out = text.to_string();
            // crude email matcher (good enough for the test fixture)
            if let Some(at) = out.find('@') {
                let start = out[..at]
                    .rfind(|c: char| !c.is_alphanumeric() && c != '.' && c != '_')
                    .map_or(0, |i| i + 1);
                if let Some(rel_end) = out[at..].find(|c: char| c.is_whitespace()) {
                    let end = at + rel_end;
                    out.replace_range(start..end, "[EMAIL]");
                } else {
                    out.replace_range(start.., "[EMAIL]");
                }
            }
            if out.contains("sk-") {
                // crude api-key token matcher
                out = out.replace("sk-ABCD1234EFGH5678IJKL9012MNOP3456", "[API_KEY]");
            }
            out
        }
    }

    #[test]
    fn d5_preview_sanitizes_email_when_level_is_basic_with_sanitizer() {
        let monitor = ClipboardMonitor::new(P::Basic).with_pii_sanitizer(Arc::new(TestSanitizer));
        let event = monitor
            .check_text_change("Contact user@example.com for details")
            .expect("event should fire on first change");
        let preview = event.preview.expect("Basic level should produce preview");
        assert!(
            !preview.contains("user@example.com"),
            "email leaked in preview"
        );
        assert!(
            preview.contains("[EMAIL]"),
            "expected [EMAIL] marker in preview"
        );
    }

    #[test]
    fn d5_preview_sanitizes_api_key_when_level_is_strict_with_sanitizer() {
        let monitor = ClipboardMonitor::new(P::Strict).with_pii_sanitizer(Arc::new(TestSanitizer));
        let event = monitor
            .check_text_change("TOKEN: sk-ABCD1234EFGH5678IJKL9012MNOP3456")
            .expect("event should fire");
        let preview = event.preview.expect("Strict level should produce preview");
        assert!(!preview.contains("sk-ABCD1234"), "API key leaked");
        // The local secret heuristic (layer 1) now redacts `sk-` tokens before the
        // injected pattern sanitizer runs, so either redaction marker is acceptable.
        assert!(
            preview.contains("[REDACTED_SECRET]") || preview.contains("[API_KEY]"),
            "expected an API-key redaction marker, got: {preview}"
        );
    }

    #[test]
    fn d5_preview_omitted_when_level_is_off() {
        let monitor = ClipboardMonitor::new(P::Off).with_pii_sanitizer(Arc::new(TestSanitizer));
        let event = monitor.check_text_change("anything").expect("event fires");
        // Off level: preview is None (entire preview field suppressed).
        assert!(event.preview.is_none(), "Off level should suppress preview");
    }

    #[test]
    fn d5_preview_without_sanitizer_falls_back_to_raw_truncate() {
        // Regression guard for the fallback path: when no sanitizer is injected
        // (test fixtures / pre-DI state), the monitor still produces a preview
        // by falling back to raw truncation. Production MUST wire the sanitizer.
        let monitor = ClipboardMonitor::new(P::Basic);
        let event = monitor
            .check_text_change("plain text that fits")
            .expect("event fires");
        let preview = event.preview.expect("Basic level produces preview");
        assert_eq!(preview, "plain text that fits");
    }

    // ── #7072 secret/entropy heuristic regression tests ─────────────────
    //
    // The wired sanitizer is pattern-only: it cannot catch arbitrary high-entropy
    // passwords at any level and only masks API tokens at Strict, yet the shipped
    // default is Standard. These tests lock the local `redact_secrets` layer that
    // runs at every non-Off level, BEFORE the pattern sanitizer. They wire NO
    // sanitizer so they isolate layer 1 (before #7072 the preview stored the raw
    // truncated secret in this configuration).

    #[test]
    fn redacts_generic_high_entropy_password_at_standard_default_level() {
        let monitor = ClipboardMonitor::new(P::Standard);
        let event = monitor
            .check_text_change("login pw: xQ9!mB2#vL8z next")
            .expect("event fires");
        let preview = event.preview.expect("Standard level produces preview");
        assert!(
            !preview.contains("xQ9!mB2#vL8z"),
            "generic password leaked into preview: {preview}"
        );
        assert!(
            preview.contains(SECRET_MARKER),
            "expected redaction marker, got: {preview}"
        );
    }

    #[test]
    fn redacts_api_token_prefix_at_standard_default_level() {
        let monitor = ClipboardMonitor::new(P::Standard);
        let event = monitor
            .check_text_change("token sk-ABCD1234EFGH5678IJKL done")
            .expect("event fires");
        let preview = event.preview.expect("preview present");
        assert!(
            !preview.contains("sk-ABCD1234"),
            "API token leaked into preview at default level: {preview}"
        );
        assert!(preview.contains(SECRET_MARKER));
    }

    #[test]
    fn redacts_known_token_prefix_even_when_low_entropy() {
        // A `ghp_` token of repeated chars has low entropy + only two character
        // classes, so the entropy path skips it — the prefix path must still catch it.
        assert_eq!(
            redact_secret_token("ghp_aaaaaaaaaaaaaaaaaa"),
            SECRET_MARKER,
            "low-entropy ghp_ token must be redacted by the prefix heuristic"
        );
        assert_eq!(
            redact_secret_token("AKIAIOSFODNN7EXAMPLE"),
            SECRET_MARKER,
            "AWS access-key id must be redacted (case-sensitive AKIA prefix)"
        );
    }

    #[test]
    fn high_entropy_secret_heuristic_flags_random_token_only() {
        // Random mixed-class token → secret.
        assert!(looks_like_high_entropy_secret("xQ9!mB2#vL8z"));
        // Ordinary long single-class word → not a secret (too few classes).
        assert!(!looks_like_high_entropy_secret("internationalization"));
        // Short token → not a secret regardless of class mix.
        assert!(!looks_like_high_entropy_secret("aB3$"));
    }

    #[test]
    fn redact_secrets_preserves_ordinary_text_and_whitespace() {
        let input = "Meeting at noon tomorrow with the team";
        assert_eq!(
            redact_secrets(input),
            input,
            "ordinary prose must pass through unchanged"
        );
    }

    #[test]
    fn truncate_counts_unicode_scalars_not_utf8_bytes() {
        let two_chars = "\u{D55C}\u{AE00}";
        assert_eq!(truncate(two_chars, 2), two_chars);
        assert_eq!(
            truncate("\u{D55C}\u{AE00}\u{D55C}", 2),
            format!("{two_chars}...")
        );
    }

    // ── #7100 suppress-clipboard-preview knob regression tests ───────────
    //
    // The #7072 entropy/prefix heuristic (layer 1) deliberately does NOT catch a
    // low-entropy single-class passphrase (e.g. `correcthorsebatterystaple`), and
    // the pattern sanitizer (layer 2) has no pattern for it either. The opt-in
    // preview kill switch (layer 0) is the only path that closes that residual by
    // dropping the preview entirely. These tests prove the residual and lock the
    // knob. The knob is opt-in, so default construction must keep the preview.

    #[test]
    fn low_entropy_passphrase_survives_sanitization_layers() {
        // Characterization of the residual the knob exists to close: with the
        // preview ENABLED (default), a single-class passphrase leaks into the
        // preview because neither the entropy heuristic (only one character class,
        // so it is skipped) nor the pattern sanitizer recognizes it.
        let monitor = ClipboardMonitor::new(P::Standard);
        let event = monitor
            .check_text_change("correcthorsebatterystaple")
            .expect("event fires");
        let preview = event.preview.expect("Standard level produces preview");
        assert!(
            preview.contains("correcthorsebatterystaple"),
            "expected the residual leak this knob closes, got: {preview}"
        );
    }

    #[test]
    fn suppress_preview_drops_low_entropy_passphrase() {
        // #7100: with the kill switch on, the same passphrase yields no preview.
        let monitor = ClipboardMonitor::new(P::Standard).with_preview_suppressed(true);
        let event = monitor
            .check_text_change("correcthorsebatterystaple")
            .expect("event fires");
        assert_eq!(
            event.preview, None,
            "preview must be suppressed entirely when the knob is on"
        );
    }

    #[test]
    fn suppress_preview_overrides_non_off_level() {
        // #7100: suppression wins over the pii_level gate at Strict too.
        let monitor = ClipboardMonitor::new(P::Strict)
            .with_pii_sanitizer(Arc::new(TestSanitizer))
            .with_preview_suppressed(true);
        let event = monitor
            .check_text_change("Contact user@example.com for details")
            .expect("event fires");
        assert_eq!(
            event.preview, None,
            "suppression must override Strict level"
        );
    }

    #[test]
    fn preview_not_suppressed_by_default() {
        // #7100 guard: the knob is opt-in — default construction keeps the preview.
        let monitor = ClipboardMonitor::new(P::Standard);
        let event = monitor
            .check_text_change("ordinary note")
            .expect("event fires");
        assert!(
            event.preview.is_some(),
            "default construction must preserve the preview contract"
        );
    }

    #[test]
    fn set_preview_suppressed_toggles_at_runtime() {
        // #7100: runtime setter mirrors set_pii_filter_level.
        let monitor = ClipboardMonitor::new(P::Standard);
        let first = monitor
            .check_text_change("first note")
            .expect("event fires");
        assert!(first.preview.is_some(), "preview on before toggle");

        monitor.set_preview_suppressed(true);
        let second = monitor
            .check_text_change("second note")
            .expect("event fires");
        assert_eq!(second.preview, None, "preview off after toggle on");

        monitor.set_preview_suppressed(false);
        let third = monitor
            .check_text_change("third note")
            .expect("event fires");
        assert!(third.preview.is_some(), "preview returns after toggle off");
    }

    #[test]
    fn command_timeout_helper_returns_fast_output() {
        let mut command = fast_output_command();
        let output = command_output_with_timeout(&mut command, std::time::Duration::from_secs(2))
            .expect("command should launch")
            .expect("fast command should complete before timeout");

        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("ok"));
    }

    #[test]
    fn command_timeout_helper_kills_hung_process() {
        let mut command = slow_command();
        let output =
            command_output_with_timeout(&mut command, std::time::Duration::from_millis(50))
                .expect("command should launch");

        assert!(output.is_none(), "hung command should time out");
    }

    #[cfg(target_os = "windows")]
    fn fast_output_command() -> std::process::Command {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "echo ok"]);
        command
    }

    #[cfg(not(target_os = "windows"))]
    fn fast_output_command() -> std::process::Command {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "printf ok"]);
        command
    }

    #[cfg(target_os = "windows")]
    fn slow_command() -> std::process::Command {
        let mut command = std::process::Command::new("powershell");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Sleep -Milliseconds 500",
        ]);
        command
    }

    #[cfg(not(target_os = "windows"))]
    fn slow_command() -> std::process::Command {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "sleep 1"]);
        command
    }
}
