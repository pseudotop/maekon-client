//! Google event → `ContextSourceRecord`/`ProjectionContent` mapping
//! (MK-EXT-01.C01 #8590, ADR-030 §2/§5/§7).
//!
//! **metadata-first**: the record carries only bounded metadata —
//! event id, etag, updated, start/end, status, and the hash of the sanitized title.
//! Sensitive fields such as `description`, `attendees`, and `location` are **not
//! reflected in content_hash** and are never carried into the projection (exposed
//! as a summary only when there is a separate data-class consent).
//!
//! **Revision model = `Monotonic` (via `updated`)**: for a given event Google
//! guarantees that `updated` (the server-authoritative last-modified time)
//! increases monotonically. We take those millis as the canonical order value
//! (`source_order`) and declare `Monotonic`. **The local clock is never used for
//! ordering** — clock skew cannot reverse the revision order.
//!
//! **Identity**: `remote_id = instance id`. Each instance expanded by
//! `singleEvents=true` has a unique id, so it is a distinct identity. A moved
//! occurrence keeps the same instance id but has a higher `updated`, so it is
//! accepted as a **higher revision**. The recurring master is linked only via
//! `recurring_event_id` as a Parent relation (opaque id).

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use sha2::{Digest, Sha256};

use maekon_core::models::work_context::{
    ContextSourceRecord, DataClassification, Lifecycle, RelationKind, RelationRef,
    SourceObjectIdentity, WorkContextKind,
};
use maekon_core::models::work_context_projection::ProjectionContent;
use maekon_core::ports::work_context::{CommitContent, RawPayloadInput};

use super::model::{GoogleEvent, GoogleEventDateTime};

/// content_hash domain-separation tag.
const CONTENT_DOMAIN: &[u8] = b"google-calendar-content/v1\0";

/// Source identity context needed for the event → record mapping.
///
/// `account_subject_ref` is ADR-031's `account_id` itself (no re-hashing).
#[derive(Debug, Clone)]
pub struct GoogleCalendarMapCtx {
    pub extension_id: String,
    pub install_id: String,
    pub account_subject_ref: String,
    pub remote_type: String,
}

/// Sanitizes by replacing control characters with spaces and collapsing whitespace.
///
/// Neutralizing prompt-injection role-markers is the responsibility of the
/// suggestion pipeline's trust boundary (#8588), so here we strip only the
/// control characters and newlines that would break provenance/display.
fn sanitize_text(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Length-prefixed field encoding (removes boundary ambiguity — isomorphic to ADR-030 §4).
fn push_field(buf: &mut Vec<u8>, value: Option<&str>) {
    match value {
        Some(v) => {
            buf.push(b'P');
            buf.extend_from_slice(&(v.len() as u64).to_be_bytes());
            buf.extend_from_slice(v.as_bytes());
        }
        None => buf.push(b'N'),
    }
}

/// Builds the canonical representation of a time for the content fingerprint.
/// All-day vs. timed is distinguished by a prefix.
///
/// - timed: `dt:<UTC rfc3339>`
/// - all-day: `date:<YYYY-MM-DD>` (anchored to UTC midnight — see [`normalize_datetime`] below)
fn datetime_repr(dt: &GoogleEventDateTime) -> Option<String> {
    if let Some(raw) = &dt.date_time {
        let parsed = DateTime::parse_from_rfc3339(raw).ok()?.with_timezone(&Utc);
        Some(format!("dt:{}", parsed.to_rfc3339()))
    } else if let Some(date) = &dt.date {
        // Validate only the format and carry the original date verbatim into the representation.
        NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
        Some(format!("date:{date}"))
    } else {
        None
    }
}

/// Normalizes an event's start/end time to UTC.
///
/// - timed (`dateTime`, with offset): converted directly to UTC.
/// - all-day (`date`): **anchored to UTC midnight**. An all-day event is a floating
///   date, so it is fixed to 00:00:00Z for a timezone-independent, deterministic
///   `occurred_at`.
pub fn normalize_datetime(dt: &GoogleEventDateTime) -> Option<DateTime<Utc>> {
    if let Some(raw) = &dt.date_time {
        DateTime::parse_from_rfc3339(raw)
            .ok()
            .map(|t| t.with_timezone(&Utc))
    } else if let Some(date) = &dt.date {
        let naive = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
        let midnight = naive.and_hms_opt(0, 0, 0)?;
        Some(Utc.from_utc_datetime(&midnight))
    } else {
        None
    }
}

/// Turns `updated` (RFC3339) into the canonical order value (millis).
///
/// `updated` is the only value for which Google guarantees per-event monotonicity,
/// so it is the sole basis for ordering. On a parse failure (e.g. the minimal
/// payload of a cancelled instance) returns `None` — a tombstone is suppressed
/// independently of order by §6 rule 6, so this is safe.
pub fn source_order_from_updated(updated: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(updated)
        .ok()
        .map(|t| t.with_timezone(&Utc).timestamp_millis())
}

/// status → lifecycle. `cancelled` is an explicit delete signal (→ tombstone).
pub fn lifecycle_of(event: &GoogleEvent) -> Lifecycle {
    if event.is_cancelled() {
        Lifecycle::Deleted
    } else {
        Lifecycle::Active
    }
}

/// Hash (hex) of the sanitized, bounded content. **Sensitive fields not reflected.**
///
/// Carries only title, start/end, and status — `description`/`attendees`/`location`
/// are never included, so two events that differ only in sensitive fields have the
/// same content_hash (a structural proof that sensitive fields are excluded).
pub fn content_hash(event: &GoogleEvent) -> String {
    let title = event.summary.as_deref().map(sanitize_text);
    let start = event.start.as_ref().and_then(datetime_repr);
    let end = event.end.as_ref().and_then(datetime_repr);
    let status = lifecycle_of(event).as_str();

    let mut buf = Vec::new();
    buf.extend_from_slice(CONTENT_DOMAIN);
    push_field(&mut buf, title.as_deref());
    push_field(&mut buf, start.as_deref());
    push_field(&mut buf, end.as_deref());
    push_field(&mut buf, Some(status));

    let digest = Sha256::digest(&buf);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Maps a Google event to a metadata-first `ContextSourceRecord`.
pub fn event_to_record(
    event: &GoogleEvent,
    ctx: &GoogleCalendarMapCtx,
    observed_at: DateTime<Utc>,
) -> ContextSourceRecord {
    let identity = SourceObjectIdentity {
        extension_id: ctx.extension_id.clone(),
        install_id: ctx.install_id.clone(),
        account_subject_ref: ctx.account_subject_ref.clone(),
        remote_type: ctx.remote_type.clone(),
        remote_id: event.id.clone(),
    };

    let occurred_at = event.start.as_ref().and_then(normalize_datetime);
    let source_updated_at = event
        .updated
        .as_deref()
        .and_then(|u| DateTime::parse_from_rfc3339(u).ok())
        .map(|t| t.with_timezone(&Utc));
    let source_order = event.updated.as_deref().and_then(source_order_from_updated);

    // A recurring instance links its master only as an **opaque** Parent relation (no name/title).
    let mut relations = Vec::new();
    if let Some(master) = &event.recurring_event_id {
        relations.push(RelationRef {
            kind: RelationKind::Parent,
            opaque_source_id: master.clone(),
            fingerprint: None,
        });
    }

    ContextSourceRecord {
        identity,
        kind: WorkContextKind::Meeting,
        // The calendar title is treated as work-internal metadata. Sensitive raw
        // content is already excluded. (Can be raised via a per-calendar policy if needed.)
        classification: DataClassification::Internal,
        remote_revision: event.updated.clone(),
        etag: event.etag.clone(),
        source_order,
        content_hash: content_hash(event),
        occurred_at,
        source_updated_at,
        observed_at,
        relations,
        lifecycle: lifecycle_of(event),
        raw_payload_handle: None,
    }
}

/// Maps a Google event to a sanitized projection (§7).
///
/// Carries **only the title**. `description` is exposed as a summary only when
/// `sensitive_consent == true` — i.e. only with a separate data-class consent +
/// explicit scope. The default (false) excludes it. `attendees`/`location` are
/// not parsed in the first place and cannot reach here.
pub fn event_to_projection(
    event: &GoogleEvent,
    sensitive_consent: bool,
) -> Option<ProjectionContent> {
    let title = event
        .summary
        .as_deref()
        .map(sanitize_text)
        .filter(|s| !s.is_empty());
    let summary = if sensitive_consent {
        event
            .description
            .as_deref()
            .map(sanitize_text)
            .filter(|s| !s.is_empty())
    } else {
        None
    };
    if title.is_none() && summary.is_none() {
        return None;
    }
    Some(ProjectionContent {
        sanitized_title: title,
        sanitized_summary: summary,
    })
}

/// Event → `CommitContent` for the commit (projection only, no raw).
///
/// If the projection is empty (e.g. all-day/cancelled) returns `None` — an empty
/// projection is not written.
pub fn event_to_commit_content(
    event: &GoogleEvent,
    source_object_key: &str,
    sensitive_consent: bool,
) -> Option<CommitContent> {
    event_to_projection(event, sensitive_consent).map(|p| CommitContent {
        source_object_key: source_object_key.to_string(),
        projection: Some(p),
        raw_payload: None::<RawPayloadInput>,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> GoogleCalendarMapCtx {
        GoogleCalendarMapCtx {
            extension_id: "com.maekon.google_calendar".into(),
            install_id: "inst_1".into(),
            account_subject_ref: "acct_opaque_1".into(),
            remote_type: "event".into(),
        }
    }

    fn base_event() -> GoogleEvent {
        GoogleEvent {
            id: "evt_1".into(),
            status: Some("confirmed".into()),
            etag: Some("\"etag-a\"".into()),
            summary: Some("Design review".into()),
            updated: Some("2026-07-22T09:00:00Z".into()),
            start: Some(GoogleEventDateTime {
                date_time: Some("2026-07-22T10:00:00+09:00".into()),
                ..Default::default()
            }),
            end: Some(GoogleEventDateTime {
                date_time: Some("2026-07-22T11:00:00+09:00".into()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn record_carries_only_bounded_metadata() {
        let now = Utc::now();
        let rec = event_to_record(&base_event(), &ctx(), now);
        assert_eq!(rec.identity.remote_id, "evt_1");
        assert_eq!(rec.kind, WorkContextKind::Meeting);
        assert_eq!(rec.lifecycle, Lifecycle::Active);
        assert_eq!(rec.etag.as_deref(), Some("\"etag-a\""));
        // occurred_at is normalized to UTC (10:00 +09:00 → 01:00Z).
        assert_eq!(
            rec.occurred_at.unwrap().to_rfc3339(),
            "2026-07-22T01:00:00+00:00"
        );
        // source_order is the updated millis — the basis for monotonic ordering.
        assert_eq!(
            rec.source_order,
            Some(
                DateTime::parse_from_rfc3339("2026-07-22T09:00:00Z")
                    .unwrap()
                    .timestamp_millis()
            )
        );
    }

    #[test]
    fn content_hash_ignores_sensitive_fields() {
        // Two events that differ only in sensitive fields have the same content_hash.
        let mut with_secrets = base_event();
        with_secrets.description = Some("Salary negotiation with Bob".into());
        let plain = base_event();
        assert_eq!(content_hash(&with_secrets), content_hash(&plain));

        // If the title changes, the content_hash changes too.
        let mut retitled = base_event();
        retitled.summary = Some("Budget review".into());
        assert_ne!(content_hash(&retitled), content_hash(&plain));
    }

    #[test]
    fn projection_excludes_sensitive_description_without_consent() {
        let mut ev = base_event();
        ev.description = Some("Salary negotiation with Bob".into());
        // No consent (default): title only, description excluded.
        let p = event_to_projection(&ev, false).unwrap();
        assert_eq!(p.sanitized_title.as_deref(), Some("Design review"));
        assert_eq!(p.sanitized_summary, None);
        // With consent: description is exposed as a summary.
        let p2 = event_to_projection(&ev, true).unwrap();
        assert_eq!(
            p2.sanitized_summary.as_deref(),
            Some("Salary negotiation with Bob")
        );
    }

    #[test]
    fn cancelled_event_maps_to_deleted_tombstone() {
        let mut ev = base_event();
        ev.status = Some("cancelled".into());
        let rec = event_to_record(&ev, &ctx(), Utc::now());
        assert_eq!(rec.lifecycle, Lifecycle::Deleted);
    }

    #[test]
    fn all_day_event_anchors_to_utc_midnight() {
        let mut ev = base_event();
        ev.start = Some(GoogleEventDateTime {
            date: Some("2026-07-22".into()),
            ..Default::default()
        });
        ev.end = None;
        let rec = event_to_record(&ev, &ctx(), Utc::now());
        assert_eq!(
            rec.occurred_at.unwrap().to_rfc3339(),
            "2026-07-22T00:00:00+00:00"
        );
    }

    #[test]
    fn recurring_instance_links_master_as_opaque_parent() {
        let mut ev = base_event();
        ev.id = "master_20260722T010000Z".into();
        ev.recurring_event_id = Some("master".into());
        let rec = event_to_record(&ev, &ctx(), Utc::now());
        assert_eq!(rec.relations.len(), 1);
        assert_eq!(rec.relations[0].kind, RelationKind::Parent);
        assert_eq!(rec.relations[0].opaque_source_id, "master");
        // The relation carries no title/name (opaque id only).
        assert_eq!(rec.relations[0].fingerprint, None);
    }

    #[test]
    fn moved_occurrence_keeps_identity_and_bumps_revision() {
        // Moved occurrence: same instance id, changed start + higher updated.
        let mut original = base_event();
        original.id = "master_20260722T010000Z".into();
        original.updated = Some("2026-07-20T00:00:00Z".into());
        let mut moved = original.clone();
        moved.start = Some(GoogleEventDateTime {
            date_time: Some("2026-07-22T14:00:00+09:00".into()),
            ..Default::default()
        });
        moved.updated = Some("2026-07-21T00:00:00Z".into());

        let r0 = event_to_record(&original, &ctx(), Utc::now());
        let r1 = event_to_record(&moved, &ctx(), Utc::now());
        // Same identity (same instance id).
        assert_eq!(r0.identity.remote_id, r1.identity.remote_id);
        // Higher revision (larger updated millis) — on merge the moved one wins.
        assert!(r1.source_order.unwrap() > r0.source_order.unwrap());
        // The time changed, so the content_hash differs too.
        assert_ne!(r0.content_hash, r1.content_hash);
    }

    #[test]
    fn distinct_occurrences_have_distinct_identities() {
        let mut occ_a = base_event();
        occ_a.id = "master_20260722T010000Z".into();
        occ_a.recurring_event_id = Some("master".into());
        let mut occ_b = base_event();
        occ_b.id = "master_20260723T010000Z".into();
        occ_b.recurring_event_id = Some("master".into());
        let ra = event_to_record(&occ_a, &ctx(), Utc::now());
        let rb = event_to_record(&occ_b, &ctx(), Utc::now());
        assert_ne!(ra.identity.remote_id, rb.identity.remote_id);
    }

    #[test]
    fn sanitize_strips_control_characters() {
        assert_eq!(
            sanitize_text("Weekly\n\tsync  meeting"),
            "Weekly sync meeting"
        );
    }
}
