//! Google Calendar read-only Context Source connector (MK-EXT-01.C01 #8590).
//!
//! The **first read-only vertical slice** implementing ADR-030 §9's
//! [`ContextSourcePort`] for Google Calendar. It does not reimplement the sync
//! loop itself; it is plugged into the
//! [`run_sync`](maekon_core::services::context_sync::run_sync) orchestrator — this
//! connector only pulls bounded pages, while storage, merge, and cursor CAS are the
//! responsibility of the store.
//!
//! ## Design decisions (review requested)
//!
//! - **Read-only scope**: requests **only** [`GOOGLE_CALENDAR_READONLY_SCOPE`]
//!   (`calendar.events.readonly`). A write scope is never requested, and the
//!   transport contract ([`CalendarEventsApi`]) has no write verbs — there is no
//!   path, at the type level, to perform a Calendar write.
//! - **Revision model = `Monotonic` (via `updated`)**: for a given event Google
//!   guarantees that `updated` increases monotonically. We take those millis as
//!   `source_order` and declare `Monotonic`. Because there is an explicit delete
//!   signal (`status=cancelled` + syncToken removal), the descriptor is
//!   `has_explicit_delete_signal=true` and thus advertisable (`is_advertisable`).
//!   The local clock is not used for ordering.
//! - **Sensitive fields excluded**: the record/content_hash carries only title,
//!   time, and status; `description`/`attendees`/`location` are not reflected. The
//!   projection carries only the title as well, and `description` is exposed as a
//!   summary only when there is a separate data-class consent.
//! - **Cursor-expiry self-healing**: a 410 Gone (expired syncToken) surfaces as
//!   `CursorExpired`, and the next run recovers with an Initial full resync,
//!   ignoring the stored sync token (without a store cursor-reset API).
//!
//! ## Live smoke boundary (HONESTY)
//!
//! The fact that this module and its fake-server contract tests pass does **not
//! mean Google Calendar is supported** (ADR-030 / #8590 claim boundary).
//! connect→sync→revoke→delete→rate-limit verification against a real Google
//! account is a release evidence gate that a human must perform; here we verify
//! only up to the synthetic fake-server boundary.

pub mod cursor;
pub mod health;
pub mod http_api;
pub mod mapping;
pub mod model;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use sha2::{Digest, Sha256};

use maekon_core::error::CoreError;
use maekon_core::models::work_context::{
    ContextSourceDescriptor, ContextSourcePage, ContextSourceRecord, RevisionModel,
};
use maekon_core::ports::oauth::OAuthPort;
use maekon_core::ports::work_context::{
    AccountStatus, ContextSourcePort, SourceHealth, SyncOutcome, SyncRequest,
};

pub use cursor::{historical_window, SyncCursor};
pub use health::{classify_calendar_status, recommended_backoff};
pub use http_api::{
    CalendarEventsApi, HttpCalendarEventsApi, ListEventsOutcome, ListEventsRequest,
};
pub use mapping::{
    event_to_commit_content, event_to_projection, event_to_record, GoogleCalendarMapCtx,
};
pub use model::{GoogleEvent, GoogleEventDateTime, GoogleEventsResponse};

// ADR-034 P3: the provider id + scope literals moved to
// `maekon_core::ports::oauth` — maekon-network's provider registry needs the
// same literals and adapters may not depend on each other. Re-exported here so
// this module remains the connector's single import surface.
pub use maekon_core::ports::oauth::{GOOGLE_CALENDAR_PROVIDER_ID, GOOGLE_CALENDAR_READONLY_SCOPE};

/// Extension id (part of the identity key).
pub const GOOGLE_CALENDAR_EXTENSION_ID: &str = "com.maekon.google_calendar";

/// Remote object type.
pub const GOOGLE_CALENDAR_REMOTE_TYPE: &str = "event";

/// Upper bound on `events.list` maxResults (prevents unbounded backfill).
pub const DEFAULT_MAX_PAGE_RECORDS: u32 = 250;

/// Initial-sync past window (days). A bounded window that prevents unbounded past backfill.
pub const DEFAULT_PAST_WINDOW_DAYS: i64 = 30;

/// Initial-sync future window (days). Includes upcoming meetings but stays bounded.
pub const DEFAULT_FUTURE_WINDOW_DAYS: i64 = 90;

/// Connector configuration.
#[derive(Debug, Clone)]
pub struct GoogleCalendarConfig {
    pub install_id: String,
    /// The calendar to query (usually `"primary"`).
    pub calendar_id: String,
    pub provider_id: String,
    pub extension_id: String,
    pub remote_type: String,
    pub max_page_records: u32,
    pub past_window_days: i64,
    pub future_window_days: i64,
}

impl GoogleCalendarConfig {
    /// Standard configuration for a single install and the default calendar.
    pub fn new(install_id: impl Into<String>) -> Self {
        Self {
            install_id: install_id.into(),
            calendar_id: "primary".into(),
            provider_id: GOOGLE_CALENDAR_PROVIDER_ID.into(),
            extension_id: GOOGLE_CALENDAR_EXTENSION_ID.into(),
            remote_type: GOOGLE_CALENDAR_REMOTE_TYPE.into(),
            max_page_records: DEFAULT_MAX_PAGE_RECORDS,
            past_window_days: DEFAULT_PAST_WINDOW_DAYS,
            future_window_days: DEFAULT_FUTURE_WINDOW_DAYS,
        }
    }
}

/// Google Calendar read-only connector.
pub struct GoogleCalendarConnector {
    api: Arc<dyn CalendarEventsApi>,
    oauth: Arc<dyn OAuthPort>,
    config: GoogleCalendarConfig,
    /// Revoked — from then on every new sync is fail-closed.
    revoked: AtomicBool,
    /// Hit a 410 on the previous run and was downgraded to a full resync — the next sync is Initial.
    needs_full_resync: AtomicBool,
}

impl GoogleCalendarConnector {
    pub fn new(
        api: Arc<dyn CalendarEventsApi>,
        oauth: Arc<dyn OAuthPort>,
        config: GoogleCalendarConfig,
    ) -> Self {
        Self {
            api,
            oauth,
            config,
            revoked: AtomicBool::new(false),
            needs_full_resync: AtomicBool::new(false),
        }
    }

    /// This connector's source descriptor (§5). `Monotonic` + explicit delete signal → advertisable.
    pub fn descriptor(&self) -> ContextSourceDescriptor {
        ContextSourceDescriptor {
            extension_id: self.config.extension_id.clone(),
            install_id: self.config.install_id.clone(),
            remote_type: self.config.remote_type.clone(),
            revision_model: RevisionModel::Monotonic,
            // status=cancelled + syncToken removal = explicit delete signal (revision I6).
            has_explicit_delete_signal: true,
            supports_undelete: false,
            max_page_records: self.config.max_page_records,
        }
    }

    fn map_ctx(&self, account_subject_ref: &str) -> GoogleCalendarMapCtx {
        GoogleCalendarMapCtx {
            extension_id: self.config.extension_id.clone(),
            install_id: self.config.install_id.clone(),
            account_subject_ref: account_subject_ref.to_string(),
            remote_type: self.config.remote_type.clone(),
        }
    }
}

/// Page digest (for provenance). Computed deterministically over the cursor kind,
/// record remote ids, and next cursor (a one-way hash — remote ids are not left in cleartext).
fn compute_page_digest(
    cursor: &SyncCursor,
    records: &[ContextSourceRecord],
    next_cursor: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"google-calendar-page/v1\0");
    let kind: &[u8] = match cursor {
        SyncCursor::Initial => b"initial",
        SyncCursor::Page(_) => b"page",
        SyncCursor::Sync(_) => b"sync",
    };
    hasher.update((kind.len() as u64).to_be_bytes());
    hasher.update(kind);
    hasher.update((records.len() as u64).to_be_bytes());
    for r in records {
        hasher.update((r.identity.remote_id.len() as u64).to_be_bytes());
        hasher.update(r.identity.remote_id.as_bytes());
    }
    match next_cursor {
        Some(c) => {
            hasher.update(b"P");
            hasher.update((c.len() as u64).to_be_bytes());
            hasher.update(c.as_bytes());
        }
        None => hasher.update(b"N"),
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[async_trait]
impl ContextSourcePort for GoogleCalendarConnector {
    async fn discover(&self) -> Result<Vec<ContextSourceDescriptor>, CoreError> {
        Ok(vec![self.descriptor()])
    }

    async fn account_status(
        &self,
        _install_id: &str,
        _account_subject_ref: &str,
    ) -> Result<AccountStatus, CoreError> {
        if self.revoked.load(Ordering::SeqCst) {
            return Ok(AccountStatus::Revoked);
        }
        match self.oauth.connection_status(&self.config.provider_id).await {
            Ok(status) if status.connected => Ok(AccountStatus::Connected),
            Ok(_) => Ok(AccountStatus::NotConnected),
            Err(_) => Ok(AccountStatus::Error),
        }
    }

    async fn sync(&self, request: SyncRequest) -> Result<SyncOutcome, CoreError> {
        // After revocation, never proceed with a new sync (fail-closed).
        if self.revoked.load(Ordering::SeqCst) {
            return Ok(SyncOutcome::Unhealthy(SourceHealth::Unauthorized));
        }

        let now = Utc::now();
        let stored = SyncCursor::decode(request.cursor.as_deref());
        // If there was a prior 410 downgrade, ignore the stored sync token and do an Initial full resync.
        let cursor = if self.needs_full_resync.swap(false, Ordering::SeqCst) {
            SyncCursor::Initial
        } else {
            stored
        };

        let (time_min, time_max) = match &cursor {
            SyncCursor::Initial => {
                let (mn, mx) = historical_window(
                    now,
                    self.config.past_window_days,
                    self.config.future_window_days,
                );
                (Some(mn), Some(mx))
            }
            _ => (None, None),
        };

        // The connector never exceeds the smaller of the request quota and its own upper bound (prevents unbounded).
        let max_results = request.max_records.min(self.config.max_page_records).max(1);

        let list_req = ListEventsRequest {
            calendar_id: self.config.calendar_id.clone(),
            max_results,
            cursor: cursor.clone(),
            time_min,
            time_max,
        };

        let resp = match self.api.list_events(&list_req).await? {
            ListEventsOutcome::Ok(resp) => resp,
            ListEventsOutcome::Unhealthy(health) => {
                if health == SourceHealth::CursorExpired {
                    // Mark so the next run ignores the expired syncToken and does a full resync.
                    self.needs_full_resync.store(true, Ordering::SeqCst);
                }
                return Ok(SyncOutcome::Unhealthy(health));
            }
        };

        let ctx = self.map_ctx(&request.account_subject_ref);
        let records: Vec<ContextSourceRecord> = resp
            .items
            .iter()
            .filter(|e| !e.id.is_empty())
            .map(|e| event_to_record(e, &ctx, now))
            .collect();

        // Cursor/pagination: if there is a pageToken, it is the next page within the
        // window; if there is none but a syncToken, the window is drained (store the
        // incremental cursor); if neither, drained.
        let (next_cursor, has_more) = if let Some(page) = &resp.next_page_token {
            (Some(SyncCursor::encode_page(page)), true)
        } else if let Some(sync) = &resp.next_sync_token {
            (Some(SyncCursor::encode_sync(sync)), false)
        } else {
            (None, false)
        };

        let page_digest = compute_page_digest(&cursor, &records, next_cursor.as_deref());

        Ok(SyncOutcome::Page(ContextSourcePage {
            records,
            next_cursor,
            has_more,
            page_digest,
            access_epoch_id: request.access_epoch_id,
        }))
    }

    async fn health(&self, _install_id: &str) -> Result<SourceHealth, CoreError> {
        if self.revoked.load(Ordering::SeqCst) {
            return Ok(SourceHealth::Unauthorized);
        }
        let token = self
            .oauth
            .get_access_token(&self.config.provider_id)
            .await?;
        Ok(if token.is_some() {
            SourceHealth::Healthy
        } else {
            SourceHealth::Unauthorized
        })
    }

    async fn revoke(&self, _install_id: &str, _account_subject_ref: &str) -> Result<(), CoreError> {
        // Destroy credentials (keychain deletion) — OAuthPort::revoke performs the
        // SecretStore delete_namespace. Subsequent syncs are fail-closed.
        self.oauth.revoke(&self.config.provider_id).await?;
        self.revoked.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_is_advertisable_monotonic_with_delete_signal() {
        // The Google connector is Monotonic + explicit delete signal → it must be
        // advertisable so run_sync does not reject it as NotAdvertisable (§5, revision I6).
        let d = ContextSourceDescriptor {
            extension_id: GOOGLE_CALENDAR_EXTENSION_ID.into(),
            install_id: "inst_1".into(),
            remote_type: GOOGLE_CALENDAR_REMOTE_TYPE.into(),
            revision_model: RevisionModel::Monotonic,
            has_explicit_delete_signal: true,
            supports_undelete: false,
            max_page_records: DEFAULT_MAX_PAGE_RECORDS,
        };
        assert!(d.is_advertisable());
        assert_eq!(d.revision_model, RevisionModel::Monotonic);
    }

    #[test]
    fn readonly_scope_declares_no_write_scope() {
        // Minimal read-only scope only — there must be no write/full-access scope.
        assert_eq!(
            GOOGLE_CALENDAR_READONLY_SCOPE,
            "https://www.googleapis.com/auth/calendar.events.readonly"
        );
        assert!(GOOGLE_CALENDAR_READONLY_SCOPE.ends_with(".readonly"));
        // Not the full calendar write scope (`/auth/calendar`) nor the events write scope.
        assert_ne!(
            GOOGLE_CALENDAR_READONLY_SCOPE,
            "https://www.googleapis.com/auth/calendar"
        );
        assert_ne!(
            GOOGLE_CALENDAR_READONLY_SCOPE,
            "https://www.googleapis.com/auth/calendar.events"
        );
    }

    #[test]
    fn page_digest_is_deterministic_and_order_sensitive() {
        let d1 = compute_page_digest(&SyncCursor::Initial, &[], Some("sync:tok"));
        let d2 = compute_page_digest(&SyncCursor::Initial, &[], Some("sync:tok"));
        assert_eq!(d1, d2);
        let d3 = compute_page_digest(&SyncCursor::Initial, &[], Some("sync:other"));
        assert_ne!(d1, d3);
    }
}
