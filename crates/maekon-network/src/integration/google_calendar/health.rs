//! HTTP status → typed `SourceHealth` classification + recommended backoff (MK-EXT-01.C01 #8590).
//!
//! **No-leakage invariant (ADR-030 §9)**: no return value in this module carries a raw
//! error body, a token, or a URL containing a token. Classification uses only the status
//! code plus the provider-supplied **bounded reason token** (e.g. `rateLimitExceeded`) and
//! the `Retry-After` seconds. `SourceHealth` structurally has no string body field, so even
//! if a value classified here flows to logs/audit/console, there is no type-level path by
//! which the body could leak.

use std::time::Duration;

use maekon_core::ports::work_context::SourceHealth;

use crate::resilience::{jittered_backoff_delay, MAX_RETRY_AFTER_SECS};

/// The set of bounded reason tokens that indicate rate-limiting on a Google 403.
///
/// A 403 may mean insufficient permissions (`insufficientPermissions`) or rate limiting
/// (`rateLimitExceeded`, etc.). We distinguish them by the reason token alone.
const RATE_LIMIT_REASONS: &[&str] = &[
    "rateLimitExceeded",
    "userRateLimitExceeded",
    "dailyLimitExceeded",
    "quotaExceeded",
];

/// Classifies an HTTP status (+ optional reason token / Retry-After) into a typed `SourceHealth`.
///
/// 2xx returns `None` (healthy). Everything else is always `Some(health)`.
///
/// - 401 → `Unauthorized` (re-auth required, retrying is pointless)
/// - 403 → `RateLimited` if the reason is in the rate-limit family, otherwise `Forbidden`
/// - 404 → `Forbidden` (target calendar is inaccessible — user action required)
/// - 410 → `CursorExpired` (expired syncToken → full resync)
/// - 429 → `RateLimited`
/// - 5xx → `ProviderUnavailable` (transient outage)
/// - any other unexpected code → `ProviderUnavailable` (retry after bounded backoff,
///   avoiding permanent blocking)
pub fn classify_calendar_status(
    status: u16,
    reason: Option<&str>,
    retry_after_secs: Option<u64>,
) -> Option<SourceHealth> {
    if (200..300).contains(&status) {
        return None;
    }
    let health = match status {
        401 => SourceHealth::Unauthorized,
        403 => {
            if reason.is_some_and(|r| RATE_LIMIT_REASONS.contains(&r)) {
                SourceHealth::RateLimited { retry_after_secs }
            } else {
                SourceHealth::Forbidden
            }
        }
        404 => SourceHealth::Forbidden,
        410 => SourceHealth::CursorExpired,
        429 => SourceHealth::RateLimited { retry_after_secs },
        s if (500..600).contains(&s) => SourceHealth::ProviderUnavailable,
        // Unknown 4xx/other — treat as a bounded-backoff target instead of permanent blocking.
        _ => SourceHealth::ProviderUnavailable,
    };
    Some(health)
}

/// Computes the recommended retry delay for an unhealthy status (reuses `resilience.rs`).
///
/// - `RateLimited{retry_after}` prefers the provider-supplied `Retry-After` but defensively
///   clamps it with [`MAX_RETRY_AFTER_SECS`] (so a malicious/buggy value cannot block for years).
/// - Other retryable states use jittered exponential backoff via [`jittered_backoff_delay`].
/// - Re-auth / insufficient permission (`should_retry()==false`) yields `None` — retrying is
///   itself pointless, so the backoff loop does not exhaust the provider's quota.
pub fn recommended_backoff(health: &SourceHealth, attempt: u32) -> Option<Duration> {
    if !health.should_retry() {
        return None;
    }
    match health {
        SourceHealth::RateLimited {
            retry_after_secs: Some(secs),
        } => Some(Duration::from_secs((*secs).min(MAX_RETRY_AFTER_SECS))),
        _ => Some(jittered_backoff_delay(
            attempt,
            Duration::from_secs(2),
            Duration::from_secs(MAX_RETRY_AFTER_SECS),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_status_is_not_unhealthy() {
        assert_eq!(classify_calendar_status(200, None, None), None);
        assert_eq!(classify_calendar_status(204, None, None), None);
    }

    #[test]
    fn unauthorized_and_forbidden_need_user_action() {
        assert_eq!(
            classify_calendar_status(401, None, None),
            Some(SourceHealth::Unauthorized)
        );
        // 403 without a rate-limit reason = genuine permission problem.
        assert_eq!(
            classify_calendar_status(403, Some("insufficientPermissions"), None),
            Some(SourceHealth::Forbidden)
        );
        assert_eq!(
            classify_calendar_status(404, None, None),
            Some(SourceHealth::Forbidden)
        );
    }

    #[test]
    fn rate_limit_403_reason_maps_to_rate_limited() {
        // Google also sends rate-limiting as 403 + reason.
        assert_eq!(
            classify_calendar_status(403, Some("userRateLimitExceeded"), Some(30)),
            Some(SourceHealth::RateLimited {
                retry_after_secs: Some(30)
            })
        );
        assert_eq!(
            classify_calendar_status(429, None, Some(45)),
            Some(SourceHealth::RateLimited {
                retry_after_secs: Some(45)
            })
        );
    }

    #[test]
    fn expired_sync_token_maps_to_cursor_expired() {
        assert_eq!(
            classify_calendar_status(410, None, None),
            Some(SourceHealth::CursorExpired)
        );
    }

    #[test]
    fn server_errors_map_to_provider_unavailable() {
        for s in [500u16, 502, 503, 504] {
            assert_eq!(
                classify_calendar_status(s, None, None),
                Some(SourceHealth::ProviderUnavailable)
            );
        }
    }

    #[test]
    fn backoff_honors_retry_after_and_clamps() {
        let rl = SourceHealth::RateLimited {
            retry_after_secs: Some(30),
        };
        assert_eq!(recommended_backoff(&rl, 0), Some(Duration::from_secs(30)));

        // A malicious Retry-After is defensively clamped.
        let abusive = SourceHealth::RateLimited {
            retry_after_secs: Some(4_294_967_295),
        };
        assert_eq!(
            recommended_backoff(&abusive, 0),
            Some(Duration::from_secs(MAX_RETRY_AFTER_SECS))
        );
    }

    #[test]
    fn no_backoff_for_states_that_need_user_action() {
        // Re-auth / insufficient permission cannot be resolved by backoff retries.
        assert_eq!(recommended_backoff(&SourceHealth::Unauthorized, 3), None);
        assert_eq!(recommended_backoff(&SourceHealth::Forbidden, 3), None);
    }

    #[test]
    fn provider_unavailable_uses_jittered_backoff() {
        let delay = recommended_backoff(&SourceHealth::ProviderUnavailable, 3)
            .expect("provider-unavailable is retryable");
        // Falls within the bounds of jittered exponential backoff (base=2s, max=60s).
        assert!(delay >= Duration::from_secs(16));
        assert!(delay <= Duration::from_secs(MAX_RETRY_AFTER_SECS));
    }
}
