//! Google Calendar `events.list` HTTP transport (MK-EXT-01.C01 #8590).
//!
//! **Read-only**: this transport contract ([`CalendarEventsApi`]) has `list_events`
//! **only** — no create/update/delete verbs. There is no type-level path by which a
//! connector could perform a Calendar write (#8590 AC "No connector path can
//! perform a Calendar write").
//!
//! **Credential / SSRF defense**: the real transport ([`HttpCalendarEventsApi`])
//! disables redirects via
//! [`hardened_client_builder`](crate::outbound::hardened_client_builder) (credential
//! exfil defense) and enforces `https_only` for remote endpoints. The OAuth
//! bearer token is fetched from [`OAuthPort`] on every call and is never logged anywhere.
//!
//! **No leakage**: error response bodies are read, but only a **bounded reason token**
//! is extracted for classification and immediately discarded. The body, the token, and
//! any URL carrying the token are never left in logs or return values.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::header::RETRY_AFTER;
use serde::Deserialize;
use tracing::{debug, warn};

use maekon_core::error::CoreError;
use maekon_core::error_codes::NetworkCode;
use maekon_core::ports::oauth::OAuthPort;
use maekon_core::ports::work_context::SourceHealth;

use super::cursor::SyncCursor;
use super::health::classify_calendar_status;
use super::model::GoogleEventsResponse;
use crate::outbound::{hardened_client_builder, read_text_capped, BodyReadError, TransportPolicy};
use crate::resilience::MAX_RETRY_AFTER_SECS;

/// `events.list` response body cap (control-plane JSON is a few KB — 16 MiB guards against OOM).
const MAX_CALENDAR_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

/// Maximum length of the extracted error reason token (defensive clamp preventing body leakage).
const MAX_REASON_LEN: usize = 64;

/// Request parameters for a single `events.list` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListEventsRequest {
    pub calendar_id: String,
    pub max_results: u32,
    pub cursor: SyncCursor,
    /// Bounded past boundary of the initial collection (carried on `Initial` only).
    pub time_min: Option<DateTime<Utc>>,
    /// Bounded future boundary of the initial collection (carried on `Initial` only).
    pub time_max: Option<DateTime<Utc>>,
}

/// `events.list` result — either a parsed page or a typed health status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListEventsOutcome {
    Ok(GoogleEventsResponse),
    /// An unhealthy status surfaced as a typed value without the raw body.
    Unhealthy(SourceHealth),
}

/// Google Calendar event-listing transport port (**read-only**).
#[async_trait]
pub trait CalendarEventsApi: Send + Sync {
    /// Lists one page. Unhealthy statuses are surfaced as
    /// `Ok(ListEventsOutcome::Unhealthy(..))` rather than as `Err`.
    async fn list_events(&self, req: &ListEventsRequest) -> Result<ListEventsOutcome, CoreError>;
}

/// reqwest-based real transport.
pub struct HttpCalendarEventsApi {
    http: reqwest::Client,
    base_url: String,
    oauth: Arc<dyn OAuthPort>,
    provider_id: String,
}

/// Minimal parse schema for a Google error response (extracts the reason token only).
#[derive(Debug, Deserialize)]
struct GoogleErrorEnvelope {
    error: Option<GoogleErrorBody>,
}

#[derive(Debug, Deserialize)]
struct GoogleErrorBody {
    #[serde(default)]
    errors: Vec<GoogleErrorItem>,
}

#[derive(Debug, Deserialize)]
struct GoogleErrorItem {
    #[serde(default)]
    reason: Option<String>,
}

impl HttpCalendarEventsApi {
    /// Builds the real transport. `base_url` is the API root (e.g.
    /// `https://www.googleapis.com/calendar/v3`). Remote endpoints require `https_only`;
    /// loopback (dev/test) allows cleartext.
    pub fn new(
        base_url: impl Into<String>,
        oauth: Arc<dyn OAuthPort>,
        provider_id: impl Into<String>,
    ) -> Result<Self, CoreError> {
        let base_url = base_url.into();
        let http = hardened_client_builder(TransportPolicy::for_endpoint(&base_url))
            .build()
            .map_err(|e| CoreError::Network {
                code: NetworkCode::Generic,
                message: format!("failed to build calendar HTTP client: {e}"),
            })?;
        Ok(Self {
            http,
            base_url,
            oauth,
            provider_id: provider_id.into(),
        })
    }

    /// Assembles the request URL. Disallowed characters in the calendar id (`#`, spaces,
    /// etc.) are percent-encoded by `set_path`, and a trailing slash on the base is
    /// prevented from producing a double slash.
    fn build_url(&self, req: &ListEventsRequest) -> Result<url::Url, CoreError> {
        let mut url = url::Url::parse(&self.base_url).map_err(|e| CoreError::Network {
            code: NetworkCode::Generic,
            message: format!("invalid calendar base url: {e}"),
        })?;
        let base_path = url.path().trim_end_matches('/').to_string();
        url.set_path(&format!("{base_path}/calendars/{}/events", req.calendar_id));
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("singleEvents", "true");
            q.append_pair("maxResults", &req.max_results.to_string());
            match &req.cursor {
                SyncCursor::Initial => {
                    if let Some(tmin) = req.time_min {
                        q.append_pair("timeMin", &tmin.to_rfc3339());
                    }
                    if let Some(tmax) = req.time_max {
                        q.append_pair("timeMax", &tmax.to_rfc3339());
                    }
                }
                SyncCursor::Page(token) => {
                    q.append_pair("pageToken", token);
                }
                SyncCursor::Sync(token) => {
                    // A syncToken query cannot carry timeMin/timeMax/orderBy (Google contract).
                    q.append_pair("syncToken", token);
                    // Surface cancelled/deleted instances in the incremental window (explicit deletion signal).
                    q.append_pair("showDeleted", "true");
                }
            }
        }
        Ok(url)
    }
}

/// Parses the `Retry-After` header in seconds (`None` when absent, defensively clamped).
fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|s| s.min(MAX_RETRY_AFTER_SECS))
}

/// Extracts **only a bounded reason token** from the error body (the body itself is discarded).
///
/// The reason is a short identifier defined by Google (`rateLimitExceeded`, etc.). We
/// defensively clamp its length and strip any character other than alphanumerics/underscore,
/// so that corrupted/malicious body text cannot leak through the reason channel into logs.
fn extract_error_reason(body: &str) -> Option<String> {
    let parsed: GoogleErrorEnvelope = serde_json::from_str(body).ok()?;
    let raw = parsed
        .error?
        .errors
        .into_iter()
        .find_map(|e| e.reason)
        .filter(|r| !r.is_empty())?;
    let sanitized: String = raw
        .chars()
        .take(MAX_REASON_LEN)
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    (!sanitized.is_empty()).then_some(sanitized)
}

#[async_trait]
impl CalendarEventsApi for HttpCalendarEventsApi {
    async fn list_events(&self, req: &ListEventsRequest) -> Result<ListEventsOutcome, CoreError> {
        // 1. Fetch the bearer token — if absent, fail-closed (Unauthorized) before any network call.
        let Some(token) = self.oauth.get_access_token(&self.provider_id).await? else {
            debug!("google calendar: no access token — surfacing unauthorized");
            return Ok(ListEventsOutcome::Unhealthy(SourceHealth::Unauthorized));
        };

        let url = self.build_url(req)?;
        // 2. GET (the token goes in the header only — never in the URL or logs).
        let response = match self.http.get(url).bearer_auth(&token).send().await {
            Ok(r) => r,
            Err(e) => {
                // Transport error (connect/timeout/DNS). Its Display may carry a URL
                // containing the token, so never log it — record only the error kind as
                // structured fields.
                warn!(
                    is_timeout = e.is_timeout(),
                    is_connect = e.is_connect(),
                    "google calendar: transport error — surfacing offline"
                );
                return Ok(ListEventsOutcome::Unhealthy(SourceHealth::Offline));
            }
        };

        let status = response.status().as_u16();
        let retry_after = parse_retry_after(response.headers());

        // 3-a. Success → parse the body. A parse failure is malformed_page (body not logged).
        if (200..300).contains(&status) {
            let body = match read_text_capped(response, MAX_CALENDAR_RESPONSE_BYTES).await {
                Ok(b) => b,
                Err(BodyReadError::TooLarge { len, cap }) => {
                    warn!(len, cap, "google calendar: response exceeded cap");
                    return Ok(ListEventsOutcome::Unhealthy(SourceHealth::MalformedPage));
                }
                Err(BodyReadError::Transport(_)) => {
                    warn!("google calendar: body read transport error");
                    return Ok(ListEventsOutcome::Unhealthy(SourceHealth::Offline));
                }
            };
            return match serde_json::from_str::<GoogleEventsResponse>(&body) {
                Ok(parsed) => Ok(ListEventsOutcome::Ok(parsed)),
                Err(_) => {
                    // Do not log the body — only the fact that parsing failed.
                    warn!("google calendar: failed to parse events.list body");
                    Ok(ListEventsOutcome::Unhealthy(SourceHealth::MalformedPage))
                }
            };
        }

        // 3-b. Error → extract only the bounded reason token for classification. The body is discarded.
        let reason = read_text_capped(response, MAX_CALENDAR_RESPONSE_BYTES)
            .await
            .ok()
            .and_then(|body| extract_error_reason(&body));
        let health = classify_calendar_status(status, reason.as_deref(), retry_after)
            .unwrap_or(SourceHealth::ProviderUnavailable);
        // Log only the status code + the bounded reason token (no body/token/URL).
        warn!(
            status,
            reason = reason.as_deref().unwrap_or("none"),
            health = health.as_str(),
            "google calendar: events.list unhealthy"
        );
        Ok(ListEventsOutcome::Unhealthy(health))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_reason_pulls_bounded_token_only() {
        let body = r#"{"error":{"code":403,"message":"blah blah secret words",
            "errors":[{"domain":"usageLimits","reason":"userRateLimitExceeded"}]}}"#;
        assert_eq!(
            extract_error_reason(body).as_deref(),
            Some("userRateLimitExceeded")
        );
    }

    #[test]
    fn extract_reason_sanitizes_and_bounds_hostile_text() {
        // Arbitrary body text cannot leak through the reason channel: alphanumerics/underscore only, length clamped.
        let hostile = format!(
            r#"{{"error":{{"errors":[{{"reason":"{}"}}]}}}}"#,
            "tok en with spaces & <script> ".repeat(10)
        );
        let out = extract_error_reason(&hostile).unwrap();
        assert!(out.len() <= MAX_REASON_LEN);
        assert!(out.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        assert!(!out.contains(' '));
        assert!(!out.contains('<'));
    }

    #[test]
    fn extract_reason_none_for_non_error_body() {
        assert_eq!(extract_error_reason(r#"{"items":[]}"#), None);
        assert_eq!(extract_error_reason("not json"), None);
    }
}
