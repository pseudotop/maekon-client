//! Cursor state machine — Google page token ↔ incremental syncToken (MK-EXT-01.C01 #8590).
//!
//! `ContextSourcePort` can only pass a single opaque `Option<String>` cursor
//! (ADR-030 §9), yet Google uses **two kinds** of cursor:
//!
//! - `pageToken`: pagination within one collection window (up to the last page).
//! - `syncToken`: the incremental cursor issued on the last page (the start point of the next run).
//!
//! This module encodes both into a single string with a prefix (`page:` / `sync:`) so the
//! store can round-trip it opaquely. If a stored cursor is a corrupted/unknown format, we
//! defensively degrade to `Initial` — a full resync is always a safe recovery.

use chrono::{DateTime, Duration, Utc};

/// Interpretation of the cursor at the start of a collection run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncCursor {
    /// Initial collection. Queries with the bounded `timeMin`/`timeMax` window.
    Initial,
    /// Next page of the same window. Continues querying with `pageToken`.
    Page(String),
    /// Incremental collection. Queries with `syncToken`; on expiry a 410 Gone arrives.
    Sync(String),
}

impl SyncCursor {
    const PAGE_PREFIX: &'static str = "page:";
    const SYNC_PREFIX: &'static str = "sync:";

    /// Interprets a stored opaque cursor.
    ///
    /// Unknown/corrupted formats degrade to `Initial` (fail-safe full resync). Even if the
    /// token itself contains `:`, only the leading prefix is stripped, so it is safe.
    pub fn decode(raw: Option<&str>) -> Self {
        match raw {
            None => Self::Initial,
            Some(s) => {
                if let Some(tok) = s.strip_prefix(Self::PAGE_PREFIX) {
                    Self::Page(tok.to_string())
                } else if let Some(tok) = s.strip_prefix(Self::SYNC_PREFIX) {
                    Self::Sync(tok.to_string())
                } else {
                    // Stale/corrupted cursor without a prefix — safely degrade to a full resync.
                    Self::Initial
                }
            }
        }
    }

    /// Encodes a page token into a storable opaque cursor.
    pub fn encode_page(token: &str) -> String {
        format!("{}{token}", Self::PAGE_PREFIX)
    }

    /// Encodes a syncToken into a storable opaque cursor.
    pub fn encode_sync(token: &str) -> String {
        format!("{}{token}", Self::SYNC_PREFIX)
    }

    /// Whether this is an incremental (syncToken) cursor.
    pub fn is_incremental(&self) -> bool {
        matches!(self, Self::Sync(_))
    }
}

/// Computes the bounded past/future window for the initial collection (prevents unbounded backfill, ADR-030 §9).
///
/// `timeMin = now - past_days`, `timeMax = now + future_days`. Once a syncToken is issued,
/// Google remembers this window, so incremental queries do not carry the window again.
pub fn historical_window(
    now: DateTime<Utc>,
    past_days: i64,
    future_days: i64,
) -> (DateTime<Utc>, DateTime<Utc>) {
    let past = past_days.max(0);
    let future = future_days.max(0);
    (now - Duration::days(past), now + Duration::days(future))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_decodes_to_initial() {
        assert_eq!(SyncCursor::decode(None), SyncCursor::Initial);
    }

    #[test]
    fn page_and_sync_round_trip() {
        let p = SyncCursor::encode_page("tok_page_A");
        let s = SyncCursor::encode_sync("tok_sync_B");
        assert_eq!(
            SyncCursor::decode(Some(&p)),
            SyncCursor::Page("tok_page_A".into())
        );
        assert_eq!(
            SyncCursor::decode(Some(&s)),
            SyncCursor::Sync("tok_sync_B".into())
        );
        assert!(SyncCursor::decode(Some(&s)).is_incremental());
        assert!(!SyncCursor::decode(Some(&p)).is_incremental());
    }

    #[test]
    fn token_containing_colon_survives_round_trip() {
        // Even if a Google token contains ':', only the leading prefix must be stripped.
        let weird = "CiAK:Eg=abc:123";
        let enc = SyncCursor::encode_sync(weird);
        assert_eq!(
            SyncCursor::decode(Some(&enc)),
            SyncCursor::Sync(weird.into())
        );
    }

    #[test]
    fn unknown_prefix_degrades_to_initial() {
        // Corrupted/unknown formats silently degrade to a full resync (fail-safe).
        assert_eq!(
            SyncCursor::decode(Some("garbage-no-prefix")),
            SyncCursor::Initial
        );
        assert_eq!(SyncCursor::decode(Some("")), SyncCursor::Initial);
    }

    #[test]
    fn historical_window_is_bounded() {
        let now = DateTime::parse_from_rfc3339("2026-07-22T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let (min, max) = historical_window(now, 30, 90);
        assert_eq!(min, now - Duration::days(30));
        assert_eq!(max, now + Duration::days(90));
        // Negative inputs are clamped to 0 — the window does not invert.
        let (min2, max2) = historical_window(now, -5, -5);
        assert_eq!(min2, now);
        assert_eq!(max2, now);
    }
}
