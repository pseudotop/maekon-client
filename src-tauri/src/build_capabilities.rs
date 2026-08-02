//! Build-capability markers embedded in the compiled binary (#9659).
//!
//! ## Why this exists
//!
//! `server` is deliberately NOT a default feature: the OSS build is a
//! self-contained local-first product and every feature works without an
//! account. Sign-in is strictly opt-in — see `commands::auth`, where the real
//! network login lives behind `cfg(feature = "server")` and the disabled build
//! answers `auth.feature_disabled`.
//!
//! That opt-in design is correct, but it left a hole: a login-capable binary
//! and a login-less one were **indistinguishable from the outside**.
//! `scripts/build-macos-dev-bundle.sh` never forwarded `--features`, so the
//! only bundle route the repository documented produced a `.app` that silently
//! could not sign in — and nothing about the successful build said so
//! (#9659). Someone preparing a demo machine found out on stage.
//!
//! The fix is to make the *artifact* answer the question. Exactly one of the
//! two literals below is compiled into the binary, selected by the same
//! `cfg(feature = "server")` that gates the login implementation, so reading
//! the binary reports what was actually built rather than what the build was
//! asked to build:
//!
//! ```text
//! ./scripts/verify-bundle-capabilities.sh "target/debug/bundle/macos/Maekon Dev.app"
//! ```
//!
//! ## Why a dedicated marker instead of probing for an incidental symbol
//!
//! Grepping for whatever string happens to differ between the two builds (an
//! error message, a mangled symbol name) works right up until someone rewords
//! the message or the optimizer inlines the symbol — and then the check
//! silently starts passing (or failing) for the wrong reason. That is the same
//! documentation-drifts-from-reality class this module exists to close, so the
//! marker is explicit, named, and covered by
//! `crates/maekon-lint/tests/bundle_capability_marker_gate.rs`, which pins the
//! literals here to the copies in the shell verifier.
//!
//! ## Runtime counterpart
//!
//! At runtime the app already reports the same fact through the `auth_status`
//! IPC command's `server_feature` field: the Sign in screen and
//! **Settings → General → Account** render a "not in this build" notice
//! instead of a form when it is `false`. This module is the *offline* half of
//! that — answerable from a file on disk, without launching the app.

/// Marker literal for a build that includes the `server` feature (sign-in,
/// server transport). Compiled in **only** when the login implementation is.
#[cfg(feature = "server")]
pub const SERVER_CAPABILITY_MARKER: &str = "maekon-build-capability:server=on";

/// Marker literal for a build without the `server` feature. This is the
/// DEFAULT build and a fully supported product configuration — the client is
/// complete without an account. It simply cannot sign in.
#[cfg(not(feature = "server"))]
pub const SERVER_CAPABILITY_MARKER: &str = "maekon-build-capability:server=off";

/// Keeps [`SERVER_CAPABILITY_MARKER`] in the linked image.
///
/// A `const` with no live use site is inlined away and never reaches the
/// binary, which would make the artifact check silently unable to find its
/// marker. `#[used]` lowers to `llvm.used`, which the Mach-O backend emits as
/// `.no_dead_strip` — so the static, and the string data it points at, survive
/// both LLVM's optimizer and the linker's dead-strip pass regardless of
/// whether any real code path reads the marker.
#[used]
static SERVER_CAPABILITY_ANCHOR: &str = SERVER_CAPABILITY_MARKER;

#[cfg(test)]
mod tests {
    use super::SERVER_CAPABILITY_MARKER;

    /// The marker claims to describe whether the login path is compiled in.
    /// Bind that claim to the login path itself: `auth_status_inner` is the
    /// server-backed status/login helper and exists *only* under
    /// `cfg(feature = "server")`, so naming it here does not compile unless
    /// the real implementation is present.
    ///
    /// KNOWN COST (review #9676 M2): this half of the binding only exists in a
    /// `--features server` build, and no PR-triggered job in the parent
    /// monorepo compiles that feature — the only parent lane that does is the
    /// weekly `maekon-client-patch-audit.yml` clippy matrix. So on a normal PR
    /// only the `not(server)` twin below runs, and a break here surfaces at the
    /// weekly cadence (or on the public export's feature matrix) rather than on
    /// the PR that caused it.
    #[cfg(feature = "server")]
    #[test]
    fn marker_reports_on_when_the_server_login_path_is_compiled() {
        let _server_login_path_exists = crate::commands::auth::auth_status_inner;
        assert_eq!(
            SERVER_CAPABILITY_MARKER,
            "maekon-build-capability:server=on"
        );
    }

    /// Mirror of the above for the default build. Asserting the exact literal
    /// (not just "differs from the other one") is what lets the shell verifier
    /// and this module be compared byte-for-byte by the maekon-lint gate.
    #[cfg(not(feature = "server"))]
    #[test]
    fn marker_reports_off_when_the_server_login_path_is_absent() {
        assert_eq!(
            SERVER_CAPABILITY_MARKER,
            "maekon-build-capability:server=off"
        );
    }

    /// The anchor must carry the *same* value the verifier greps for. A
    /// refactor that points the anchor at some other string would leave the
    /// marker out of the binary while this module still looked correct.
    #[test]
    fn anchor_carries_the_marker_value() {
        assert_eq!(super::SERVER_CAPABILITY_ANCHOR, SERVER_CAPABILITY_MARKER);
    }
}
