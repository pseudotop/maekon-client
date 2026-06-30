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
//! `crates/maekon-web/src/lib.rs:524` (case-insensitive per RFC 7235) — REST
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

/// #6420: per-session local-auth gate for the loopback gRPC dashboard, mirroring the
/// REST `require_local_auth` middleware (`crates/maekon-web/src/lib.rs`). On a multi-user
/// host a loopback bind alone does not isolate other local users, so the gRPC surface must
/// require the same per-session token the REST `/api` surface does.
///
/// Returns `Err` when no token is configured (mirrors REST's fail-closed
/// `require_local_auth`). Otherwise the request must present the token via the
/// `x-local-auth` metadata key or `authorization: Bearer <token>`; compared in constant
/// time (subtle) to avoid a timing side channel.
pub fn check_local_auth(
    metadata: &tonic::metadata::MetadataMap,
    configured_token: Option<&str>,
) -> Result<(), Status> {
    let Some(expected) = configured_token.map(str::trim).filter(|t| !t.is_empty()) else {
        return Err(Status::unauthenticated(
            "local-auth token is not configured",
        ));
    };
    let presented = metadata
        .get("x-local-auth")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .or_else(|| {
            metadata
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|h| strip_prefix_ignore_ascii_case(h.trim(), "Bearer "))
                .map(str::trim)
                .filter(|t| !t.is_empty())
        });
    match presented {
        Some(p) => {
            use subtle::ConstantTimeEq;
            if bool::from(p.as_bytes().ct_eq(expected.as_bytes())) {
                Ok(())
            } else {
                Err(Status::unauthenticated("invalid local-auth token"))
            }
        }
        None => Err(Status::unauthenticated("missing local-auth token")),
    }
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
            // Security-relevant: validate_authority is the DNS-rebinding guard.
            // A regression that accidentally returns Err for a loopback authority
            // would silently block legitimate callers. Pin the Ok value (unit) to
            // distinguish from a permission-denied / invalid-argument error. (#5594)
            validate_authority(Some(a)).unwrap_or_else(|e| {
                panic!(
                    "authority should be allowlisted but was rejected: {a} — got {:?} (code={:?})",
                    e,
                    e.code()
                )
            });
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
            assert_eq!(
                validate_authority(Some(a)).unwrap_err().code(),
                tonic::Code::PermissionDenied,
                "authority should be rejected with PermissionDenied: {a}"
            );
        }
    }

    #[test]
    fn validate_authority_missing_returns_invalid_argument() {
        let err = validate_authority(None).expect_err("None must reject");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn validate_authority_absent_host_is_rejected_not_bypassed() {
        // Finding #28: SubscribeEvents/SubscribeMetrics must pass the `"host"`
        // metadata straight through (Option<&str>) so an ABSENT authority hits
        // this missing-authority rejection branch on the external/routable
        // instance, instead of silently skipping the allowlist guard. An empty
        // string is an explicit-but-empty authority, which is treated as a
        // non-allowlisted host (PermissionDenied), distinct from absence (None).
        assert_eq!(
            validate_authority(None).unwrap_err().code(),
            tonic::Code::InvalidArgument,
            "absent authority must be rejected, never bypassed"
        );
        assert_eq!(
            validate_authority(Some("")).unwrap_err().code(),
            tonic::Code::PermissionDenied,
            "empty authority is not allowlisted"
        );
    }

    // ── check_local_auth tests (#6420) ──────────────────────────
    fn md_with(key: &str, val: &str) -> tonic::metadata::MetadataMap {
        let mut m = tonic::metadata::MetadataMap::new();
        let k: tonic::metadata::MetadataKey<tonic::metadata::Ascii> =
            key.parse().expect("valid metadata key");
        m.insert(k, val.parse().expect("valid metadata value"));
        m
    }

    #[test]
    fn local_auth_no_token_configured_fails_closed() {
        let err = check_local_auth(&tonic::metadata::MetadataMap::new(), None)
            .expect_err("no configured token must fail closed");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
        let err = check_local_auth(&tonic::metadata::MetadataMap::new(), Some("   "))
            .expect_err("whitespace-only configured token must fail closed");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }

    #[test]
    fn local_auth_accepts_matching_token_via_either_channel() {
        check_local_auth(&md_with("x-local-auth", "tok123"), Some("tok123"))
            .expect("matching x-local-auth must pass");
        check_local_auth(&md_with("authorization", "Bearer tok123"), Some("tok123"))
            .expect("matching authorization bearer must pass");
        check_local_auth(&md_with("authorization", "bearer tok123"), Some("tok123"))
            .expect("bearer scheme is case-insensitive (RFC 7235)");
    }

    #[test]
    fn local_auth_rejects_missing_token() {
        let err = check_local_auth(&tonic::metadata::MetadataMap::new(), Some("tok123"))
            .expect_err("a configured token with no presented token must reject");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }

    #[test]
    fn local_auth_rejects_wrong_token() {
        let err = check_local_auth(&md_with("x-local-auth", "wrong"), Some("tok123"))
            .expect_err("wrong token must reject");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }
}
