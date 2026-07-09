//! Pure, platform-agnostic parsers for the Windows active-window FFI shim
//! (`windows.rs`).
//!
//! The Win32 calls themselves (`GetWindowTextW`, `GetWindowRect`,
//! `GetLastInputInfo`/`GetTickCount`) can only run on a Windows host, but the
//! *decisions* they feed — how a UTF-16 title buffer is decoded, when a `RECT`
//! is a usable window rectangle, and how idle ticks are converted to seconds —
//! are ordinary logic. Extracting them here (taking primitive inputs, with no
//! `windows-sys` types and no `#[cfg]` gate) makes that logic unit-testable on
//! **any** OS, so a regression is caught by the ordinary ubuntu/macOS test run
//! rather than requiring a (billing-blocked) Windows CI runner.
//!
//! Mirrors the `key_hook::classify` seam. Public CI posture:
//! `docs/guides/ci-transparency.md#native-parser-seams` (#5120).

// Each helper is the parsing core of a platform FFI shim (`windows.rs`,
// `macos.rs`) plus the cross-platform unit tests below. On any single target
// the *other* platform's helpers have no non-test caller, so they read as
// "dead" in a non-test build there — allow it module-wide rather than gating
// each helper to the one OS that calls it (which would defeat the cross-platform
// testability that is the whole point of this seam).
#![allow(dead_code)]

use maekon_core::models::context::WindowBounds;

/// Decode a UTF-16 window-title buffer as returned by `GetWindowTextW`.
///
/// `len` is the count of UTF-16 code units the API reports it wrote (its return
/// value). `len <= 0` means no title (or a failed query) → empty string. `len`
/// is clamped to the buffer length so a bogus over-long count can never index
/// out of bounds.
pub(crate) fn decode_window_title(buf: &[u16], len: i32) -> String {
    if len <= 0 {
        return String::new();
    }
    let end = (len as usize).min(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Convert Win32 `RECT` edges into `WindowBounds`, returning `None` for a
/// degenerate (zero- or negative-area) rectangle — the `width > 0 && height > 0`
/// guard the inline code used.
///
/// The width/height comparison is done on **signed** `i32` values *before* the
/// `u32` cast. The previous inline code cast `(right - left) as u32` first and
/// then compared `> 0`, so an inverted rectangle (`right < left`) wrapped to a
/// huge positive `u32` and was wrongly accepted; doing the guard on the signed
/// difference fixes that.
pub(crate) fn window_bounds_from_edges(
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
) -> Option<WindowBounds> {
    let width = right - left;
    let height = bottom - top;
    if width > 0 && height > 0 {
        Some(WindowBounds {
            x: left,
            y: top,
            width: width as u32,
            height: height as u32,
        })
    } else {
        None
    }
}

/// Idle seconds from two `GetTickCount` samples (`now`, `last_input`).
///
/// `GetTickCount` is a `u32` millisecond counter that wraps to 0 after ~49.7
/// days; `wrapping_sub` yields the correct elapsed interval across that
/// boundary instead of a saturating-to-zero or panicking subtraction.
pub(crate) fn idle_secs_from_ticks(now: u32, last_input: u32) -> u64 {
    (now.wrapping_sub(last_input) / 1000) as u64
}

// -- macOS osascript active-window fallback (macos.rs) --

pub(crate) const OSASCRIPT_FIELD_SEPARATOR: &str = "\u{1F}";

/// A window parsed from the macOS `osascript` fallback's unit-separator-delimited
/// stdout, *before* self-window filtering (own-app-name / own-pid), which the
/// caller applies. Field extraction only — no environment or process access —
/// so it is unit-testable on any OS with fabricated osascript output.
#[derive(Debug, Clone)]
pub(crate) struct ParsedOsascriptWindow {
    pub app_name: String,
    pub title: String,
    pub bundle_id: Option<String>,
    pub pid: u32,
    pub bounds: Option<WindowBounds>,
}

fn split_osascript_fields(result: &str) -> Vec<&str> {
    if result.contains(OSASCRIPT_FIELD_SEPARATOR) {
        result.split(OSASCRIPT_FIELD_SEPARATOR).collect()
    } else {
        result.split('|').collect()
    }
}

/// Parse one osascript active-window line:
/// `app <US> title <US> x <US> y <US> width <US> height <US> pid <US> bundle_id`.
///
/// Older pipe-delimited output is still accepted as a compatibility fallback.
///
/// Returns `None` for empty/whitespace-only output. (The inline path guarded
/// with `parts.is_empty()`, which is dead — `str::split` never yields an empty
/// iterator — so its intended "no osascript output ⇒ no active window" was lost;
/// this restores it as an explicit empty-input check.) Missing fields default
/// (title/bundle empty, numerics to 0); a zero-or-smaller area drops `bounds`.
pub(crate) fn parse_osascript_active_window(stdout: &str) -> Option<ParsedOsascriptWindow> {
    let result = stdout.trim();
    if result.is_empty() {
        return None;
    }

    let parts = split_osascript_fields(result);
    // `result` is non-empty, so `split` yields at least one element.
    let app_name = parts[0].to_string();
    let title = parts.get(1).map(|s| s.to_string()).unwrap_or_default();
    let pid = parts
        .get(6)
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    let bundle_id = parts
        .get(7)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);

    let bounds = if parts.len() >= 6 {
        let x = parts[2].parse::<i32>().unwrap_or(0);
        let y = parts[3].parse::<i32>().unwrap_or(0);
        let width = parts[4].parse::<u32>().unwrap_or(0);
        let height = parts[5].parse::<u32>().unwrap_or(0);
        // osascript reports width/height directly (already unsigned), so the
        // `> 0` guard here is on real magnitudes — no inverted-edge hazard.
        if width > 0 && height > 0 {
            Some(WindowBounds {
                x,
                y,
                width,
                height,
            })
        } else {
            None
        }
    } else {
        None
    };

    Some(ParsedOsascriptWindow {
        app_name,
        title,
        bundle_id,
        pid,
        bounds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- decode_window_title --

    #[test]
    fn decode_title_reads_exactly_len_code_units() {
        // "Hi" plus trailing garbage past `len` that must be ignored.
        let buf: [u16; 5] = ['H' as u16, 'i' as u16, 'X' as u16, 0, 0];
        assert_eq!(decode_window_title(&buf, 2), "Hi");
    }

    #[test]
    fn decode_title_empty_when_len_zero_or_negative() {
        let buf: [u16; 3] = ['A' as u16, 'B' as u16, 'C' as u16];
        assert_eq!(decode_window_title(&buf, 0), "");
        assert_eq!(decode_window_title(&buf, -1), "");
    }

    #[test]
    fn decode_title_clamps_len_to_buffer_length() {
        // A bogus over-long `len` must not index out of bounds.
        let buf: [u16; 2] = ['O' as u16, 'K' as u16];
        assert_eq!(decode_window_title(&buf, 999), "OK");
    }

    #[test]
    fn decode_title_handles_unicode_and_lone_surrogate() {
        // U+D55C (a Hangul syllable) then a lone high surrogate → from_utf16_lossy
        // yields U+FFFD.
        let buf: [u16; 2] = [0xD55C, 0xD800];
        let out = decode_window_title(&buf, 2);
        assert_eq!(out.chars().next(), Some('\u{D55C}'));
        assert!(
            out.contains('\u{FFFD}'),
            "lone surrogate must become U+FFFD"
        );
    }

    // -- window_bounds_from_edges --

    #[test]
    fn bounds_from_normal_rect() {
        let b = window_bounds_from_edges(10, 20, 110, 70).expect("positive-area rect");
        assert_eq!((b.x, b.y, b.width, b.height), (10, 20, 100, 50));
    }

    #[test]
    fn bounds_none_for_zero_area() {
        assert!(window_bounds_from_edges(0, 0, 0, 0).is_none());
        assert!(window_bounds_from_edges(5, 5, 5, 50).is_none()); // zero width
        assert!(window_bounds_from_edges(5, 5, 50, 5).is_none()); // zero height
    }

    #[test]
    fn bounds_none_for_inverted_rect_regression() {
        // right < left / bottom < top. The old `(right-left) as u32 > 0` accepted
        // this (wrapped to a huge u32); the signed guard correctly rejects it.
        assert!(window_bounds_from_edges(100, 100, 10, 200).is_none());
        assert!(window_bounds_from_edges(10, 200, 110, 100).is_none());
    }

    #[test]
    fn bounds_preserve_negative_origin() {
        // A window on a secondary monitor left/above the primary has negative
        // origin but positive area — must be kept.
        let b = window_bounds_from_edges(-1920, -100, -920, 500).expect("valid");
        assert_eq!((b.x, b.y, b.width, b.height), (-1920, -100, 1000, 600));
    }

    // -- idle_secs_from_ticks --

    #[test]
    fn idle_secs_basic() {
        assert_eq!(idle_secs_from_ticks(10_000, 2_500), 7); // 7.5s floors to 7
        assert_eq!(idle_secs_from_ticks(5_000, 5_000), 0);
    }

    #[test]
    fn idle_secs_handles_tick_wraparound() {
        // last_input just before u32 wrap, now just after → 2000ms elapsed = 2s.
        let last = u32::MAX - 999; // 1000ms before wrap
        let now = 1_000; // 1000ms after wrap
        assert_eq!(idle_secs_from_ticks(now, last), 2);
    }

    // -- parse_osascript_active_window --

    #[test]
    fn osascript_parses_full_line() {
        let w = parse_osascript_active_window("Safari|My Page|10|20|800|600|1234|com.apple.Safari")
            .expect("a full line parses");
        assert_eq!(w.app_name, "Safari");
        assert_eq!(w.title, "My Page");
        assert_eq!(w.pid, 1234);
        assert_eq!(w.bundle_id.as_deref(), Some("com.apple.Safari"));
        let b = w.bounds.expect("full line has bounds");
        assert_eq!((b.x, b.y, b.width, b.height), (10, 20, 800, 600));
    }

    #[test]
    fn osascript_preserves_pipe_in_title_with_unit_separator_fields() {
        let sep = '\u{1F}';
        let line = format!(
            "Safari{sep}Inbox (3) | Gmail{sep}10{sep}20{sep}800{sep}600{sep}1234{sep}com.apple.Safari"
        );
        let w = parse_osascript_active_window(&line).expect("unit-separator line parses");
        assert_eq!(w.app_name, "Safari");
        assert_eq!(w.title, "Inbox (3) | Gmail");
        assert_eq!(w.pid, 1234);
        assert_eq!(w.bundle_id.as_deref(), Some("com.apple.Safari"));
        let b = w.bounds.expect("full line has bounds");
        assert_eq!((b.x, b.y, b.width, b.height), (10, 20, 800, 600));
    }

    #[test]
    fn osascript_empty_or_blank_is_none() {
        // The restored guard: the inline `parts.is_empty()` was dead, so empty
        // osascript output used to yield a bogus all-empty window.
        assert!(parse_osascript_active_window("").is_none());
        assert!(parse_osascript_active_window("   \n").is_none());
    }

    #[test]
    fn osascript_app_only_has_empty_title_and_no_bounds() {
        let w = parse_osascript_active_window("Finder").expect("app-only line parses");
        assert_eq!(w.app_name, "Finder");
        assert_eq!(w.title, "");
        assert_eq!(w.pid, 0);
        assert!(w.bundle_id.is_none());
        assert!(w.bounds.is_none()); // < 6 parts → no bounds
    }

    #[test]
    fn osascript_zero_area_drops_bounds() {
        let w = parse_osascript_active_window("App|T|0|0|0|600|99|b").expect("parses");
        assert!(w.bounds.is_none(), "zero width must drop bounds");
        assert_eq!(w.pid, 99);
    }

    #[test]
    fn osascript_empty_or_missing_bundle_is_none() {
        // Trailing empty bundle field → None (whitespace-trimmed, filtered).
        let w = parse_osascript_active_window("App|T|0|0|10|10|5|").expect("parses");
        assert!(w.bundle_id.is_none());
        // Only 7 fields (no bundle index) → None.
        let w2 = parse_osascript_active_window("App|T|0|0|10|10|5").expect("parses");
        assert!(w2.bundle_id.is_none());
    }

    #[test]
    fn osascript_non_numeric_fields_degrade_to_defaults() {
        // Garbage pid/coords degrade to 0 (matching the inline `unwrap_or(0)`).
        let w = parse_osascript_active_window("App|T|x|y|w|h|notapid|b").expect("parses");
        assert_eq!(w.pid, 0);
        assert!(w.bounds.is_none()); // width/height parse to 0 → dropped
    }
}
