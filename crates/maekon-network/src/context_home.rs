//! Authenticated context-home transport (#9625, WD-02.2a).
//!
//! Implements [`ContextHomeClient`] on the existing [`HttpApiClient`], reusing
//! its hardened reqwest client (redirect-none, TLS policy, timeout) and the
//! single shared `TokenManager` that already holds the server bearer.
//!
//! ## What this module holds shut
//!
//! **The bearer never leaves Rust.** It is read from `TokenManager` inside
//! `authorized_request` and attached there. It is not a parameter, not a return
//! value, and not in any error built here — the server's error body is dropped
//! rather than interpolated, because a 401 body can echo request context and
//! this error text is carried across IPC into the WebView.
//!
//! **No identity parameter exists.** The path is a constant with no query
//! string; the server resolves actor and org from the JWT alone.
//!
//! ## Deviation from the canonical status table (deliberate, #9625)
//!
//! `docs/guides/http-status-error-mapping.md` maps **401 and 403 both** to
//! `auth.failed`, and `HttpApiClient::check_response` implements exactly that.
//! This surface splits them: 401 → `auth.failed`, 403 → `policy.denied`.
//!
//! The reason is that the two demand opposite responses from the user. 401
//! means the session is gone and re-login fixes it. 403 means authenticated but
//! not permitted — re-login fixes nothing, and routing the user to a login
//! screen is a dead end they cannot escape. The guide's own stated purpose for
//! wire codes ("code-based i18n lookup, code-based retry logic") is served by
//! the split, not by the merge. `policy.denied` is an already-registered code,
//! so the frozen 54-code wire contract is untouched.
//!
//! `check_response` is left alone: widening it would change behaviour for every
//! existing caller, none of which asked for the distinction. The deviation is
//! recorded in the guide's dispatcher table.

use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::error_codes::{
    AuthCode, NetworkCode, NotFoundCode, PolicyCode, ServiceCode, ValidationCode,
};
use maekon_core::models::context_home::ContextHomeSnapshot;
use maekon_core::ports::context_home_client::ContextHomeClient;

use crate::http_client::HttpApiClient;
use crate::outbound::{read_text_capped, BodyReadError};
use crate::resilience::extract_retry_after;

/// Server path. A constant with no query string — see the module note on why
/// there is no identity parameter.
pub const CONTEXT_HOME_PATH: &str = "/api/v1/user-context/context-home";

/// Upper bound on the snapshot body.
///
/// The server bounds the payload itself (20 mail + 20 messenger + 10 project
/// items, 12 participants per thread, 120-char previews), so a response near
/// this cap means the contract changed or something upstream is wrong — either
/// way, buffering it unbounded is not the right answer. 2 MiB leaves two orders
/// of magnitude of headroom over a realistic snapshot while staying a hard
/// ceiling.
pub const MAX_CONTEXT_HOME_BYTES: u64 = 2 * 1024 * 1024;

#[async_trait]
impl ContextHomeClient for HttpApiClient {
    async fn fetch_context_home(&self) -> Result<ContextHomeSnapshot, CoreError> {
        let request = self
            .authorized_request(reqwest::Method::GET, CONTEXT_HOME_PATH)
            .await
            .map_err(CoreError::from)?;

        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                CoreError::RequestTimeout {
                    code: NetworkCode::Timeout,
                    // sentinel: the client-wide budget is not visible here.
                    timeout_ms: 0,
                }
            } else {
                CoreError::Network {
                    code: NetworkCode::Generic,
                    message: format!("context home request failed: {e}"),
                }
            }
        })?;

        let status = response.status();
        if !status.is_success() {
            // The server's body is deliberately NOT interpolated: a 401 body can
            // echo request context back, and this string travels across IPC into
            // the WebView.
            return Err(map_failure_status(
                status.as_u16(),
                extract_retry_after(&response),
            ));
        }

        let body = read_text_capped(response, MAX_CONTEXT_HOME_BYTES)
            .await
            .map_err(|e| match e {
                BodyReadError::TooLarge { len, cap } => CoreError::Validation {
                    code: ValidationCode::InvalidField,
                    field: "body".to_string(),
                    message: format!("context home response too large ({len} > {cap} bytes)"),
                },
                BodyReadError::Transport(te) if te.is_timeout() => CoreError::RequestTimeout {
                    code: NetworkCode::Timeout,
                    timeout_ms: 0,
                },
                BodyReadError::Transport(te) => CoreError::Network {
                    code: NetworkCode::Generic,
                    message: format!("context home response read failed: {te}"),
                },
            })?;

        serde_json::from_str::<ContextHomeSnapshot>(&body).map_err(|e| CoreError::Validation {
            code: ValidationCode::InvalidField,
            field: "body".to_string(),
            // serde's position info (line/column) only — never the body.
            message: format!("context home response is not a valid snapshot: {e}"),
        })
    }
}

/// Map a non-2xx status to a typed failure the UI can branch on.
///
/// A free function on purpose: the whole point is the status table, and a test
/// that needed a live socket to exercise it would not get written. Follows
/// `docs/guides/http-status-error-mapping.md` except for the 403 arm — see the
/// module doc for why.
pub(crate) fn map_failure_status(status: u16, retry_after_secs: u64) -> CoreError {
    match status {
        401 => CoreError::Auth {
            code: AuthCode::Failed,
            message: "context home session expired".to_string(),
        },
        // The one place this departs from the canonical table: re-login does
        // not resolve a 403, so it must not read as an expired session.
        403 => CoreError::PolicyDenied {
            code: PolicyCode::Denied,
            message: "context home access denied for this actor".to_string(),
        },
        404 => CoreError::NotFound {
            code: NotFoundCode::ResourceMissing,
            resource_type: "context_home".to_string(),
            // A constant, not the server body, which can echo request context.
            id: "snapshot".to_string(),
        },
        408 | 504 => CoreError::RequestTimeout {
            code: NetworkCode::Timeout,
            timeout_ms: 0,
        },
        429 => CoreError::RateLimit {
            code: NetworkCode::RateLimit,
            retry_after_secs,
        },
        500..=599 => CoreError::ServiceUnavailable {
            code: ServiceCode::Unavailable,
            message: format!("context home unavailable ({status})"),
        },
        // Remaining 4xx are permanent: the same request returns the same answer.
        400..=499 => CoreError::Validation {
            code: ValidationCode::InvalidField,
            field: "request".to_string(),
            message: format!("context home request rejected ({status})"),
        },
        other => CoreError::Network {
            code: NetworkCode::Generic,
            message: format!("context home returned an unexpected status ({other})"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_RETRY_AFTER: u64 = 0;

    #[test]
    fn path_carries_no_identity_parameter() {
        // The moment the request can name its own subject, the only thing left
        // guarding the data is one server-side check.
        assert!(!CONTEXT_HOME_PATH.contains('?'));
        for forbidden in ["user_id", "organization_id", "actor_id", "org_id"] {
            assert!(
                !CONTEXT_HOME_PATH.contains(forbidden),
                "path must not carry {forbidden}"
            );
        }
    }

    #[test]
    fn session_expiry_and_permission_denial_are_different_answers() {
        // This split is the entire reason this module does not reuse
        // `check_response`. Merging 401 into 403 sends an unauthorized user to
        // a login screen, where nothing they can do will help.
        let unauthorized = map_failure_status(401, NO_RETRY_AFTER);
        let forbidden = map_failure_status(403, NO_RETRY_AFTER);

        assert_eq!(unauthorized.code(), "auth.failed");
        assert_eq!(forbidden.code(), "policy.denied");
        assert_ne!(unauthorized.code(), forbidden.code());
    }

    #[test]
    fn canonical_table_arms_match_the_guide() {
        // docs/guides/http-status-error-mapping.md — every arm except 403.
        for (status, expected) in [
            (401u16, "auth.failed"),
            (404, "not_found.resource_missing"),
            (408, "network.timeout"),
            (429, "network.rate_limit"),
            (502, "service.unavailable"),
            (503, "service.unavailable"),
            (504, "network.timeout"),
        ] {
            assert_eq!(
                map_failure_status(status, NO_RETRY_AFTER).code(),
                expected,
                "status {status}"
            );
        }
    }

    #[test]
    fn bare_500_is_retryable_not_a_permanent_internal() {
        // The #5069/#6078 trap: if a bare 500 lands in a non-retryable arm, a
        // transient server fault is indistinguishable from a permanent failure.
        assert_eq!(
            map_failure_status(500, NO_RETRY_AFTER).code(),
            "service.unavailable"
        );
    }

    #[test]
    fn permanent_client_errors_are_not_reported_as_transient() {
        // Folding 400/409/422 into unavailable or timeout makes callers retry forever.
        for status in [400, 409, 413, 422] {
            let code = map_failure_status(status, NO_RETRY_AFTER).code();
            assert_ne!(code, "service.unavailable", "{status}");
            assert_ne!(code, "network.timeout", "{status}");
            assert_eq!(code, "validation.invalid_field", "{status}");
        }
    }

    #[test]
    fn retry_after_seconds_survive_into_the_rate_limit_error() {
        // Reading the header and dropping it leaves the caller inventing a backoff.
        match map_failure_status(429, 42) {
            CoreError::RateLimit {
                retry_after_secs, ..
            } => assert_eq!(retry_after_secs, 42),
            other => panic!("429 must map to RateLimit, got {other}"),
        }
    }

    #[test]
    fn failure_messages_never_carry_a_bearer_or_server_body() {
        // These messages travel across IPC into the WebView.
        for status in [401, 403, 404, 408, 429, 500, 422, 600] {
            let msg = map_failure_status(status, NO_RETRY_AFTER).to_string();
            let lower = msg.to_lowercase();
            assert!(!lower.contains("bearer"), "{status}: {msg}");
            assert!(!lower.contains("authorization"), "{status}: {msg}");
            assert!(!msg.contains("eyJ"), "{status}: {msg}"); // JWT prefix
        }
    }

    #[test]
    fn body_cap_is_bounded_and_far_above_a_real_snapshot() {
        // The server bounds the item counts, so a real snapshot stays in the tens of KB.
        //
        // `const {}` because both sides are constants: CI runs clippy with
        // `-D warnings`, and `assertions_on_constants` fires on a plain
        // `assert!` here. Inside a const block the check moves to compile time,
        // which is also where a bound on a `const` belongs.
        const {
            assert!(MAX_CONTEXT_HOME_BYTES >= 512 * 1024);
            assert!(MAX_CONTEXT_HOME_BYTES <= 8 * 1024 * 1024);
        }
    }
}
