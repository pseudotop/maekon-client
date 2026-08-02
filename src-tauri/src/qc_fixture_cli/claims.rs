use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use chrono::{Duration, Utc};
use maekon_core::config_manager::ConfigManager;
use maekon_core::models::memory_graph::{
    ClaimKind, ClaimStatus, EdgeType, MemoryClaim, MemoryEdge,
};
use maekon_core::models::tiered_memory::{SegmentSummary, TriggerReason};
use maekon_core::ports::memory_graph_port::MemoryGraphPort;
use maekon_core::ports::storage::StorageService;
use maekon_storage::encryption::EncryptionKey;
use maekon_storage::sqlite::SqliteStorage;

use crate::storage_runtime::resolve_shared_master_key;

use super::types::ClaimsSeedReport;
use super::{
    require_exact_gate, require_isolated_profile, CLAIMS_GATE_ENV, CLAIMS_MARKER_KEY,
    CLAIMS_MARKER_VERSION, MARKER_IN_PROGRESS,
};

pub(crate) fn run_claims_from_env() -> Result<ClaimsSeedReport> {
    require_isolated_profile()?;
    require_exact_gate(CLAIMS_GATE_ENV)?;

    let config = ConfigManager::new()
        .context("initialize isolated config")?
        .get();
    let data_dir = ConfigManager::data_dir().context("resolve isolated data directory")?;
    std::fs::create_dir_all(&data_dir).context("create isolated data directory")?;
    let encryption_key =
        resolve_shared_master_key(&data_dir).context("resolve isolated profile encryption key")?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("create QC claims fixture runtime")?;
    runtime.block_on(seed_claims_fixture(
        &data_dir,
        encryption_key,
        config.storage.retention_days,
    ))
}

pub(super) async fn seed_claims_fixture(
    data_dir: &Path,
    encryption_key: EncryptionKey,
    retention_days: u32,
) -> Result<ClaimsSeedReport> {
    let db_path = data_dir.join(maekon_storage::encryption::SQLCIPHER_DB_FILENAME);
    let storage = Arc::new(
        SqliteStorage::open(&db_path, retention_days, Some(&encryption_key))
            .context("open isolated encrypted QC database")?,
    );

    match storage.get_meta(CLAIMS_MARKER_KEY).as_deref() {
        Some(CLAIMS_MARKER_VERSION) => {
            return Ok(ClaimsSeedReport {
                data_dir: data_dir.display().to_string(),
                claims: 0,
                segments: 0,
                edges: 0,
                already_seeded: true,
            });
        }
        Some(MARKER_IN_PROGRESS) => {
            bail!(
                "an earlier claims seed attempt did not finish; discard this isolated QC profile and retry"
            );
        }
        // v1 wrote claims + edges but omitted their synthetic source segments,
        // so the production orphan-edge GC correctly removed the evidence.
        // Re-running v2 repairs that isolated fixture in-place; all writes are
        // idempotent on stable synthetic ids.
        Some("1") => {}
        Some(other) => bail!("unsupported QC claims fixture marker version: {other}"),
        None => {}
    }

    storage
        .set_meta_checked(CLAIMS_MARKER_KEY, MARKER_IN_PROGRESS)
        .context("mark QC claims fixture seed in progress")?;

    let now = Utc::now().timestamp();
    let claims = synthetic_claims(now);
    let segments = synthetic_claim_segments(now);
    let edges = synthetic_claim_edges(now);
    let storage_service: Arc<dyn StorageService> = storage.clone();
    let memory_graph: Arc<dyn MemoryGraphPort> = storage.clone();
    for segment in &segments {
        storage_service
            .save_activity_segment(segment)
            .await
            .context("persist deterministic QC claim source segment")?;
    }
    for claim in &claims {
        memory_graph
            .save_claim(claim)
            .await
            .context("persist deterministic QC claim")?;
    }
    for edge in &edges {
        memory_graph
            .add_edge(edge)
            .await
            .context("persist deterministic QC claim provenance")?;
    }

    storage
        .set_meta_checked(CLAIMS_MARKER_KEY, CLAIMS_MARKER_VERSION)
        .context("commit QC claims fixture marker")?;

    Ok(ClaimsSeedReport {
        data_dir: data_dir.display().to_string(),
        claims: claims.len(),
        segments: segments.len(),
        edges: edges.len(),
        already_seeded: false,
    })
}

fn synthetic_claim_segments(now: i64) -> [SegmentSummary; 3] {
    [
        synthetic_claim_segment("qc-cj04-04-segment-a", now - 180),
        synthetic_claim_segment("qc-cj04-04-segment-b", now - 150),
        synthetic_claim_segment("qc-cj04-04-segment-c", now - 270),
    ]
}

fn synthetic_claim_segment(segment_id: &str, start_epoch_secs: i64) -> SegmentSummary {
    let start_time = chrono::DateTime::from_timestamp(start_epoch_secs, 0)
        .expect("fixed QC fixture timestamp must be representable");
    SegmentSummary {
        segment_id: segment_id.to_string(),
        start_time,
        end_time: start_time + Duration::seconds(30),
        duration_secs: 30,
        regime_id: None,
        trigger_reason: TriggerReason::ScoreHigh,
        event_count: 0,
        app_breakdown: HashMap::new(),
        category_breakdown: HashMap::new(),
        context_switch_count: 0,
        dominant_category: "QC synthetic provenance".to_string(),
        avg_importance: 0.0,
        patterns_detected: Vec::new(),
        content_activities: Vec::new(),
        container: None,
        llm_summary: None,
    }
}

fn synthetic_claims(now: i64) -> [MemoryClaim; 4] {
    [
        synthetic_claim(
            "qc-cj04-04-procedural-active",
            ClaimKind::Procedural,
            "A short planning pass helps turn a review into one concrete next step.",
            "digest_highlight",
            0.91,
            ClaimStatus::Active,
            now - 120,
        ),
        synthetic_claim(
            "qc-cj04-04-semantic-active",
            ClaimKind::Semantic,
            "Focused work sessions are most useful when the goal is stated clearly.",
            "digest_timeline",
            0.84,
            ClaimStatus::Active,
            now - 240,
        ),
        synthetic_claim(
            "qc-cj04-04-episodic-superseded",
            ClaimKind::Episodic,
            "A synthetic review session previously ended with a documented handoff.",
            "pattern_miner",
            0.72,
            ClaimStatus::Superseded,
            now - 360,
        ),
        synthetic_claim(
            "qc-cj04-04-reflective-retracted",
            ClaimKind::Reflective,
            "A synthetic reflection was withdrawn and must remain excluded from future use.",
            "digest_highlight",
            0.65,
            ClaimStatus::Retracted,
            now - 480,
        ),
    ]
}

fn synthetic_claim(
    claim_id: &str,
    kind: ClaimKind,
    text: &str,
    source: &str,
    confidence: f32,
    status: ClaimStatus,
    created_at: i64,
) -> MemoryClaim {
    MemoryClaim {
        claim_id: claim_id.to_string(),
        kind,
        text: text.to_string(),
        source: source.to_string(),
        confidence,
        status,
        created_at,
        updated_at: created_at,
    }
}

fn synthetic_claim_edges(now: i64) -> [MemoryEdge; 3] {
    [
        synthetic_evidence_edge(
            "qc-cj04-04-edge-procedural-a",
            "qc-cj04-04-procedural-active",
            "qc-cj04-04-segment-a",
            now - 120,
        ),
        synthetic_evidence_edge(
            "qc-cj04-04-edge-procedural-b",
            "qc-cj04-04-procedural-active",
            "qc-cj04-04-segment-b",
            now - 120,
        ),
        synthetic_evidence_edge(
            "qc-cj04-04-edge-semantic-a",
            "qc-cj04-04-semantic-active",
            "qc-cj04-04-segment-c",
            now - 240,
        ),
    ]
}

fn synthetic_evidence_edge(
    edge_id: &str,
    claim_id: &str,
    segment_id: &str,
    created_at: i64,
) -> MemoryEdge {
    MemoryEdge {
        edge_id: edge_id.to_string(),
        src_id: claim_id.to_string(),
        dst_id: segment_id.to_string(),
        edge_type: EdgeType::Evidence,
        confidence: 1.0,
        evidence_ref: Some(segment_id.to_string()),
        source: "qc_fixture".to_string(),
        created_at,
    }
}
