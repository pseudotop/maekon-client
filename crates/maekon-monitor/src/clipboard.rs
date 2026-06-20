use maekon_core::config::PiiFilterLevel;
use maekon_core::ports::pii_sanitizer::PiiSanitizer;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::sync::atomic::{AtomicU32, Ordering};
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
/// # D5 PII sanitization (iter-2, CRITICAL fix)
///
/// Clipboard content is a high-risk PII source (passwords, credit cards, addresses
/// frequently copied). Prior to D5 iter-2, the preview field was populated via
/// `truncate(text, 50)` when `pii_level != Off` — but WITHOUT applying the PII
/// filter. Effect: first 50 chars of ANY clipboard content leaked raw through
/// `ClipboardEvent.preview` → SQLite → server upload.
///
/// Fix: inject `Arc<dyn PiiSanitizer>` via `with_pii_sanitizer` and apply it
/// BEFORE truncation. When the sanitizer is `None` (tests / DI not wired), the
/// monitor falls back to raw truncation to preserve existing behavior for
/// test fixtures that explicitly want it.
pub struct ClipboardMonitor {
    last_content_hash: Mutex<u64>,
    pii_filter_level: Mutex<PiiFilterLevel>,
    /// Cumulative clipboard change count since last `take_change_count()`.
    change_count: AtomicU32,
    /// D5 iter-2: injected sanitizer. Routes through `maekon-core::ports::pii_sanitizer::PiiSanitizer`
    /// so `maekon-monitor` doesn't need a direct `maekon-vision` dep (hexagonal arch rule).
    pii_sanitizer: Option<Arc<dyn PiiSanitizer>>,
}

impl ClipboardMonitor {
    pub fn new(pii_level: PiiFilterLevel) -> Self {
        Self {
            last_content_hash: Mutex::new(0),
            pii_filter_level: Mutex::new(pii_level),
            change_count: AtomicU32::new(0),
            pii_sanitizer: None,
        }
    }

    /// D5 iter-2: attach a `PiiSanitizer` implementation. At DI time,
    /// `src-tauri/src/main.rs` wires `Arc::new(maekon_vision::privacy::VisionPiiSanitizer)`.
    pub fn with_pii_sanitizer(mut self, sanitizer: Arc<dyn PiiSanitizer>) -> Self {
        self.pii_sanitizer = Some(sanitizer);
        self
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

        // D5 iter-2 CRITICAL FIX: sanitize BEFORE truncate. The previous code
        // was a logic-inversion bug — `pii_level != Off` gated preview generation
        // but the non-Off branch stored raw truncated text without applying the
        // filter, leaking the first 50 chars of any clipboard content.
        let preview = if pii_level != PiiFilterLevel::Off {
            let sanitized = self
                .pii_sanitizer
                .as_ref()
                .map(|s| s.sanitize_text(text, pii_level))
                .unwrap_or_else(|| text.to_string());
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
/// Returns `None` if the command fails or produces no output.
fn read_system_clipboard() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let mut command = std::process::Command::new("pbpaste");
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
        let mut xclip = std::process::Command::new("xclip");
        xclip.args(["-selection", "clipboard", "-o"]);
        let output = command_output_with_timeout(&mut xclip, CLIPBOARD_COMMAND_TIMEOUT)
            .ok()
            .flatten()
            .filter(|output| output.status.success())
            .or_else(|| {
                let mut xsel = std::process::Command::new("xsel");
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
        let mut command = std::process::Command::new("powershell");
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
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut result: String = s.chars().take(max_len).collect();
        result.push_str("...");
        result
    }
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
        assert!(preview.contains("[API_KEY]"), "expected [API_KEY] marker");
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
