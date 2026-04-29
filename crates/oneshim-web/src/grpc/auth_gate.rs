//! D13-v2b dashboard gRPC — opt-out trust gate + :authority allowlist.
//!
//! Two pure functions:
//!
//! - [`honor_opt_out`] — returns `true` iff the server MUST keep enforcement
//!   on for this request. Clients may opt out by setting
//!   `respect_server_hints = false`; the server only honors the opt-out when
//!   the connection is trusted (loopback binding OR `Authorization: Bearer
//!   <integration_auth_token>` match — the latter is forward-compat and
//!   unreachable under v2b's loopback-only bind; see spec §4.3).
//!
//! - [`validate_authority`] — tower-layer gate rejecting DNS-rebound
//!   `:authority` / `Host` values that would route browser-based attacks to
//!   the loopback-bound gRPC surface. See spec IMP-V2-A.
//!
//! Security posture: token compare is constant-time via `subtle` to prevent
//! timing-side-channel extraction under DNS-rebind + `performance.now()`
//! probes. Token normalization is stricter than the REST handler at
//! `crates/oneshim-web/src/lib.rs:524` (case-insensitive per RFC 7235) — REST
//! parity is tracked as a separate v3 hygiene item.

use std::net::{IpAddr, SocketAddr};

use tonic::Status;

/// Returns `true` when the server MUST keep enforcement on (= must respect
/// hints). Opposite polarity of the function name for historical reasons
/// (naming retained per spec decision D9); the return carries the
/// enforce-or-not bit.
pub fn honor_opt_out(
    req_respect_hints: bool,
    remote_addr: Option<SocketAddr>,
    auth_header: Option<&str>,
    configured_token: Option<&str>,
) -> bool {
    // Client opted in → enforcement always on.
    if req_respect_hints {
        return true;
    }
    // CRIT-7: if `remote_addr` is None (e.g., unusual wiring / layer stripped
    // the connect-info extension), we cannot identify the caller. Safe default
    // is to enforce — a valid token alone without an identifiable source is
    // not sufficient trust.
    let Some(addr) = remote_addr else {
        return true;
    };
    // Trust (a): loopback binding. IPv4 127.0.0.0/8, IPv6 ::1, and IPv4-mapped
    // v6 loopback `::ffff:127.0.0.1` all treated as trusted.
    if is_local_loopback(&addr.ip()) {
        return false;
    }
    // Trust (b): external but token-verified — matching configured token
    // (RFC 7235 Bearer scheme). Forward-compat path; unreachable under v2b's
    // loopback-only bind since this branch requires a non-loopback remote_addr.
    // Normalized: configured token stripped of whitespace; empty → None so a
    // mis-configured empty string can't bypass.
    let normalized_token = configured_token.and_then(|t| {
        let t = t.trim();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    });
    if let (Some(h), Some(t)) = (auth_header, normalized_token) {
        if let Some(presented) = strip_prefix_ignore_ascii_case(h.trim(), "Bearer ") {
            let presented = presented.trim();
            use subtle::ConstantTimeEq;
            if bool::from(presented.as_bytes().ct_eq(t.as_bytes())) {
                return false;
            }
        }
    }
    // Default: enforce.
    true
}

/// `IpAddr::is_loopback` but with explicit IPv4-mapped-v6 handling. Spec IMP-V2-H.
pub(super) fn is_local_loopback(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
    }
}

/// 4-LoC RFC 7235-compliant prefix stripper (stdlib doesn't provide).
fn strip_prefix_ignore_ascii_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

/// Reject `:authority` values that aren't in the localhost allowlist.
/// Used as a tower-layer guard before the gRPC handler runs; protects against
/// DNS-rebinding attacks that route attacker-controlled hostnames to 127.0.0.1.
///
/// IPv6-bracket-aware: `[::1]:10080` -> extracts `"::1"` correctly. See spec
/// IMP-V2-A + the iter-2 finding that naive `split(':')` breaks bracketed
/// IPv6.
pub fn validate_authority(authority: Option<&str>) -> Result<(), Status> {
    const ALLOWED: &[&str] = &["localhost", "127.0.0.1", "::1", "::ffff:127.0.0.1"];
    let authority = authority.ok_or_else(|| Status::invalid_argument("missing :authority"))?;
    let host = if let Some(stripped) = authority.strip_prefix('[') {
        // IPv6 bracketed — host ends at ']'
        stripped.split(']').next().unwrap_or("")
    } else {
        // IPv4 / hostname — host ends at first ':'
        authority.split(':').next().unwrap_or("")
    };
    if ALLOWED.iter().any(|a| a.eq_ignore_ascii_case(host)) {
        Ok(())
    } else {
        Err(Status::permission_denied("authority not allowlisted"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── honor_opt_out tests (8) ─────────────────────────────────

    #[test]
    fn opt_in_always_enforces() {
        assert!(honor_opt_out(true, None, None, None));
        assert!(honor_opt_out(
            true,
            Some("127.0.0.1:0".parse().unwrap()),
            Some("Bearer abc"),
            Some("abc"),
        ));
    }

    #[test]
    fn opt_out_honored_on_loopback() {
        assert!(!honor_opt_out(
            false,
            Some("127.0.0.1:42".parse().unwrap()),
            None,
            None,
        ));
        assert!(!honor_opt_out(
            false,
            Some("[::1]:42".parse().unwrap()),
            None,
            None,
        ));
    }

    #[test]
    fn opt_out_honored_on_ipv6_mapped_loopback() {
        // ::ffff:127.0.0.1 is IPv4-mapped v6 loopback. IMP-V2-H.
        assert!(!honor_opt_out(
            false,
            Some("[::ffff:127.0.0.1]:42".parse().unwrap()),
            None,
            None,
        ));
    }

    #[test]
    fn opt_out_honored_with_valid_bearer() {
        assert!(!honor_opt_out(
            false,
            Some("10.0.0.5:42".parse().unwrap()),
            Some("Bearer secret"),
            Some("secret"),
        ));
    }

    #[test]
    fn opt_out_accepts_case_insensitive_bearer_per_rfc7235() {
        // RFC 7235: scheme is case-insensitive. IMP-V2-13.
        assert!(!honor_opt_out(
            false,
            Some("10.0.0.5:42".parse().unwrap()),
            Some("bearer secret"),
            Some("secret"),
        ));
        assert!(!honor_opt_out(
            false,
            Some("10.0.0.5:42".parse().unwrap()),
            Some("  Bearer  secret  "),
            Some("secret"),
        ));
    }

    #[test]
    fn opt_out_rejected_external_no_token() {
        assert!(honor_opt_out(
            false,
            Some("10.0.0.5:42".parse().unwrap()),
            None,
            Some("configured"),
        ));
    }

    #[test]
    fn opt_out_rejected_external_wrong_token() {
        assert!(honor_opt_out(
            false,
            Some("10.0.0.5:42".parse().unwrap()),
            Some("Bearer wrong"),
            Some("configured"),
        ));
    }

    #[test]
    fn opt_out_rejected_malformed_auth_header() {
        assert!(honor_opt_out(
            false,
            Some("10.0.0.5:42".parse().unwrap()),
            Some("NotBearer configured"),
            Some("configured"),
        ));
    }

    #[test]
    fn opt_out_rejected_when_configured_token_is_whitespace() {
        // Empty or whitespace-only tokens normalize to None → can't bypass.
        assert!(honor_opt_out(
            false,
            Some("10.0.0.5:42".parse().unwrap()),
            Some("Bearer "),
            Some(""),
        ));
        assert!(honor_opt_out(
            false,
            Some("10.0.0.5:42".parse().unwrap()),
            Some("Bearer "),
            Some("   "),
        ));
    }

    #[test]
    fn auth_gate_handles_missing_remote_addr_as_enforce() {
        // CRIT-7 fallback: under unusual wiring `remote_addr()` could return
        // None; the safe default is to enforce (not silently trust).
        assert!(honor_opt_out(false, None, None, None));
        assert!(honor_opt_out(
            false,
            None,
            Some("Bearer secret"),
            Some("secret"),
        ));
    }

    // ── validate_authority tests (3) ────────────────────────────

    #[test]
    fn validate_authority_allowlist_match_cases() {
        for a in &[
            "localhost",
            "localhost:10080",
            "127.0.0.1",
            "127.0.0.1:10080",
            "[::1]:10080",
            "[::ffff:127.0.0.1]:10080",
            "LOCALHOST:10080", // case-insensitive
        ] {
            assert!(
                validate_authority(Some(a)).is_ok(),
                "authority should be allowlisted: {a}"
            );
        }
    }

    #[test]
    fn validate_authority_rejection_cases() {
        for a in &[
            "example.com:10080",
            "0.0.0.0:10080",
            "[::]:10080",
            "",
            "[fe80::1]:10080",
        ] {
            let result = validate_authority(Some(a));
            assert!(result.is_err(), "authority should be rejected: {a}");
            assert_eq!(
                result.unwrap_err().code(),
                tonic::Code::PermissionDenied,
                "expected PermissionDenied for {a}"
            );
        }
    }

    #[test]
    fn validate_authority_missing_returns_invalid_argument() {
        let err = validate_authority(None).expect_err("None must reject");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }
}
