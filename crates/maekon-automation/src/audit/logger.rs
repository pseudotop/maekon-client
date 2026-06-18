use chrono::Utc;
use maekon_core::config::PiiFilterLevel;
use maekon_core::models::audit::{AuditEntry, AuditLevel, AuditStats, AuditStatus};
use maekon_core::ports::pii_sanitizer::PiiSanitizer;
use std::collections::VecDeque;
use std::sync::Arc;

use super::traits::{AuditPersistence, AuditQuery};

const REDACTED_APP: &str = "[REDACTED_APP]";
const REDACTED_SECRET: &str = "[REDACTED_SECRET]";
const REDACTED_STDERR: &str = "[REDACTED_STDERR]";
const REDACTED_STDOUT: &str = "[REDACTED_STDOUT]";
const REDACTED_WINDOW_TITLE: &str = "[REDACTED_WINDOW_TITLE]";

pub struct AuditLogger {
    pub(super) buffer: VecDeque<AuditEntry>,
    pub(super) max_buffer_size: usize,
    pub(super) batch_size: usize,
    pub(super) persistence: Option<Arc<dyn AuditPersistence>>,
    /// Storage-backed historical query handle. When set, `entries_by_command_id`
    /// falls through to this after exhausting the in-memory buffer.
    pub(super) query: Option<Arc<dyn AuditQuery>>,
    /// D5 iter-6: Audit log details may include command stdout/stderr which
    /// can contain API keys, tokens, or other sensitive output. Apply the
    /// strictest PII filtering unconditionally at the record boundary (not
    /// user-configurable — audit log is a security control, not a feature).
    pub(super) pii_sanitizer: Option<Arc<dyn PiiSanitizer>>,
}

impl AuditLogger {
    pub fn new(max_buffer_size: usize, batch_size: usize) -> Self {
        Self {
            buffer: VecDeque::with_capacity(max_buffer_size),
            max_buffer_size,
            batch_size,
            persistence: None,
            query: None,
            pii_sanitizer: None,
        }
    }

    /// Attach a persistence callback for durable storage of audit entries.
    ///
    /// When set, every new audit entry is forwarded to this callback
    /// immediately after being added to the in-memory buffer.
    pub fn with_persistence(mut self, cb: Arc<dyn AuditPersistence>) -> Self {
        self.persistence = Some(cb);
        self
    }

    /// Attach a query handle for historical (storage-backed) audit lookup.
    ///
    /// When set, [`Self::entries_by_command_id`] falls through to this query
    /// handle after consulting the in-memory buffer, merging results and
    /// deduplicating by `entry_id`. Use the matching binary-crate wrapper
    /// (e.g., `SqliteAuditQuery` in `src-tauri`) to bridge to storage —
    /// `maekon-automation` itself MUST NOT depend on `maekon-storage`.
    pub fn with_query(mut self, q: Arc<dyn AuditQuery>) -> Self {
        self.query = Some(q);
        self
    }

    /// D5 iter-6: attach a PII sanitizer. Audit log applies
    /// `PiiFilterLevel::Strict` unconditionally (not user-configurable per
    /// O3 in the D5 design spec) — audit trails are a security control.
    pub fn with_pii_sanitizer(mut self, sanitizer: Arc<dyn PiiSanitizer>) -> Self {
        self.pii_sanitizer = Some(sanitizer);
        self
    }

    /// D5 iter-6: sanitize a details string for audit storage.
    fn sanitize_details(&self, details: Option<String>) -> Option<String> {
        details.map(|raw| {
            if let Some(sanitizer) = self.pii_sanitizer.as_ref() {
                sanitizer.sanitize_text(&raw, PiiFilterLevel::Strict)
            } else {
                fail_closed_sanitize_details(&raw)
            }
        })
    }

    pub fn log_start(&mut self, command_id: &str, session_id: &str, action_type: &str) {
        self.push_entry(
            command_id,
            session_id,
            action_type,
            AuditStatus::Started,
            None,
        );
    }

    pub fn log_complete(&mut self, command_id: &str, session_id: &str, details: &str) {
        self.push_entry(
            command_id,
            session_id,
            "complete",
            AuditStatus::Completed,
            Some(details.to_string()),
        );
    }

    pub fn log_denied(&mut self, command_id: &str, session_id: &str, action_type: &str) {
        self.push_entry(
            command_id,
            session_id,
            action_type,
            AuditStatus::Denied,
            None,
        );
    }

    pub fn log_failed(&mut self, command_id: &str, session_id: &str, error: &str) {
        self.push_entry(
            command_id,
            session_id,
            "failed",
            AuditStatus::Failed,
            Some(error.to_string()),
        );
    }

    pub fn log_event(&mut self, action_type: &str, session_id: &str, details: &str) {
        self.push_entry(
            &maekon_core::generate_id("evt"),
            session_id,
            action_type,
            AuditStatus::Completed,
            Some(details.to_string()),
        );
    }

    pub fn log_start_if(
        &mut self,
        level: AuditLevel,
        command_id: &str,
        session_id: &str,
        action_type: &str,
    ) {
        if matches!(level, AuditLevel::None) {
            return;
        }
        self.push_entry(
            command_id,
            session_id,
            action_type,
            AuditStatus::Started,
            None,
        );
    }

    pub fn log_complete_with_time(
        &mut self,
        level: AuditLevel,
        command_id: &str,
        session_id: &str,
        details: &str,
        execution_time_ms: u64,
    ) {
        if matches!(level, AuditLevel::None) {
            return;
        }
        self.push_entry_with_time(
            command_id,
            session_id,
            "complete",
            AuditStatus::Completed,
            Some(details.to_string()),
            Some(execution_time_ms),
        );
    }

    pub fn log_timeout(&mut self, command_id: &str, session_id: &str, timeout_ms: u64) {
        self.push_entry_with_time(
            command_id,
            session_id,
            "timeout",
            AuditStatus::Timeout,
            Some(format!("Exceeded {}ms", timeout_ms)),
            Some(timeout_ms),
        );
    }

    pub fn has_pending_batch(&self) -> bool {
        self.buffer.len() >= self.batch_size
    }

    pub fn pending_count(&self) -> usize {
        self.buffer.len()
    }

    pub fn drain_batch(&mut self) -> Vec<AuditEntry> {
        let count = self.buffer.len().min(self.batch_size);
        self.buffer.drain(..count).collect()
    }

    pub fn drain_all(&mut self) -> Vec<AuditEntry> {
        self.buffer.drain(..).collect()
    }

    pub fn recent_entries(&self, limit: usize) -> Vec<AuditEntry> {
        self.buffer.iter().rev().take(limit).cloned().collect()
    }

    pub fn entries_by_status(&self, status: &AuditStatus, limit: usize) -> Vec<AuditEntry> {
        self.buffer
            .iter()
            .rev()
            .filter(|e| &e.status == status)
            .take(limit)
            .cloned()
            .collect()
    }

    /// Filter entries by action_type prefix at the data level (no over-reading).
    pub fn entries_by_action_prefix(&self, prefix: &str, limit: usize) -> Vec<AuditEntry> {
        self.buffer
            .iter()
            .rev()
            .filter(|e| e.action_type.starts_with(prefix))
            .take(limit)
            .cloned()
            .collect()
    }

    /// Lookup audit entries by `command_id` with storage fall-through.
    ///
    /// Walks the in-memory `VecDeque` buffer first (newest-first via
    /// `iter().rev()`). If the buffer doesn't satisfy `limit` and an
    /// [`AuditQuery`] handle was attached via [`Self::with_query`], queries
    /// the historical storage for the remainder, deduplicating by `entry_id`
    /// (entries persisted to storage may still be present in the buffer —
    /// both write paths fire on the same insertion). Final results are
    /// re-sorted by `timestamp DESC` and truncated to `limit`.
    pub fn entries_by_command_id(&self, command_id: &str, limit: usize) -> Vec<AuditEntry> {
        if limit == 0 {
            return Vec::new();
        }

        // Buffer first — newest entries (capacity ~1000 by default).
        let mut results: Vec<AuditEntry> = self
            .buffer
            .iter()
            .rev()
            .filter(|e| e.command_id == command_id)
            .take(limit)
            .cloned()
            .collect();

        // Fall-through: if buffer didn't satisfy `limit`, query storage for
        // the remainder, deduping by entry_id (entries persisted to storage
        // may still be in buffer — both write paths fire on the same insertion).
        if results.len() < limit {
            if let Some(q) = &self.query {
                let buffer_ids: std::collections::HashSet<String> =
                    results.iter().map(|e| e.entry_id.clone()).collect();
                let storage_results = q.entries_by_command_id(command_id, limit);
                for entry in storage_results {
                    if results.len() >= limit {
                        break;
                    }
                    if !buffer_ids.contains(&entry.entry_id) {
                        results.push(entry);
                    }
                }
            }
        }

        // Re-sort by timestamp DESC. Buffer rows are inserted-newest-first
        // (VecDeque + .rev()), and storage rows arrive in timestamp DESC. After
        // merge they may interleave, so re-sort to maintain newest-first.
        results.sort_by_key(|e| std::cmp::Reverse(e.timestamp));
        results.truncate(limit);
        results
    }

    pub fn stats(&self) -> AuditStats {
        let mut completed = 0;
        let mut failed = 0;
        let mut denied = 0;
        let mut timeout = 0;
        for entry in &self.buffer {
            match entry.status {
                AuditStatus::Completed => completed += 1,
                AuditStatus::Failed => failed += 1,
                AuditStatus::Denied => denied += 1,
                AuditStatus::Timeout => timeout += 1,
                AuditStatus::Started => {}
            }
        }
        let total = completed + failed + denied + timeout;
        AuditStats {
            total,
            completed,
            failed,
            denied,
            timeout,
        }
    }

    fn push_entry(
        &mut self,
        command_id: &str,
        session_id: &str,
        action_type: &str,
        status: AuditStatus,
        details: Option<String>,
    ) {
        if self.buffer.len() >= self.max_buffer_size {
            self.buffer.pop_front();
            tracing::warn!("audit buffer full: dropping oldest entry");
        }

        let entry = AuditEntry {
            entry_id: maekon_core::generate_id("aud"),
            timestamp: Utc::now(),
            session_id: session_id.to_string(),
            command_id: command_id.to_string(),
            action_type: action_type.to_string(),
            status,
            details: self.sanitize_details(details),
            execution_time_ms: None,
        };

        if let Some(ref cb) = self.persistence {
            cb.persist(&entry);
        }

        self.buffer.push_back(entry);
    }

    fn push_entry_with_time(
        &mut self,
        command_id: &str,
        session_id: &str,
        action_type: &str,
        status: AuditStatus,
        raw_details: Option<String>,
        execution_time_ms: Option<u64>,
    ) {
        // D5 iter-6: sanitize details at record boundary.
        let details = self.sanitize_details(raw_details);
        if self.buffer.len() >= self.max_buffer_size {
            self.buffer.pop_front();
            tracing::warn!("audit buffer full: dropping oldest entry");
        }

        let entry = AuditEntry {
            entry_id: maekon_core::generate_id("aud"),
            timestamp: Utc::now(),
            session_id: session_id.to_string(),
            command_id: command_id.to_string(),
            action_type: action_type.to_string(),
            status,
            details,
            execution_time_ms,
        };

        if let Some(ref cb) = self.persistence {
            cb.persist(&entry);
        }

        self.buffer.push_back(entry);
    }

    /// #6277: record a completion with the caller's REAL status + action_type
    /// (not the hardcoded `Completed`/`"complete"` of `log_complete_with_time`),
    /// so the durable audit row reflects the true outcome.
    #[allow(clippy::too_many_arguments)]
    pub fn log_with_status_and_time(
        &mut self,
        level: AuditLevel,
        command_id: &str,
        session_id: &str,
        action_type: &str,
        status: AuditStatus,
        details: &str,
        execution_time_ms: u64,
    ) {
        if matches!(level, AuditLevel::None) {
            return;
        }
        self.push_entry_with_time(
            command_id,
            session_id,
            action_type,
            status,
            Some(details.to_string()),
            Some(execution_time_ms),
        );
    }
}

impl Default for AuditLogger {
    fn default() -> Self {
        Self::new(1000, 50)
    }
}

pub(super) fn fail_closed_sanitize_details(raw: &str) -> String {
    let mut sanitized = raw.to_string();
    for (key, marker, consume_until_next_key) in [
        ("stdout", REDACTED_STDOUT, true),
        ("stderr", REDACTED_STDERR, true),
        ("active_window", REDACTED_WINDOW_TITLE, true),
        ("window_title", REDACTED_WINDOW_TITLE, true),
        ("title", REDACTED_WINDOW_TITLE, true),
        ("app", REDACTED_APP, false),
        ("api_key", REDACTED_SECRET, false),
        ("apikey", REDACTED_SECRET, false),
        ("access_token", REDACTED_SECRET, false),
        ("refresh_token", REDACTED_SECRET, false),
        ("policy_token", REDACTED_SECRET, false),
        ("capability_token", REDACTED_SECRET, false),
        ("integration_auth_token", REDACTED_SECRET, false),
        ("authorization", REDACTED_SECRET, true),
        ("password", REDACTED_SECRET, false),
        ("secret", REDACTED_SECRET, false),
        ("token", REDACTED_SECRET, false),
    ] {
        sanitized = redact_detail_field(&sanitized, key, marker, consume_until_next_key);
    }
    redact_bearer_tokens(&sanitized)
}

fn redact_detail_field(
    input: &str,
    key: &str,
    marker: &str,
    consume_until_next_key: bool,
) -> String {
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0;

    while let Some(relative) = find_case_insensitive(&input[cursor..], key) {
        let key_start = cursor + relative;
        let Some(separator_idx) = field_separator_index(input, key_start, key) else {
            let next = key_start + key.len();
            out.push_str(&input[cursor..next]);
            cursor = next;
            continue;
        };
        let Some((value_start, value_end)) =
            field_value_bounds(input, separator_idx, consume_until_next_key)
        else {
            let next = separator_idx + 1;
            out.push_str(&input[cursor..next]);
            cursor = next;
            continue;
        };

        out.push_str(&input[cursor..value_start]);
        out.push_str(marker);
        cursor = value_end;
    }

    out.push_str(&input[cursor..]);
    out
}

fn find_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    let haystack_lower = haystack.to_ascii_lowercase();
    let needle_lower = needle.to_ascii_lowercase();
    haystack_lower.find(&needle_lower)
}

fn field_separator_index(input: &str, key_start: usize, key: &str) -> Option<usize> {
    let before_key = input[..key_start].chars().next_back();
    if before_key.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return None;
    }

    let mut idx = key_start + key.len();
    if let Some((ch, next)) = char_at(input, idx) {
        if ch == '"' || ch == '\'' {
            idx = next;
        }
    }

    idx = skip_ascii_whitespace(input, idx);
    match char_at(input, idx) {
        Some(('=' | ':', _)) => Some(idx),
        _ => None,
    }
}

fn field_value_bounds(
    input: &str,
    separator_idx: usize,
    consume_until_next_key: bool,
) -> Option<(usize, usize)> {
    let (_, mut idx) = char_at(input, separator_idx)?;
    idx = skip_ascii_whitespace(input, idx);
    let (first, first_next) = char_at(input, idx)?;

    if first == '"' || first == '\'' {
        let quote = first;
        let value_start = first_next;
        let value_end = input[value_start..]
            .char_indices()
            .find_map(|(offset, ch)| (ch == quote).then_some(value_start + offset))
            .unwrap_or(input.len());
        return Some((value_start, value_end));
    }

    let value_start = idx;
    let value_end = if consume_until_next_key {
        find_next_field_boundary(input, value_start).unwrap_or(input.len())
    } else {
        input[value_start..]
            .char_indices()
            .find_map(|(offset, ch)| {
                (ch.is_ascii_whitespace()
                    || matches!(ch, ',' | ';' | '}' | ']' | ')' | '\n' | '\r'))
                .then_some(value_start + offset)
            })
            .unwrap_or(input.len())
    };

    Some((value_start, value_end))
}

fn find_next_field_boundary(input: &str, value_start: usize) -> Option<usize> {
    for (offset, ch) in input[value_start..].char_indices().skip(1) {
        if !matches!(ch, ' ' | ',' | ';' | '\n' | '\r') {
            continue;
        }
        let boundary = value_start + offset;
        let mut idx = skip_ascii_whitespace(input, boundary + ch.len_utf8());
        if let Some((quote, next)) = char_at(input, idx) {
            if quote == '"' || quote == '\'' {
                idx = next;
            }
        }

        let key_start = idx;
        while let Some((candidate, next)) = char_at(input, idx) {
            if candidate.is_ascii_alphanumeric() || candidate == '_' || candidate == '-' {
                idx = next;
            } else {
                break;
            }
        }
        if idx == key_start {
            continue;
        }
        if let Some((quote, next)) = char_at(input, idx) {
            if quote == '"' || quote == '\'' {
                idx = next;
            }
        }
        idx = skip_ascii_whitespace(input, idx);
        if matches!(char_at(input, idx), Some(('=' | ':', _))) {
            return Some(boundary);
        }
    }
    None
}

fn redact_bearer_tokens(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0;
    while let Some(relative) = find_case_insensitive(&input[cursor..], "bearer ") {
        let bearer_start = cursor + relative;
        let value_start = bearer_start + "bearer ".len();
        let value_end = input[value_start..]
            .char_indices()
            .find_map(|(offset, ch)| {
                (ch.is_ascii_whitespace() || matches!(ch, ',' | ';' | '}' | ']'))
                    .then_some(value_start + offset)
            })
            .unwrap_or(input.len());

        out.push_str(&input[cursor..value_start]);
        out.push_str(REDACTED_SECRET);
        cursor = value_end;
    }
    out.push_str(&input[cursor..]);
    out
}

fn skip_ascii_whitespace(input: &str, mut idx: usize) -> usize {
    while let Some((ch, next)) = char_at(input, idx) {
        if ch.is_ascii_whitespace() {
            idx = next;
        } else {
            break;
        }
    }
    idx
}

fn char_at(input: &str, idx: usize) -> Option<(char, usize)> {
    input[idx..]
        .chars()
        .next()
        .map(|ch| (ch, idx + ch.len_utf8()))
}
