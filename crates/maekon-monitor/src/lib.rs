// Cast safety: system metrics, CPU percentages, process counters — precision loss acceptable.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
// P2 PR-C: `missing_const_for_fn` accepted crate-wide.
// Rationale: const-viral cascade + nursery false-positive rate outweigh the value.
#![allow(clippy::missing_const_for_fn)]
// P2 remaining-nursery-lints: stylistic/cosmetic nursery lints accepted crate-wide.
#![allow(
    clippy::use_self,
    clippy::option_if_let_else,
    clippy::redundant_pub_crate
)]
// P2 PR-A nursery-hardening. (Enforced workspace-wide via
// `[workspace.lints.clippy]`, #7719.)
#![cfg_attr(test, allow(clippy::significant_drop_tightening))]

//! # maekon-monitor

pub mod error;
pub use error::MonitorError;

pub mod activity;
pub mod clipboard;
pub mod file_access;
// Foreground external-window fullscreen detection (#8849). The pure decision
// function compiles + unit-tests on every host; only platform-specific
// coordinate collection is delegated to each OS module.
pub mod foreground_fullscreen;
// Shared running-flag + thread-handle + platform-wake skeleton for
// `key_hook`/`mouse_hook` (#7727) -- private, reached crate-wide via
// `crate::hook_lifecycle`, mirroring the `trusted_binary` mod pattern below.
mod hook_lifecycle;
pub mod idle;
pub mod input_activity;
pub mod input_detail;
pub mod key_hook;
pub mod keyboard_pattern;
pub mod mouse_hook;
pub mod process;
pub mod system;
pub mod system_info;
pub mod window_layout;

// Pure, cfg-free parsers for the Windows active-window FFI shim (#5120). Kept
// un-gated so the logic is unit-testable on any OS, not only a Windows runner.
mod active_window_parse;
// Cfg-free macOS `pmset -g batt` → PowerStatus parser (#5138), same rationale.
mod power_parse;
// Compatibility re-export for the subprocess-spawn circuit breaker (#6828).
// The state machine itself now lives in `maekon_core::circuit_breaker`
// (#7720 E6 consolidation, shared with `maekon-vision`'s accessibility
// guards); this module keeps the `crate::circuit_breaker::CircuitBreaker`
// path stable for every existing call site in this crate.
mod circuit_breaker;
// Native EWMH active-window via pure-Rust x11rb (#6828). Un-gated so it compiles +
// the title-decode helper is unit-tested on every host; only `linux` CALLS the
// connection path (it returns None without a reachable X server elsewhere).
mod x11_active_window;
// SEC-MON-01: trusted-path resolver for bare-name external-command spawns
// (#7574, generalizing the macOS-only fix in #7483). Un-gated so the resolver
// and its per-OS directory table are unit-tested on every host; `macos`,
// `linux`, `clipboard`, and `key_hook::linux` all route their subprocess
// spawns through it. `pub` + re-exported below so `src-tauri` — which spawns
// the SAME class of bare-name system tools (osascript, powershell, xdotool,
// etc.) from the composition-root binary — shares this ONE allowlist instead
// of duplicating it (intra-binary hardening divergence closed).
mod trusted_binary;
pub use trusted_binary::resolve_trusted_binary;
// Content-free log digest for window titles + suggestion content (#5591, #6006)
// + workspace-wide textual guard against raw-title/content tracing.
// Un-gated so the guard runs on every host OS.
pub mod log_privacy;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "macos")]
mod macos_ax_ffi;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "linux")]
pub mod linux;

/// Whether active-window detection is expected to work reliably on this
/// platform + runtime session.
///
/// macOS (`osascript`) and Windows (`GetForegroundWindow`) each have a single,
/// dependable native active-window API and always report `true`. Linux is
/// runtime-dependent: an X11 session queries `_NET_ACTIVE_WINDOW` natively and
/// reliably, but a Wayland session has no single dependable path — modern
/// GNOME disables `org.gnome.Shell.Eval` by default since GNOME 41, and any
/// compositor without a dedicated adapter falls back to XWayland (X11/XWayland
/// apps only; native Wayland apps stay invisible). See
/// `linux::get_active_window_linux()` for the full GNOME → Sway → XWayland
/// fallback chain and its own one-time `warn!` on the GNOME-disabled path.
///
/// This is a conservative, cheap (env-var read only, no subprocess spawn)
/// signal — it reads the same `$XDG_SESSION_TYPE`/`$WAYLAND_DISPLAY`/`$DISPLAY`
/// state as [`linux::detect_display_server`], not a live probe of which
/// compositor adapter actually succeeds. A Wayland session running Sway (which
/// DOES have a working native path via `swaymsg`) is conservatively reported
/// as `false` here — the safe direction (never over-promise a working
/// capability).
///
/// `FeatureCapabilitySnapshot.active_window_available` (src-tauri) is the
/// single consumer of this predicate (#7678).
#[cfg(target_os = "linux")]
#[must_use]
pub fn active_window_reliable() -> bool {
    !matches!(
        crate::linux::detect_display_server(),
        crate::linux::DisplayServer::Wayland
    )
}

#[cfg(not(target_os = "linux"))]
#[must_use]
pub const fn active_window_reliable() -> bool {
    true
}

/// Owner-app names of all normal, on-screen windows on the current display(s).
///
/// Used by the capture-time partial-occlusion guard (#8054 P2-4) to detect
/// background windows of excluded/sensitive apps that share the screen with the
/// active (non-excluded) window. macOS enumerates via CGWindowList (owner names
/// only — titles need screen-recording permission). Windows and Linux have no
/// cheap, permission-free all-window enumeration wired yet, so they return an
/// empty list (the active-app exclusion check still applies on every platform);
/// broader coverage is tracked as follow-up.
#[must_use]
pub fn visible_window_app_names() -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        crate::macos::visible_window_app_names()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod active_window_capability_tests {
    use super::active_window_reliable;

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn non_linux_platforms_are_always_reliable() {
        assert!(active_window_reliable());
    }

    /// On Linux, the flag must track `detect_display_server()` directly (not
    /// merely the same env-var reads duplicated) so a future edit to one
    /// without the other is caught here instead of silently drifting
    /// (mirrors maekon-vision's `native_ocr` cross-check, #7602/#7678).
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_flag_matches_detect_display_server() {
        let expected = !matches!(
            crate::linux::detect_display_server(),
            crate::linux::DisplayServer::Wayland
        );
        assert_eq!(active_window_reliable(), expected);
    }
}
