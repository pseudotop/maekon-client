//! Shared table descriptor for the 6 authoritative cross-device sync tables.
//!
//! Single source of truth for each table's column list, primary-key shape,
//! and conflict-merge mode. `sync_merger::merge_row` (the write side) and
//! `sync_extractor::extract_changeset_blocking` (the read side) both drive
//! off the SAME [`TableDescriptor`] instead of each hand-spelling its own
//! copy of the column list — see issue #7742 (ctd-W3 F3): three past
//! sweep-fixes (#5202, #5174, #6174) each had to be hand-applied across
//! every one of the former 6 per-table merge functions AND the extractor's
//! matching `json_object(...)` literal, because nothing tied them together.
//!
//! This module intentionally encodes the axes the 6 merge functions
//! actually differed on as plain DATA on [`ColumnSpec`] / [`TableDescriptor`]
//! rather than a per-table closure:
//! * [`MergeMode`] — append-only union-insert vs. last-write-wins (LWW) by
//!   HLC, vs. LWW with a monotonic status tiebreak layered in front of the
//!   HLC compare (`suggestions`: acted > dismissed > shown > null).
//! * [`ColumnKind::TextDefault`] / [`ColumnKind::IntegerDefault`] — a wire
//!   default applied when a pre-fix peer's changeset omits the field
//!   (#5202: `activity_segments.trigger_reason` is `TEXT NOT NULL` with no
//!   SQL-side default; a pre-fix peer never emitted it, so a naive INSERT
//!   would hit the NOT NULL constraint and silently roll back the whole
//!   merge transaction).
//! * [`ColumnSpec::immutable_on_update`] — excluded from the generic
//!   routine's UPDATE SET list even though it IS written on INSERT. Today
//!   only `suggestions.created_at` needs this: a peer must never overwrite
//!   the original creation timestamp of an existing suggestion on a
//!   winning LWW update.
//! * [`ColumnKind::HexBlob`] — the one genuinely irregular per-row
//!   transform (hex-decode `embedding_vectors.vector`, REJECTING — not
//!   zero-filling — the whole row on a malformed value, #35/#6081). It is
//!   modeled as a column *kind* (data), not a per-table closure, because
//!   exactly one column across all 6 tables needs it.
//! * [`ExtractorGate`] — the 3 `SyncConfig` data-minimization flags that
//!   conditionally omit a column from the extractor's wire payload
//!   (`llm_summary`, `content_activities_json`, `embedding_vectors.
//!   original_text`). A closed enum over the 3 real flags, not a closure.

use maekon_core::config::SyncConfig;
use rusqlite::ToSql;

use crate::error::StorageError;

// ─────────────────────────── descriptor shape ───────────────────────────

/// One of the 6 authoritative cross-device sync tables.
pub(crate) struct TableDescriptor {
    /// The SQL table name — also the `sync_tombstones.table_name` value and
    /// the GDPR `DeletionEvent` per-table DELETE target.
    pub(crate) data_table: &'static str,
    pub(crate) pk: PrimaryKey,
    pub(crate) mode: MergeMode,
    /// Every value column written by INSERT/UPDATE, in the table's
    /// canonical wire/SQL order. Includes primary-key columns that are
    /// ALSO ordinary value columns (e.g. `embedding_vectors.segment_id`).
    pub(crate) columns: &'static [ColumnSpec],
}

/// How a row is identified for the dedup / LWW-conflict lookup.
pub(crate) enum PrimaryKey {
    /// A single TEXT column that is both the SQL PK and the row's stable
    /// cross-device identity (`activity_segments.id`,
    /// `regime_overrides.override_id`, `regimes.id`,
    /// `suggestions.suggestion_id`, `trigger_params_snapshots.id`).
    Simple(&'static str),
    /// A composite natural key (two TEXT columns) backed by an internal
    /// autoincrement surrogate used ONLY for the UPDATE `WHERE` clause.
    /// `embedding_vectors`: its visible `id` is a per-device autoincrement
    /// and is NOT cross-device-stable, so tombstones/lookups key on
    /// `(segment_id, model_id)` instead — see `EMB_KEY_SEP` in
    /// `sync_merger`.
    CompositeWithSurrogate {
        parts: (&'static str, &'static str),
        surrogate: &'static str,
    },
}

/// How conflicting rows for the same primary key are resolved.
pub(crate) enum MergeMode {
    /// Union merge: INSERT only if the PK does not already exist; a
    /// re-send of an existing PK is a duplicate, not an update
    /// (`activity_segments`, `regime_overrides`, `trigger_params_snapshots`
    /// — none of these ever update an existing row).
    AppendOnly,
    /// Last-Write-Wins by HLC: an existing row is replaced wholesale when
    /// the incoming HLC is causally after the local one; otherwise the
    /// incoming row is discarded (`regimes`, `embedding_vectors`).
    LastWriteWins,
    /// LWW with a monotonic status tiebreak evaluated BEFORE the HLC
    /// compare: a higher status ordinal always wins regardless of HLC, and
    /// only an ordinal TIE falls back to the HLC compare. This is the
    /// #5174 "IMPORTANT-2" fix path (an `acted` row can lose to a
    /// re-synced lower-HLC row, but never resurrects a tombstoned one —
    /// that guard is table-shape-independent and lives in
    /// `sync_merger::merge_row`, not here). Only `suggestions` uses this.
    LastWriteWinsWithTiebreak(StatusTiebreak),
}

/// A monotonic status ordinal computed from column PRESENCE (non-null),
/// highest-priority first. `suggestions`: `["acted_at", "dismissed_at",
/// "shown_at"]` encodes acted(3) > dismissed(2) > shown(1) > null(0) —
/// the exact ordinal `SqliteSyncMerger::suggestion_status_ordinal` used to
/// hand-compute.
pub(crate) struct StatusTiebreak {
    pub(crate) presence_rank: &'static [&'static str],
}

impl StatusTiebreak {
    /// The ordinal for `row` under this tiebreak: `presence_rank.len() - i`
    /// for the first present column at index `i`, else `0`.
    pub(crate) fn ordinal(&self, row: &serde_json::Value) -> u8 {
        for (i, col) in self.presence_rank.iter().enumerate() {
            if row.get(*col).and_then(|v| v.as_str()).is_some() {
                return (self.presence_rank.len() - i) as u8;
            }
        }
        0
    }
}

/// A gate over `SyncConfig`'s 3 data-minimization flags controlling whether
/// the extractor emits a column at all (omitted, not null, when closed —
/// privacy parity for a data-minimized sync). Merge-side reading is
/// unaffected: an omitted column is simply absent from the wire row, and
/// `ColumnKind`'s `TextOptional`/`*Default` handles that uniformly.
#[derive(Clone, Copy)]
pub(crate) enum ExtractorGate {
    Always,
    IncludeLlmSummary,
    IncludeContentActivities,
    IncludeEmbeddingText,
}

impl ExtractorGate {
    pub(crate) fn is_open(self, sync_config: &SyncConfig) -> bool {
        match self {
            ExtractorGate::Always => true,
            ExtractorGate::IncludeLlmSummary => sync_config.include_llm_summary,
            ExtractorGate::IncludeContentActivities => sync_config.include_content_activities,
            ExtractorGate::IncludeEmbeddingText => sync_config.include_embedding_text,
        }
    }
}

/// How a single column's wire JSON value binds to SQL.
#[derive(Clone, Copy)]
pub(crate) enum ColumnKind {
    /// Required TEXT (`json_str`) — a missing field is a hard error.
    Text,
    /// Required TEXT with a wire default applied when the field is absent
    /// (a past peer-compat sweep fix — see the module doc's #5202 note).
    TextDefault(&'static str),
    /// Nullable TEXT (`json_str_opt`) — absent or JSON `null` binds SQL
    /// `NULL`.
    TextOptional,
    /// Required INTEGER (`json_i64`) — a missing field is a hard error.
    Integer,
    /// Required INTEGER with a default applied when the field is absent.
    IntegerDefault(i64),
    /// Required REAL (`json_f64`) — a missing field is a hard error.
    Real,
    /// A hex-encoded BLOB. The extractor SELECTs `hex(col)`; the merger
    /// decodes it back to bytes, REJECTING (not zero-filling) the whole
    /// row on a malformed value — see [`decode_vector`] (#35/#6081). Only
    /// `embedding_vectors.vector` uses this today.
    HexBlob,
}

/// One column's wire shape + merge/extract behavior.
pub(crate) struct ColumnSpec {
    pub(crate) name: &'static str,
    pub(crate) kind: ColumnKind,
    /// `true` when the extractor emits this column but the merger never
    /// reads it back (a legacy wire wart, not a live column). Today only
    /// `embedding_vectors.id`: the extractor has always emitted the row's
    /// (per-device, non-cross-device-stable) autoincrement `id` for wire
    /// compat, but `merge_row` looks the local row up by
    /// `(segment_id, model_id)` and never consults the incoming `id`.
    pub(crate) extractor_only: bool,
    pub(crate) extractor_gate: ExtractorGate,
    /// `true` for a column that must survive a winning LWW UPDATE
    /// unchanged — excluded from the generic routine's UPDATE SET list
    /// even though it IS part of the INSERT column list. See the module
    /// doc's note on `suggestions.created_at`.
    pub(crate) immutable_on_update: bool,
}

const fn col(name: &'static str, kind: ColumnKind) -> ColumnSpec {
    ColumnSpec {
        name,
        kind,
        extractor_only: false,
        extractor_gate: ExtractorGate::Always,
        immutable_on_update: false,
    }
}

const fn col_gated(name: &'static str, kind: ColumnKind, gate: ExtractorGate) -> ColumnSpec {
    ColumnSpec {
        name,
        kind,
        extractor_only: false,
        extractor_gate: gate,
        immutable_on_update: false,
    }
}

const fn col_extractor_only(name: &'static str, kind: ColumnKind) -> ColumnSpec {
    ColumnSpec {
        name,
        kind,
        extractor_only: true,
        extractor_gate: ExtractorGate::Always,
        immutable_on_update: false,
    }
}

const fn col_immutable(name: &'static str, kind: ColumnKind) -> ColumnSpec {
    ColumnSpec {
        name,
        kind,
        extractor_only: false,
        extractor_gate: ExtractorGate::Always,
        immutable_on_update: true,
    }
}

// ───────────────────────────── the registry ─────────────────────────────

const ACTIVITY_SEGMENTS_COLUMNS: &[ColumnSpec] = &[
    col("id", ColumnKind::Text),
    col("start_time", ColumnKind::Text),
    col("end_time", ColumnKind::Text),
    col("duration_secs", ColumnKind::Integer),
    // #5202: TEXT NOT NULL with no SQL default; a pre-fix peer's changeset
    // omits it, so the merge INSERT must supply "sync" itself or the
    // constraint rolls back the whole transaction.
    col("trigger_reason", ColumnKind::TextDefault("sync")),
    col("regime_id", ColumnKind::TextOptional),
    col("dominant_category", ColumnKind::Text),
    col("app_breakdown", ColumnKind::TextDefault("{}")),
    // Privacy data-minimization (#5174): an LLM narrative of screen
    // activity, opt-in only.
    col_gated(
        "llm_summary",
        ColumnKind::TextOptional,
        ExtractorGate::IncludeLlmSummary,
    ),
    // #11738: coarse provenance and safe failure reasons follow the same
    // privacy opt-in as the narrative they describe.
    col_gated(
        "llm_summary_status_json",
        ColumnKind::TextDefault("{}"),
        ExtractorGate::IncludeLlmSummary,
    ),
    col_gated(
        "content_activities_json",
        ColumnKind::TextDefault("[]"),
        ExtractorGate::IncludeContentActivities,
    ),
];

pub(crate) const ACTIVITY_SEGMENTS: TableDescriptor = TableDescriptor {
    data_table: "activity_segments",
    pk: PrimaryKey::Simple("id"),
    mode: MergeMode::AppendOnly,
    columns: ACTIVITY_SEGMENTS_COLUMNS,
};

const REGIMES_COLUMNS: &[ColumnSpec] = &[
    col("id", ColumnKind::Text),
    col("label", ColumnKind::Text),
    col("detected_at", ColumnKind::Text),
    col("last_seen_at", ColumnKind::Text),
    col("occurrence_count", ColumnKind::Integer),
    col("avg_density", ColumnKind::Real),
    col("avg_importance", ColumnKind::Real),
    col("dominant_category", ColumnKind::Text),
    col("params_snapshot_id", ColumnKind::TextOptional),
    col("is_active", ColumnKind::Integer),
    col("is_deleted", ColumnKind::IntegerDefault(0)),
    col("deleted_at", ColumnKind::TextOptional),
];

pub(crate) const REGIMES: TableDescriptor = TableDescriptor {
    data_table: "regimes",
    pk: PrimaryKey::Simple("id"),
    mode: MergeMode::LastWriteWins,
    columns: REGIMES_COLUMNS,
};

const REGIME_OVERRIDES_COLUMNS: &[ColumnSpec] = &[
    col("override_id", ColumnKind::Text),
    col("segment_id", ColumnKind::Text),
    col("original_regime_id", ColumnKind::TextOptional),
    col("action_type", ColumnKind::Text),
    col("action_data", ColumnKind::TextOptional),
    col("created_at", ColumnKind::Text),
];

pub(crate) const REGIME_OVERRIDES: TableDescriptor = TableDescriptor {
    data_table: "regime_overrides",
    pk: PrimaryKey::Simple("override_id"),
    mode: MergeMode::AppendOnly,
    columns: REGIME_OVERRIDES_COLUMNS,
};

const EMBEDDING_VECTORS_COLUMNS: &[ColumnSpec] = &[
    // Extractor-only wire wart: emitted for wire compat, never read back by
    // the merger (the row is looked up by `(segment_id, model_id)`).
    col_extractor_only("id", ColumnKind::Integer),
    col("segment_id", ColumnKind::Text),
    col("content_type", ColumnKind::Text),
    col("content_label", ColumnKind::TextOptional),
    // #5210: gated with `embedding_vectors.original_text` under
    // `include_embedding_text` — a SEGMENT_SUMMARY embedding's text is the
    // SAME llm_summary narrative, so this must not become a backdoor
    // around `include_llm_summary` (the SEGMENT_SUMMARY row filter lives
    // in `sync_extractor`, not here — it excludes the whole row, not a
    // column).
    col_gated(
        "original_text",
        ColumnKind::TextOptional,
        ExtractorGate::IncludeEmbeddingText,
    ),
    col("vector", ColumnKind::HexBlob),
    col("model_id", ColumnKind::Text),
    col("timestamp", ColumnKind::Text),
    col("is_stale", ColumnKind::IntegerDefault(0)),
    col("is_deleted", ColumnKind::IntegerDefault(0)),
    col("deleted_at", ColumnKind::TextOptional),
];

pub(crate) const EMBEDDING_VECTORS: TableDescriptor = TableDescriptor {
    data_table: "embedding_vectors",
    pk: PrimaryKey::CompositeWithSurrogate {
        parts: ("segment_id", "model_id"),
        surrogate: "id",
    },
    mode: MergeMode::LastWriteWins,
    columns: EMBEDDING_VECTORS_COLUMNS,
};

const SUGGESTIONS_COLUMNS: &[ColumnSpec] = &[
    col("suggestion_id", ColumnKind::Text),
    col("suggestion_type", ColumnKind::Text),
    col("source", ColumnKind::Text),
    col("content", ColumnKind::Text),
    col("priority", ColumnKind::Text),
    col("confidence_score", ColumnKind::Real),
    col("relevance_score", ColumnKind::Real),
    col("is_actionable", ColumnKind::Integer),
    col("reasoning", ColumnKind::TextOptional),
    col("context_app", ColumnKind::TextOptional),
    col("context_window", ColumnKind::TextOptional),
    col("context_target_id", ColumnKind::TextOptional),
    col("shown_at", ColumnKind::TextOptional),
    col("dismissed_at", ColumnKind::TextOptional),
    col("acted_at", ColumnKind::TextOptional),
    // Immutable on update: a peer must never overwrite an existing
    // suggestion's original creation timestamp.
    col_immutable("created_at", ColumnKind::Text),
    col("expires_at", ColumnKind::TextOptional),
    col("is_deleted", ColumnKind::IntegerDefault(0)),
    col("deleted_at", ColumnKind::TextOptional),
];

pub(crate) const SUGGESTIONS: TableDescriptor = TableDescriptor {
    data_table: "suggestions",
    pk: PrimaryKey::Simple("suggestion_id"),
    mode: MergeMode::LastWriteWinsWithTiebreak(StatusTiebreak {
        presence_rank: &["acted_at", "dismissed_at", "shown_at"],
    }),
    columns: SUGGESTIONS_COLUMNS,
};

const TRIGGER_PARAMS_SNAPSHOTS_COLUMNS: &[ColumnSpec] = &[
    col("id", ColumnKind::Text),
    col("created_at", ColumnKind::Text),
    col("preset", ColumnKind::Text),
    col("params_json", ColumnKind::Text),
];

pub(crate) const TRIGGER_PARAMS_SNAPSHOTS: TableDescriptor = TableDescriptor {
    data_table: "trigger_params_snapshots",
    pk: PrimaryKey::Simple("id"),
    mode: MergeMode::AppendOnly,
    columns: TRIGGER_PARAMS_SNAPSHOTS_COLUMNS,
};

/// The 6 authoritative synced tables, in the historical order every
/// hand-rolled `let tables = [...]` copy used (`sync_merger::
/// handle_deletion_event`, `sync_extractor::backfill_origin_device_id` +
/// `compute_max_hlc`, `sqlite::hlc_clock::SYNCED_TABLES`). Adding a 7th
/// synced table means adding a [`TableDescriptor`] above plus appending it
/// here (and the schema migration + GDPR erasure wiring — unchanged from
/// before this refactor).
pub(crate) const ALL_TABLE_NAMES: [&str; 6] = [
    ACTIVITY_SEGMENTS.data_table,
    REGIMES.data_table,
    REGIME_OVERRIDES.data_table,
    EMBEDDING_VECTORS.data_table,
    SUGGESTIONS.data_table,
    TRIGGER_PARAMS_SNAPSHOTS.data_table,
];

const ALL_TABLES: [&TableDescriptor; 6] = [
    &ACTIVITY_SEGMENTS,
    &REGIMES,
    &REGIME_OVERRIDES,
    &EMBEDDING_VECTORS,
    &SUGGESTIONS,
    &TRIGGER_PARAMS_SNAPSHOTS,
];

/// Maps a synced `table_name` to its primary-key column, for the tombstone
/// hard-DELETE (`sync_merger::apply_tombstone`). Returns `None` for an
/// unknown table name — a wire-supplied name is never formatted into SQL
/// unvalidated — AND for `embedding_vectors`, whose composite key
/// `apply_tombstone` handles separately (see `EMB_KEY_SEP` in
/// `sync_merger`).
pub(crate) fn tombstone_pk_col(table_name: &str) -> Option<&'static str> {
    ALL_TABLES.iter().find_map(|desc| {
        if desc.data_table != table_name {
            return None;
        }
        match &desc.pk {
            PrimaryKey::Simple(pk) => Some(*pk),
            PrimaryKey::CompositeWithSurrogate { .. } => None,
        }
    })
}

/// Look up the [`TableDescriptor`] for a synced `table_name`, or `None` for a
/// non-synced/unknown table. Lets the erase-time / retention-time / DeletionEvent
/// tombstone-capture paths derive the row_id SQL expression from this single
/// descriptor registry instead of re-hardcoding a 7th copy of the per-table
/// column facts (the #7742 SSOT rationale).
pub(crate) fn descriptor_for(table_name: &str) -> Option<&'static TableDescriptor> {
    ALL_TABLES
        .iter()
        .copied()
        .find(|d| d.data_table == table_name)
}

// ──────────────────────── row extraction + binding ────────────────────────

/// A column value ready to bind into a rusqlite statement — the small set
/// of scalar shapes every sync-merge column reduces to, so `TableDescriptor`
/// -driven INSERT/UPDATE building needs no per-table match arms.
pub(crate) enum BoundValue {
    Text(String),
    TextOpt(Option<String>),
    Int(i64),
    Real(f64),
    Blob(Vec<u8>),
}

impl ToSql for BoundValue {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        match self {
            BoundValue::Text(s) => s.to_sql(),
            BoundValue::TextOpt(o) => o.to_sql(),
            BoundValue::Int(i) => i.to_sql(),
            BoundValue::Real(f) => f.to_sql(),
            BoundValue::Blob(b) => b.to_sql(),
        }
    }
}

impl ColumnSpec {
    /// Extracts + binds this column's value from a wire row. `pk_values` is
    /// the row's already-extracted primary-key parts, in `PrimaryKey`
    /// order — only consulted by a `HexBlob` column to label a
    /// decode-failure warning (mirrors the pre-refactor
    /// `decode_vector(hex, segment_id, model_id)` call sites). Returns
    /// `Ok(None)` when a `HexBlob` value is corrupt: the caller must skip
    /// (not hard-fail) the whole row (#35/#6081).
    pub(crate) fn extract(
        &self,
        row: &serde_json::Value,
        pk_values: &[&str],
    ) -> Result<Option<BoundValue>, StorageError> {
        Ok(match self.kind {
            ColumnKind::Text => Some(BoundValue::Text(json_str(row, self.name)?.to_string())),
            ColumnKind::TextDefault(default) => Some(BoundValue::Text(
                json_str_or_default(row, self.name, default).to_string(),
            )),
            ColumnKind::TextOptional => Some(BoundValue::TextOpt(json_str_opt(row, self.name))),
            ColumnKind::Integer => Some(BoundValue::Int(json_i64(row, self.name)?)),
            ColumnKind::IntegerDefault(default) => Some(BoundValue::Int(json_i64_or_default(
                row, self.name, default,
            ))),
            ColumnKind::Real => Some(BoundValue::Real(json_f64(row, self.name)?)),
            ColumnKind::HexBlob => {
                let hex_str = json_str(row, self.name)?;
                let label_a = pk_values.first().copied().unwrap_or("");
                let label_b = pk_values.get(1).copied().unwrap_or("");
                decode_vector(hex_str, label_a, label_b).map(BoundValue::Blob)
            }
        })
    }
}

impl TableDescriptor {
    /// Extracts this row's primary-key parts, in `PrimaryKey` column order
    /// (1 element for `Simple`, 2 for `CompositeWithSurrogate`).
    pub(crate) fn pk_values<'a>(
        &self,
        row: &'a serde_json::Value,
    ) -> Result<Vec<&'a str>, StorageError> {
        match &self.pk {
            PrimaryKey::Simple(name) => Ok(vec![json_str(row, name)?]),
            PrimaryKey::CompositeWithSurrogate { parts: (a, b), .. } => {
                Ok(vec![json_str(row, a)?, json_str(row, b)?])
            }
        }
    }

    /// The label used in `extract_hlc`/`extract_hlc_counter` warning logs —
    /// `/`-joined for a composite key (mirrors the pre-refactor
    /// `format!("{segment_id}/{model_id}")`), otherwise the bare pk value.
    pub(crate) fn merge_row_label(&self, pk_values: &[&str]) -> String {
        pk_values.join("/")
    }

    /// The SQL expression yielding a row's cross-device-stable tombstone `row_id`
    /// for a tombstone-capture `SELECT` (erase, retention, or DeletionEvent fast
    /// path): the `Simple` pk column, or the `char(31)`-joined composite for a
    /// `CompositeWithSurrogate` table (whose visible `id` is a per-device
    /// autoincrement, not cross-device-stable). Matches the merge-side
    /// [`Self::tombstone_key`] (EMB_KEY_SEP = U+001F = `char(31)`) and
    /// `delete_all_data_inner`'s `SYNCED_TOMBSTONE_SOURCES` — the runtime string
    /// value of one is the SQL-side value of the other.
    pub(crate) fn tombstone_row_id_sql(&self) -> String {
        match &self.pk {
            PrimaryKey::Simple(pk) => (*pk).to_string(),
            PrimaryKey::CompositeWithSurrogate { parts: (a, b), .. } => {
                format!("{a} || char(31) || {b}")
            }
        }
    }

    /// The `sync_tombstones` suppression key: identical to the pk value for
    /// a `Simple` table, or the `composite_sep`-joined composite for a
    /// `CompositeWithSurrogate` table (its visible `id` is a per-device
    /// autoincrement, not cross-device-stable).
    pub(crate) fn tombstone_key(&self, pk_values: &[&str], composite_sep: char) -> String {
        match &self.pk {
            PrimaryKey::Simple(_) => pk_values[0].to_string(),
            PrimaryKey::CompositeWithSurrogate { .. } => {
                format!("{}{}{}", pk_values[0], composite_sep, pk_values[1])
            }
        }
    }

    /// Binds every non-`extractor_only` column's value, in descriptor
    /// order. Returns `Ok(None)` when a `HexBlob` column's value is corrupt
    /// — the whole row must be skipped, not partially written.
    pub(crate) fn bind_all_columns(
        &self,
        row: &serde_json::Value,
        pk_values: &[&str],
    ) -> Result<Option<Vec<(&'static str, BoundValue)>>, StorageError> {
        let mut out = Vec::with_capacity(self.columns.len());
        for spec in self.columns {
            if spec.extractor_only {
                continue;
            }
            match spec.extract(row, pk_values)? {
                Some(v) => out.push((spec.name, v)),
                None => return Ok(None),
            }
        }
        Ok(Some(out))
    }

    /// `true` when `col_name` must be excluded from the generic routine's
    /// UPDATE SET list: either it is the `Simple` primary key (redundant —
    /// it is the WHERE match key, and re-setting it is a no-op the
    /// pre-refactor regimes/suggestions UPDATEs never bothered doing), or
    /// it is flagged [`ColumnSpec::immutable_on_update`]. A
    /// `CompositeWithSurrogate` table's natural-key columns
    /// (`embedding_vectors.segment_id`/`model_id`) are NOT excluded: the
    /// WHERE clause matches on the surrogate `id`, so re-setting them to
    /// the same value the lookup already matched on is a provable no-op
    /// (the pre-refactor UPDATE happened to already include `model_id` and
    /// omit `segment_id` here — an inconsistency with no observable effect
    /// either way, since both are the no-op case).
    pub(crate) fn excluded_from_update(&self, col_name: &str) -> bool {
        if let PrimaryKey::Simple(pk) = &self.pk {
            if *pk == col_name {
                return true;
            }
        }
        self.columns
            .iter()
            .find(|c| c.name == col_name)
            .is_some_and(|c| c.immutable_on_update)
    }

    /// Builds the extractor's `json_object(...)` SELECT projection — the
    /// single source for each table's column list on the read side,
    /// replacing 6 hand-spelled `json_object(...)` literals. A gated
    /// column ([`ExtractorGate`]) is OMITTED (not emitted as JSON `null`)
    /// exactly like the pre-refactor conditional string fragments, so a
    /// data-minimized sync still carries neither key. Always appends the
    /// `hlc_wall_ms`/`hlc_counter`/`origin_device_id` trailer every table
    /// carries.
    pub(crate) fn extractor_select_expr(&self, sync_config: &SyncConfig) -> String {
        let mut parts: Vec<String> = self
            .columns
            .iter()
            .filter(|c| c.extractor_gate.is_open(sync_config))
            .map(|c| {
                let expr = if matches!(c.kind, ColumnKind::HexBlob) {
                    format!("hex({0})", c.name)
                } else {
                    c.name.to_string()
                };
                format!("'{0}',{1}", c.name, expr)
            })
            .collect();
        parts.push("'hlc_wall_ms',hlc_wall_ms".to_string());
        parts.push("'hlc_counter',hlc_counter".to_string());
        parts.push("'origin_device_id',origin_device_id".to_string());
        format!("json_object({})", parts.join(","))
    }
}

// ──────────────────────────── JSON primitives ────────────────────────────
// Moved here (from `sync_merger`) as the shared row-codec layer both
// `ColumnSpec::extract` and `sync_merger`'s HLC-drift/suppression-key
// helpers depend on.

pub(crate) fn json_str<'a>(v: &'a serde_json::Value, key: &str) -> Result<&'a str, StorageError> {
    v.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| StorageError::Internal(format!("missing string field: {key}")))
}

pub(crate) fn json_str_opt(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

pub(crate) fn json_str_or_default<'a>(
    v: &'a serde_json::Value,
    key: &str,
    default: &'a str,
) -> &'a str {
    v.get(key).and_then(|v| v.as_str()).unwrap_or(default)
}

pub(crate) fn json_i64(v: &serde_json::Value, key: &str) -> Result<i64, StorageError> {
    v.get(key)
        .and_then(|v| v.as_i64())
        .ok_or_else(|| StorageError::Internal(format!("missing i64 field: {key}")))
}

pub(crate) fn json_i64_or_default(v: &serde_json::Value, key: &str, default: i64) -> i64 {
    v.get(key).and_then(|v| v.as_i64()).unwrap_or(default)
}

pub(crate) fn json_u64(v: &serde_json::Value, key: &str) -> Result<u64, StorageError> {
    v.get(key)
        .and_then(|v| v.as_u64())
        .ok_or_else(|| StorageError::Internal(format!("missing u64 field: {key}")))
}

pub(crate) fn json_f64(v: &serde_json::Value, key: &str) -> Result<f64, StorageError> {
    v.get(key)
        .and_then(|v| v.as_f64())
        .ok_or_else(|| StorageError::Internal(format!("missing f64 field: {key}")))
}

/// Extract a wire-supplied HLC counter, rejecting just the offending row on
/// overflow.
///
/// #6174: the `hlc_counter` field is part of every synced row's HLC. A
/// missing field is a structurally malformed changeset and stays a hard
/// error (same as every other required field above). But a *present*
/// counter that exceeds `u32::MAX` (corrupt/buggy/hostile peer) must NEVER
/// be silently truncated — that would corrupt causal ordering on merge.
/// Mirroring [`decode_vector`], an out-of-range counter logs a warning
/// naming the row and returns `None` so the caller REJECTS just that row
/// and keeps merging the rest of the changeset, instead of aborting the
/// whole transaction (and stalling sync, since the watermark would never
/// advance) over one bad peer row.
pub(crate) fn extract_hlc_counter(
    row: &serde_json::Value,
    row_label: &str,
) -> Result<Option<u32>, StorageError> {
    let raw = json_u64(row, "hlc_counter")?;
    match u32::try_from(raw) {
        Ok(counter) => Ok(Some(counter)),
        Err(_) => {
            tracing::warn!(
                row = %row_label,
                "rejected remote row with out-of-range HLC counter (would corrupt causal ordering): {raw}"
            );
            Ok(None)
        }
    }
}

/// Extract a full HLC from a wire row. Returns `None` (caller skips just
/// this row) when the counter is out of `u32` range — see
/// [`extract_hlc_counter`] (#6174).
pub(crate) fn extract_hlc(
    row: &serde_json::Value,
    row_label: &str,
) -> Result<Option<maekon_core::sync::Hlc>, StorageError> {
    let Some(counter) = extract_hlc_counter(row, row_label)? else {
        return Ok(None);
    };
    Ok(Some(maekon_core::sync::Hlc {
        wall_ms: json_u64(row, "hlc_wall_ms")?,
        counter,
        device_id: json_str(row, "origin_device_id")?.to_string(),
    }))
}

/// Decode a wire-supplied hex embedding vector back to its BLOB bytes.
///
/// #35 / #6081: a malformed hex string (truncated, odd-length, or non-hex
/// char from a corrupt/buggy/hostile peer) must NEVER silently collapse to
/// an empty `Vec<u8>` that would overwrite a valid local vector with a
/// zero-length blob under last-write-wins. Returns `None` on a decode
/// failure (logging a warning that names the offending row) so the caller
/// REJECTS just that row and keeps merging the rest of the changeset,
/// instead of aborting the whole transaction over one bad peer row.
pub(crate) fn decode_vector(vector_hex: &str, segment_id: &str, model_id: &str) -> Option<Vec<u8>> {
    match hex::decode(vector_hex) {
        Ok(bytes) => Some(bytes),
        Err(e) => {
            tracing::warn!(
                segment_id = %segment_id,
                model_id = %model_id,
                "rejected corrupt remote embedding vector (malformed hex): {e}"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_table_names_matches_registry_order() {
        assert_eq!(
            ALL_TABLE_NAMES,
            [
                "activity_segments",
                "regimes",
                "regime_overrides",
                "embedding_vectors",
                "suggestions",
                "trigger_params_snapshots",
            ]
        );
    }

    #[test]
    fn tombstone_pk_col_matches_simple_tables() {
        assert_eq!(tombstone_pk_col("activity_segments"), Some("id"));
        assert_eq!(tombstone_pk_col("regimes"), Some("id"));
        assert_eq!(tombstone_pk_col("regime_overrides"), Some("override_id"));
        assert_eq!(tombstone_pk_col("suggestions"), Some("suggestion_id"));
        assert_eq!(tombstone_pk_col("trigger_params_snapshots"), Some("id"));
    }

    #[test]
    fn tombstone_pk_col_none_for_composite_and_unknown() {
        assert_eq!(
            tombstone_pk_col("embedding_vectors"),
            None,
            "composite-key table handled separately by apply_tombstone"
        );
        assert_eq!(tombstone_pk_col("not_a_real_table"), None);
    }

    #[test]
    fn status_tiebreak_ordinal_matches_acted_dismissed_shown_priority() {
        let tb = &StatusTiebreak {
            presence_rank: &["acted_at", "dismissed_at", "shown_at"],
        };
        assert_eq!(
            tb.ordinal(&serde_json::json!({"acted_at": "t", "dismissed_at": "t", "shown_at": "t"})),
            3
        );
        assert_eq!(
            tb.ordinal(&serde_json::json!({"dismissed_at": "t", "shown_at": "t"})),
            2
        );
        assert_eq!(tb.ordinal(&serde_json::json!({"shown_at": "t"})), 1);
        assert_eq!(tb.ordinal(&serde_json::json!({})), 0);
    }

    #[test]
    fn excluded_from_update_covers_simple_pk_and_immutable_flag() {
        assert!(REGIMES.excluded_from_update("id"), "Simple pk excluded");
        assert!(!REGIMES.excluded_from_update("label"));
        assert!(
            SUGGESTIONS.excluded_from_update("created_at"),
            "immutable_on_update flag"
        );
        assert!(!SUGGESTIONS.excluded_from_update("content"));
        assert!(
            !EMBEDDING_VECTORS.excluded_from_update("segment_id"),
            "composite natural-key part is a no-op-safe SET, not excluded"
        );
        assert!(!EMBEDDING_VECTORS.excluded_from_update("model_id"));
    }

    #[test]
    fn extractor_select_expr_omits_gated_columns_when_closed() {
        let cfg = SyncConfig::default();
        let expr = ACTIVITY_SEGMENTS.extractor_select_expr(&cfg);
        assert!(!expr.contains("'llm_summary'"));
        assert!(!expr.contains("'llm_summary_status_json'"));
        assert!(expr.contains("'trigger_reason',trigger_reason"));
        assert!(expr.contains("'hlc_wall_ms',hlc_wall_ms"));
    }

    #[test]
    fn extractor_select_expr_includes_gated_columns_when_open() {
        let cfg = SyncConfig {
            include_llm_summary: true,
            ..SyncConfig::default()
        };
        let expr = ACTIVITY_SEGMENTS.extractor_select_expr(&cfg);
        assert!(expr.contains("'llm_summary',llm_summary"));
        assert!(expr.contains("'llm_summary_status_json',llm_summary_status_json"));
    }

    #[test]
    fn extractor_select_expr_hex_wraps_blob_columns() {
        let cfg = SyncConfig::default();
        let expr = EMBEDDING_VECTORS.extractor_select_expr(&cfg);
        assert!(expr.contains("'vector',hex(vector)"));
    }

    #[test]
    fn bind_all_columns_skips_extractor_only() {
        let row = serde_json::json!({
            "id": 42, "segment_id": "seg-1", "content_type": "screen",
            "vector": "0102", "model_id": "m1", "timestamp": "2026-01-01",
        });
        let pk_values = vec!["seg-1", "m1"];
        let bound = EMBEDDING_VECTORS
            .bind_all_columns(&row, &pk_values)
            .unwrap()
            .unwrap();
        assert!(
            bound.iter().all(|(name, _)| *name != "id"),
            "extractor_only 'id' must not be bound by the merger"
        );
        assert_eq!(
            bound[0].0, "segment_id",
            "segment_id is the first bound column"
        );
    }

    #[test]
    fn bind_all_columns_rejects_corrupt_hex_blob() {
        let row = serde_json::json!({
            "segment_id": "seg-1", "content_type": "screen",
            "vector": "nothex", "model_id": "m1", "timestamp": "2026-01-01",
        });
        let pk_values = vec!["seg-1", "m1"];
        let bound = EMBEDDING_VECTORS
            .bind_all_columns(&row, &pk_values)
            .unwrap();
        assert!(bound.is_none(), "corrupt HexBlob must skip the whole row");
    }
}
