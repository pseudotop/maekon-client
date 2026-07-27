//! Google Calendar `events.list` response DTO (MK-EXT-01.C01 #8590).
//!
//! **Boundary principle (ADR-030 §2)**: this DTO declares only the fields the
//! connector **chooses to parse**. serde silently drops undeclared fields, so
//! sensitive fields such as `attendees`, `location`, `creator`, `organizer`,
//! `conferenceData`, and `hangoutLink` are **not deserialized in the first place**
//! — there is structurally no path for them to reach the record/envelope/projection.
//!
//! Only `description` is declared as an exception. This is so the mapper can gate
//! it, exposing it as a sanitized summary only when there is a separate data-class
//! consent (explicit scope). Without consent (the default) the mapper drops this
//! field ([`super::mapping::event_to_projection`]).

use serde::{Deserialize, Serialize};

/// A single page of the `events.list` response.
///
/// - `next_page_token`: a signal that another page remains within the same sync window.
/// - `next_sync_token`: an incremental cursor issued **only on the last page** (Google-native).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct GoogleEventsResponse {
    #[serde(default)]
    pub items: Vec<GoogleEvent>,
    #[serde(rename = "nextPageToken", default)]
    pub next_page_token: Option<String>,
    #[serde(rename = "nextSyncToken", default)]
    pub next_sync_token: Option<String>,
}

/// A single (expanded) event instance.
///
/// Since it is queried with `singleEvents=true`, a recurring schedule is expanded
/// into instances, each with a unique `id` (e.g. `<master>_20260722T090000Z`). The
/// instance `id` is the stable identity, and the recurring master is linked via
/// `recurring_event_id`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct GoogleEvent {
    #[serde(default)]
    pub id: String,
    /// `confirmed` | `tentative` | `cancelled`. `cancelled` is an **explicit delete
    /// signal** (ADR-030 revision I6) — not an inference from absence in the listing.
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub etag: Option<String>,
    /// Event title. The only content subject to sanitization that is included (not a sensitive field).
    #[serde(default)]
    pub summary: Option<String>,
    /// Server-authoritative last-modified time (RFC3339). For a given event Google
    /// guarantees this value **increases monotonically** — the sole basis for
    /// `RevisionModel::Monotonic`.
    #[serde(default)]
    pub updated: Option<String>,
    #[serde(default)]
    pub start: Option<GoogleEventDateTime>,
    #[serde(default)]
    pub end: Option<GoogleEventDateTime>,
    /// The recurring master's id (only for an instance). Linked via a Parent relation.
    #[serde(rename = "recurringEventId", default)]
    pub recurring_event_id: Option<String>,
    /// The slot this instance originally occupied. Used to detect a moved occurrence.
    #[serde(rename = "originalStartTime", default)]
    pub original_start_time: Option<GoogleEventDateTime>,
    /// **Sensitive field**. Excluded by the mapper by default — exposed as a
    /// sanitized summary only when there is a separate data-class consent (ADR-030 §7, #8590).
    #[serde(default)]
    pub description: Option<String>,
}

impl GoogleEvent {
    /// Whether `status == "cancelled"` — an explicit delete/cancel signal.
    pub fn is_cancelled(&self) -> bool {
        self.status.as_deref() == Some("cancelled")
    }
}

/// Event start/end time.
///
/// `date_time` (RFC3339, with offset) is a timed schedule, and `date`
/// ("YYYY-MM-DD") is an all-day schedule. Only one of the two is present.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct GoogleEventDateTime {
    #[serde(rename = "dateTime", default)]
    pub date_time: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(rename = "timeZone", default)]
    pub time_zone: Option<String>,
}

impl GoogleEventDateTime {
    /// Whether this is an all-day schedule (a date only, with no time).
    pub fn is_all_day(&self) -> bool {
        self.date_time.is_none() && self.date.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_sensitive_fields_are_dropped_at_parse_time() {
        // attendees, location, and creator are not declared in the DTO, so
        // deserialization itself drops them — structural sensitive-field exclusion (ADR-030 §2).
        let raw = r#"{
            "items": [{
                "id": "evt_1",
                "status": "confirmed",
                "summary": "Design review",
                "updated": "2026-07-22T09:00:00Z",
                "description": "secret agenda text",
                "location": "Building 7, secret room",
                "attendees": [{"email": "alice@example.com"}, {"email": "bob@example.com"}],
                "creator": {"email": "carol@example.com"}
            }],
            "nextSyncToken": "tok_sync_1"
        }"#;
        let parsed: GoogleEventsResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.items.len(), 1);
        let ev = &parsed.items[0];
        assert_eq!(ev.id, "evt_1");
        assert_eq!(ev.summary.as_deref(), Some("Design review"));
        // description is parsed (it is gated), but the mapper excludes it without consent.
        assert_eq!(ev.description.as_deref(), Some("secret agenda text"));
        // attendees/location/creator were never parsed at all — no trace even when reserialized.
        let reserialized = serde_json::to_string(ev).unwrap();
        assert!(!reserialized.contains("alice@example.com"));
        assert!(!reserialized.contains("bob@example.com"));
        assert!(!reserialized.contains("carol@example.com"));
        assert!(!reserialized.contains("secret room"));
        assert_eq!(parsed.next_sync_token.as_deref(), Some("tok_sync_1"));
    }

    #[test]
    fn all_day_detection() {
        let all_day = GoogleEventDateTime {
            date: Some("2026-07-22".into()),
            ..Default::default()
        };
        let timed = GoogleEventDateTime {
            date_time: Some("2026-07-22T09:00:00+09:00".into()),
            ..Default::default()
        };
        assert!(all_day.is_all_day());
        assert!(!timed.is_all_day());
    }
}
