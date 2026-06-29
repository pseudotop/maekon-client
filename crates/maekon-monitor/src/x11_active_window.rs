//! Native EWMH active-window detection via pure-Rust x11rb (#6828).
//!
//! Replaces the per-1s-tick `xdotool` subprocess fork-storm (getactivewindow +
//! getwindowname + getwindowpid + getwindowgeometry — four forks per tick) with a
//! single X11 connection that reads the EWMH properties directly:
//! `_NET_ACTIVE_WINDOW` (root) → `_NET_WM_NAME` (UTF8, `WM_NAME` fallback) +
//! `_NET_WM_PID` + window geometry.
//!
//! x11rb's `RustConnection` speaks the X11 protocol over a socket (no libxcb), so
//! this module COMPILES on every host — only the runtime connection requires a
//! reachable X server (`$DISPLAY`), so on non-Linux / headless / Wayland-without-
//! XWayland it simply returns `None` (degrading exactly like the old xdotool path).
//! It is only CALLED from the Linux active-window path; the connection helpers are
//! therefore `dead_code` on non-Linux hosts (suppressed below).

use crate::circuit_breaker::CircuitBreaker;
use maekon_core::models::context::WindowBounds;
use std::time::Duration;

/// Native active-window result (pre-app-name; the caller resolves the process name
/// from `pid`, reusing the existing `/proc`-based `get_process_name`).
// Fields are consumed by the Linux active-window path; on other hosts the struct is
// only constructed by the (dead-on-non-Linux) connection fn, so the fields read as
// never-used there.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Clone)]
pub(crate) struct NativeActiveWindow {
    pub title: String,
    pub pid: Option<u32>,
    pub bounds: Option<WindowBounds>,
}

/// Decode an `_NET_WM_NAME` (UTF8_STRING) or `WM_NAME` (Latin-1/STRING) property
/// value into a trimmed title.
///
/// `from_utf8_lossy` handles both: a modern `_NET_WM_NAME` is already UTF-8, and a
/// legacy `WM_NAME` is Latin-1 whose ASCII subset (< 0x80) is valid UTF-8 (high
/// bytes degrade to U+FFFD — acceptable, and the UTF-8 property is tried first).
/// An empty / whitespace-only value yields an empty string (matching the old
/// xdotool `getwindowname` empty-output behavior).
pub(crate) fn decode_x11_title(value: &[u8]) -> String {
    String::from_utf8_lossy(value).trim().to_string()
}

/// Query the X server for the active window's title, pid, and bounds.
///
/// Returns `None` when no X server is reachable (`$DISPLAY` unset / connect
/// refused — non-Linux, headless, or non-XWayland Wayland), when there is no active
/// window, or on any X protocol error — the same degradation as the previous
/// xdotool path. Synchronous + blocking (x11rb round-trips are blocking syscalls):
/// the caller MUST run it under `tokio::task::spawn_blocking`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn query_active_window() -> Option<NativeActiveWindow> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

    let (conn, screen_num) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots.get(screen_num)?.root;

    let net_active_window = intern_atom(&conn, b"_NET_ACTIVE_WINDOW")?;
    let net_wm_name = intern_atom(&conn, b"_NET_WM_NAME")?;
    let utf8_string = intern_atom(&conn, b"UTF8_STRING")?;
    let net_wm_pid = intern_atom(&conn, b"_NET_WM_PID")?;

    // Active window id from the root window's _NET_ACTIVE_WINDOW property.
    let active = conn
        .get_property(false, root, net_active_window, AtomEnum::WINDOW, 0, 1)
        .ok()?
        .reply()
        .ok()?;
    let window = active.value32().and_then(|mut ids| ids.next())?;
    if window == 0 {
        return None;
    }

    // Title: _NET_WM_NAME (UTF8_STRING) preferred, legacy WM_NAME (STRING) fallback.
    let title = property_value(&conn, window, net_wm_name, utf8_string)
        .filter(|bytes| !bytes.is_empty())
        .or_else(|| {
            property_value(
                &conn,
                window,
                AtomEnum::WM_NAME.into(),
                AtomEnum::STRING.into(),
            )
        })
        .map(|bytes| decode_x11_title(&bytes))
        .unwrap_or_default();

    // PID from _NET_WM_PID (CARDINAL); 0 / absent → None (→ "Unknown" app name upstream).
    let pid = conn
        .get_property(false, window, net_wm_pid, AtomEnum::CARDINAL, 0, 1)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .and_then(|reply| reply.value32().and_then(|mut vals| vals.next()))
        .filter(|&p| p != 0);

    let bounds = window_bounds(&conn, window, root);

    Some(NativeActiveWindow { title, pid, bounds })
}

/// Intern an atom, returning its id (or `None` on connection / protocol error).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn intern_atom(conn: &x11rb::rust_connection::RustConnection, name: &[u8]) -> Option<u32> {
    use x11rb::protocol::xproto::ConnectionExt;
    conn.intern_atom(false, name)
        .ok()?
        .reply()
        .ok()
        .map(|reply| reply.atom)
}

/// Fetch a window property's raw bytes (up to 1 MiB), or `None` on error / absence.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn property_value(
    conn: &x11rb::rust_connection::RustConnection,
    window: u32,
    property: u32,
    type_: u32,
) -> Option<Vec<u8>> {
    use x11rb::protocol::xproto::ConnectionExt;
    // long_length is in 4-byte units; 262_144 * 4 = 1 MiB cap on a window title.
    let reply = conn
        .get_property(false, window, property, type_, 0, 262_144)
        .ok()?
        .reply()
        .ok()?;
    Some(reply.value)
}

/// Absolute window bounds: geometry (size, parent-relative origin) + a
/// translate_coordinates round-trip to convert the origin to root-absolute x/y.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn window_bounds(
    conn: &x11rb::rust_connection::RustConnection,
    window: u32,
    root: u32,
) -> Option<WindowBounds> {
    use x11rb::protocol::xproto::ConnectionExt;
    let geometry = conn.get_geometry(window).ok()?.reply().ok()?;
    let translated = conn
        .translate_coordinates(window, root, 0, 0)
        .ok()?
        .reply()
        .ok()?;
    Some(WindowBounds {
        x: i32::from(translated.dst_x),
        y: i32::from(translated.dst_y),
        width: u32::from(geometry.width),
        height: u32::from(geometry.height),
    })
}

/// Per-query timeout for the native X round-trips. Mirrors `linux.rs`
/// `SUBPROCESS_TIMEOUT_SECS` so the native path degrades on the same 5s budget as the
/// sibling subprocess paths.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const X11_QUERY_TIMEOUT_SECS: u64 = 5;

/// #6882: x11rb's `RustConnection` opens its socket with no read/connect timeout, so a
/// wedged or remote-forwarded X server (e.g. X11 over a stalled SSH/TCP link, or a hung
/// Xorg) makes every `.reply()` block forever. The deleted xdotool active-window path
/// had bounded each fork with a `timeout` + the `XDOTOOL_BREAKER`; the #6828 native
/// rewrite dropped both. This breaker restores the bound: after `threshold` consecutive
/// query timeouts it opens, so a persistently-stalled `$DISPLAY` stops being re-attempted
/// every monitor tick (matching `linux.rs`'s `CircuitBreaker::new(3, 60)` siblings).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
static X11_ACTIVE_WINDOW_BREAKER: CircuitBreaker = CircuitBreaker::new(3, 60);

/// Run [`query_active_window`] on a blocking thread, bounded by the breaker and the
/// `X11_QUERY_TIMEOUT_SECS` budget, so a wedged/remote X server cannot stall the awaited
/// monitor tick. Returns `None` on timeout, panic/cancel, or a clean no-active-window
/// result — the same degradation contract as the old xdotool path, now also covering the
/// hang case it never bounded.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) async fn query_active_window_bounded() -> Option<NativeActiveWindow> {
    run_bounded(
        query_active_window,
        Duration::from_secs(X11_QUERY_TIMEOUT_SECS),
    )
    .await
}

/// Breaker + timeout wrapper around a blocking active-window query. Extracted and made
/// generic over the query closure so the timeout/breaker semantics are unit-testable on
/// every host without a real X server. Only a TIMEOUT advances the breaker — a clean
/// `None` (no reachable X server / no active window) leaves it closed, since that is a
/// fast connect-refused, not a stall.
///
/// NOTE: `tokio::time::timeout` cannot interrupt the in-flight `spawn_blocking` thread;
/// on timeout it keeps blocking until the socket finally errors/returns (OS TCP timeout)
/// and then exits. The breaker bounds this to at most one leaked thread per
/// `retry_interval` window while open, and — crucially — the monitor tick no longer
/// stalls waiting for it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
async fn run_bounded<F>(query: F, timeout_dur: Duration) -> Option<NativeActiveWindow>
where
    F: FnOnce() -> Option<NativeActiveWindow> + Send + 'static,
{
    if !X11_ACTIVE_WINDOW_BREAKER.should_proceed() {
        return None;
    }
    match tokio::time::timeout(timeout_dur, tokio::task::spawn_blocking(query)).await {
        Ok(Ok(native)) => {
            // The X server responded (even if with `None`) — connection healthy.
            X11_ACTIVE_WINDOW_BREAKER.record_success();
            native
        }
        Ok(Err(join_err)) => {
            // Panicked / cancelled — not a connection stall, so do not open the breaker.
            tracing::debug!("native x11 active-window task panicked/cancelled: {join_err}");
            None
        }
        Err(_elapsed) => {
            X11_ACTIVE_WINDOW_BREAKER.record_failure();
            tracing::debug!("native x11 active-window query timed out (wedged/remote X server?)");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_title_utf8_and_trim() {
        assert_eq!(decode_x11_title(b"  Firefox  "), "Firefox");
        // UTF-8 multibyte (e.g. a CJK title) is preserved.
        assert_eq!(
            decode_x11_title("프로젝트 \u{1F680}".as_bytes()),
            "프로젝트 \u{1F680}"
        );
    }

    #[test]
    fn decode_title_empty_and_latin1() {
        assert_eq!(decode_x11_title(b""), "");
        assert_eq!(decode_x11_title(b"   \n"), "");
        // ASCII subset of Latin-1 (WM_NAME fallback) decodes cleanly.
        assert_eq!(decode_x11_title(b"xterm"), "xterm");
        // A high Latin-1 byte degrades to the replacement char rather than panicking.
        let decoded = decode_x11_title(&[b'a', 0xE9, b'b']);
        assert!(decoded.starts_with('a') && decoded.ends_with('b'));
    }

    #[test]
    fn query_active_window_never_panics() {
        // The contract is "never panic" on any host. The result is environment-
        // dependent — a live Linux X session may return Some, while a headless host
        // (CI runners, macOS without XQuartz) returns None — so we only assert the
        // deterministic case: with no X display advertised, the connect must fail
        // cleanly into None rather than panic. (A dev running this inside an active
        // X11 desktop where DISPLAY is set would otherwise spuriously fail an
        // unconditional is_none() assertion — #6828 review.)
        let result = query_active_window();
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
            assert!(
                result.is_none(),
                "no X/Wayland display advertised → no active window"
            );
        }
    }

    /// #6882: a wedged/remote X server (here: a query that blocks far longer than the
    /// timeout) must NOT stall the caller — `run_bounded` returns `None` at the timeout,
    /// and after the threshold of consecutive timeouts the breaker opens and
    /// short-circuits subsequent calls immediately. Serialized because the breaker is a
    /// module-global static.
    #[tokio::test]
    #[serial_test::serial]
    async fn run_bounded_bounds_a_stalled_query_then_opens_the_breaker() {
        // Reset the shared breaker to a known-closed state.
        X11_ACTIVE_WINDOW_BREAKER.record_success();

        let slow = || {
            std::thread::sleep(Duration::from_millis(200));
            Some(NativeActiveWindow {
                title: "stalled".to_string(),
                pid: None,
                bounds: None,
            })
        };

        // 1) A stalled query degrades to None AT the timeout, not after the full block.
        let start = std::time::Instant::now();
        let res = run_bounded(slow, Duration::from_millis(30)).await;
        assert!(res.is_none(), "a stalled query must degrade to None");
        assert!(
            start.elapsed() < Duration::from_millis(150),
            "must return at the 30ms timeout, not after the 200ms query"
        );

        // 2) Two more timeouts drive failures to the threshold (3).
        for _ in 0..2 {
            let _ = run_bounded(slow, Duration::from_millis(30)).await;
        }

        // 3) With the breaker open, the next call short-circuits without spawning the
        //    blocking query at all — well under the timeout budget.
        let start = std::time::Instant::now();
        let res = run_bounded(slow, Duration::from_millis(30)).await;
        assert!(res.is_none(), "an open breaker must short-circuit to None");
        assert!(
            start.elapsed() < Duration::from_millis(15),
            "an open breaker must not spawn the query or wait for the timeout"
        );

        // Leave the breaker closed for any other tests sharing the static.
        X11_ACTIVE_WINDOW_BREAKER.record_success();
    }

    /// #6882: the COMMON case — a clean `None` (no reachable X server / no active
    /// window: headless, Wayland-without-XWayland, or a screen with nothing focused)
    /// must NOT advance the breaker. If it did, the breaker would open on every healthy
    /// headless host and needlessly suppress the query. Only a TIMEOUT may open it.
    /// Pins the `Ok(Ok(None)) => record_success` arm against a future match-arm refactor.
    #[tokio::test]
    #[serial_test::serial]
    async fn run_bounded_clean_none_keeps_the_breaker_closed() {
        X11_ACTIVE_WINDOW_BREAKER.record_success(); // reset to closed

        // Far more clean-None results than the breaker threshold (3) — each must keep
        // proceeding rather than counting toward opening.
        for _ in 0..(3 * 5) {
            assert!(run_bounded(|| None, Duration::from_millis(50))
                .await
                .is_none());
        }

        // Breaker must still be closed: a subsequent slow query is actually attempted
        // (and times out at ~30ms) rather than short-circuited (<15ms). If any of the
        // clean Nones above had advanced the breaker, it would be open here and this
        // call would short-circuit, failing the lower-bound assertion.
        let slow = || {
            std::thread::sleep(Duration::from_millis(200));
            Some(NativeActiveWindow {
                title: "x".to_string(),
                pid: None,
                bounds: None,
            })
        };
        let start = std::time::Instant::now();
        assert!(run_bounded(slow, Duration::from_millis(30)).await.is_none());
        assert!(
            start.elapsed() >= Duration::from_millis(25),
            "breaker must still be closed (query attempted + timed out) — clean None never opens it"
        );

        X11_ACTIVE_WINDOW_BREAKER.record_success(); // leave closed
    }
}
