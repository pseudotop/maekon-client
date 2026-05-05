use std::time::Duration;

use rand::RngExt;
use reqwest::header::RETRY_AFTER;
use tokio::time::Instant;

use crate::error::NetworkError;

const DEFAULT_RETRY_AFTER_SECS: u64 = 60;
const MAX_BACKOFF_EXPONENT: u32 = 10;

pub fn extract_retry_after(response: &reqwest::Response) -> u64 {
    response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_RETRY_AFTER_SECS)
}

pub fn scale_duration(duration: Duration, factor: u32) -> Duration {
    let scaled_ms = duration
        .as_millis()
        .saturating_mul(factor as u128)
        .min(u64::MAX as u128) as u64;
    Duration::from_millis(scaled_ms)
}

pub fn jittered_backoff_delay(attempt: u32, base: Duration, max: Duration) -> Duration {
    let base_ms = base.as_millis().min(u64::MAX as u128) as u64;
    let max_ms = max.as_millis().min(u64::MAX as u128) as u64;
    if base_ms == 0 || max_ms == 0 {
        return Duration::from_millis(0);
    }

    let exp_ms = base_ms.saturating_mul(2u64.saturating_pow(attempt.min(MAX_BACKOFF_EXPONENT)));
    let jitter_max_ms = exp_ms / 4;
    let jitter_ms = if jitter_max_ms == 0 {
        0
    } else {
        let mut rng = rand::rng();
        rng.random_range(0..=jitter_max_ms)
    };

    Duration::from_millis(exp_ms.saturating_add(jitter_ms).min(max_ms))
}

#[derive(Debug, Clone)]
pub struct RetryBackoffPolicy {
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl RetryBackoffPolicy {
    pub fn new(base_delay: Duration, max_delay: Duration) -> Self {
        Self {
            base_delay,
            max_delay,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RetryBackoffGate {
    policy: RetryBackoffPolicy,
    consecutive_failures: u32,
    blocked_until: Option<Instant>,
}

impl RetryBackoffGate {
    pub fn new(policy: RetryBackoffPolicy) -> Self {
        Self {
            policy,
            consecutive_failures: 0,
            blocked_until: None,
        }
    }

    pub fn is_ready(&self, now: Instant) -> bool {
        self.blocked_until.map_or(true, |until| now >= until)
    }

    pub fn on_success(&mut self) {
        self.consecutive_failures = 0;
        self.blocked_until = None;
    }

    pub fn on_failure(&mut self, now: Instant, error: &NetworkError) -> Duration {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        let delay = match error {
            NetworkError::RateLimited { retry_after_secs } => {
                Duration::from_secs(*retry_after_secs)
            }
            _ => jittered_backoff_delay(
                self.consecutive_failures.saturating_sub(1),
                self.policy.base_delay,
                self.policy.max_delay,
            ),
        };
        self.blocked_until = Some(now + delay);
        delay
    }
}

/// Outcome of a network call from the circuit breaker's perspective.
///
/// Used by adapters wired to `CircuitBreakerRegistry` to classify HTTP
/// responses before recording into the per-endpoint `CircuitBreaker`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerSignal {
    Success,
    Failure,
    /// Caller-side bug or ambiguous response — do not affect breaker state.
    Neutral,
}

/// Classify an HTTP outcome for circuit breaker accounting.
///
/// - `status: Some(s)`: HTTP response status code observed.
/// - `status: None`: no response received.
/// - `transport_err: true`: DNS/connect/read error (takes precedence over status).
///
/// Rules:
/// - 2xx → `Success`.
/// - 5xx, 401, 429, or transport error → `Failure` (endpoint health concern).
/// - 4xx other than 401/429 → `Neutral` (caller-side bug; must not trip the
///   shared breaker for every other caller against the same endpoint).
pub fn classify_for_breaker(status: Option<u16>, transport_err: bool) -> BreakerSignal {
    if transport_err {
        return BreakerSignal::Failure;
    }
    match status {
        Some(s) if (200..300).contains(&s) => BreakerSignal::Success,
        Some(401) | Some(429) => BreakerSignal::Failure,
        Some(s) if s >= 500 => BreakerSignal::Failure,
        Some(_) => BreakerSignal::Neutral,
        None => BreakerSignal::Failure,
    }
}

/// Returns `"scheme://host:port"` — path/query/fragment stripped.
/// Port canonicalizes to the scheme default if absent (https → 443, http → 80),
/// so `https://api.openai.com/v1/chat` and `https://api.openai.com:443/v1/embeddings`
/// produce the same key and share one `CircuitBreaker`.
///
/// # Errors
/// Returns `url::ParseError` when the input is not a valid absolute URL.
pub fn endpoint_authority(url: &str) -> Result<String, url::ParseError> {
    let parsed = ::url::Url::parse(url)?;
    let scheme = parsed.scheme();
    let host = parsed.host_str().unwrap_or("");
    let port = parsed.port_or_known_default().unwrap_or(0);
    Ok(format!("{scheme}://{host}:{port}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jittered_backoff_is_bounded() {
        let delay = jittered_backoff_delay(3, Duration::from_secs(2), Duration::from_secs(30));
        assert!(delay >= Duration::from_secs(16));
        assert!(delay <= Duration::from_secs(20));
    }

    #[test]
    fn scaled_duration_scales_millis() {
        let delay = scale_duration(Duration::from_millis(250), 8);
        assert_eq!(delay, Duration::from_secs(2));
    }

    #[tokio::test]
    async fn retry_gate_blocks_until_backoff_expires() {
        let mut gate = RetryBackoffGate::new(RetryBackoffPolicy::new(
            Duration::from_millis(10),
            Duration::from_millis(100),
        ));
        let now = Instant::now();
        let delay = gate.on_failure(now, &NetworkError::ServiceUnavailable("down".to_string()));
        assert!(delay >= Duration::from_millis(10));
        assert!(!gate.is_ready(now));
        assert!(gate.is_ready(now + delay));

        gate.on_success();
        assert!(gate.is_ready(Instant::now()));
    }

    // ── classify_for_breaker ────────────────────────────────────────────

    #[test]
    fn classify_success_on_2xx() {
        assert_eq!(
            classify_for_breaker(Some(200), false),
            BreakerSignal::Success
        );
        assert_eq!(
            classify_for_breaker(Some(201), false),
            BreakerSignal::Success
        );
        assert_eq!(
            classify_for_breaker(Some(299), false),
            BreakerSignal::Success
        );
    }

    #[test]
    fn classify_failure_on_5xx() {
        assert_eq!(
            classify_for_breaker(Some(500), false),
            BreakerSignal::Failure
        );
        assert_eq!(
            classify_for_breaker(Some(502), false),
            BreakerSignal::Failure
        );
        assert_eq!(
            classify_for_breaker(Some(503), false),
            BreakerSignal::Failure
        );
    }

    #[test]
    fn classify_failure_on_transport_error() {
        assert_eq!(classify_for_breaker(None, true), BreakerSignal::Failure);
        // transport_err takes precedence even when status is present:
        assert_eq!(
            classify_for_breaker(Some(200), true),
            BreakerSignal::Failure
        );
    }

    #[test]
    fn classify_failure_on_auth_and_rate_limit() {
        assert_eq!(
            classify_for_breaker(Some(401), false),
            BreakerSignal::Failure
        );
        assert_eq!(
            classify_for_breaker(Some(429), false),
            BreakerSignal::Failure
        );
    }

    #[test]
    fn classify_neutral_on_4xx_caller_bug() {
        // 400, 404, 422 are caller-side / ambiguous — must not trip the shared breaker.
        assert_eq!(
            classify_for_breaker(Some(400), false),
            BreakerSignal::Neutral
        );
        assert_eq!(
            classify_for_breaker(Some(404), false),
            BreakerSignal::Neutral
        );
        assert_eq!(
            classify_for_breaker(Some(422), false),
            BreakerSignal::Neutral
        );
    }

    // ── endpoint_authority ──────────────────────────────────────────────

    #[test]
    fn authority_canonicalizes_default_https_port() {
        assert_eq!(
            endpoint_authority("https://api.openai.com/v1/chat").unwrap(),
            "https://api.openai.com:443"
        );
    }

    #[test]
    fn authority_canonicalizes_default_http_port() {
        assert_eq!(
            endpoint_authority("http://localhost/health").unwrap(),
            "http://localhost:80"
        );
    }

    #[test]
    fn authority_preserves_explicit_port() {
        assert_eq!(
            endpoint_authority("http://localhost:11434/api/generate").unwrap(),
            "http://localhost:11434"
        );
    }

    #[test]
    fn authority_collapses_path_and_query() {
        let a = endpoint_authority("https://api.openai.com/v1/embeddings?model=foo").unwrap();
        let b = endpoint_authority("https://api.openai.com/v1/chat").unwrap();
        assert_eq!(a, b);
        assert_eq!(a, "https://api.openai.com:443");
    }

    #[test]
    fn authority_rejects_malformed_url() {
        assert!(endpoint_authority("not a url").is_err());
    }
}
