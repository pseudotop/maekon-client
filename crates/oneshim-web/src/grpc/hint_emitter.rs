//! D13-v2b dashboard gRPC — per-stream ServerLoadHint emission bookkeeping.
//!
//! Tracks last-emitted `LoadLevel` + last-emit `Instant`. `maybe_emit` returns
//! `Some(ServerLoadHint)` iff the stream should send one:
//!   - first call of the stream (no prior emit)
//!   - level transition since last emit
//!   - `HEARTBEAT` (30s) elapsed since last emit
//!
//! `force_emit_degraded` bypasses the should-emit gate for DB-degraded
//! signaling (spec IMP-B2) but still updates state so the heartbeat clock
//! advances normally (prevents extra heartbeat immediately after degraded
//! emission).

use std::time::{Duration, Instant};

use crate::proto::dashboard::v1::server_load_hint::Level as ProtoLevel;
use crate::proto::dashboard::v1::ServerLoadHint;

use super::load_policy::{LoadLevel, LoadPolicy};

/// Per-stream heartbeat interval for `ServerLoadHint` emissions when neither
/// an initial emit nor a level transition fires.
pub const HEARTBEAT: Duration = Duration::from_secs(30);

pub struct HintEmitter {
    last_level: Option<LoadLevel>,
    last_emit_at: Option<Instant>,
}

impl HintEmitter {
    pub fn new() -> Self {
        Self {
            last_level: None,
            last_emit_at: None,
        }
    }

    /// Return `Some(ServerLoadHint)` iff the stream should emit one per the
    /// initial / transition / 30s-heartbeat rules. Mutates internal state
    /// BEFORE returning (so subsequent `maybe_emit` after a yield sees the
    /// updated state; spec §4.2 drop-ordering note).
    pub fn maybe_emit(
        &mut self,
        level: LoadLevel,
        policy: &LoadPolicy,
        cpu_pct: f32,
        memory_pct: f32,
        is_warmup: bool,
    ) -> Option<ServerLoadHint> {
        let now = Instant::now();
        let should = match (self.last_level, self.last_emit_at) {
            (None, _) => true,
            (Some(prev), _) if prev != level => true,
            (_, Some(t)) if now.duration_since(t) >= HEARTBEAT => true,
            _ => false,
        };
        if !should {
            return None;
        }
        self.last_level = Some(level);
        self.last_emit_at = Some(now);
        Some(build_hint(
            level, policy, cpu_pct, memory_pct, is_warmup, None,
        ))
    }

    /// IMP-B2: bypass the should_emit gate (for forced degraded-state signaling
    /// from the stream handler's DB-error consecutive-failure counter). Still
    /// mutates `last_level` + `last_emit_at` so the heartbeat clock advances.
    pub fn force_emit_degraded(
        &mut self,
        level: LoadLevel,
        policy: &LoadPolicy,
        cpu_pct: f32,
        memory_pct: f32,
        reason_tag: &str,
    ) -> ServerLoadHint {
        let now = Instant::now();
        self.last_level = Some(level);
        self.last_emit_at = Some(now);
        build_hint(level, policy, cpu_pct, memory_pct, false, Some(reason_tag))
    }
}

impl Default for HintEmitter {
    fn default() -> Self {
        Self::new()
    }
}

/// Build a `ServerLoadHint`.
///
/// `reason_override = Some(tag)` formats reason as `"{tag} (cpu=… mem=…)"`
/// (used by IMP-B2 `force_emit_degraded` — e.g. `"db_error_degraded"`).
/// Otherwise the reason is `"warmup {LEVEL} (…)"` during warm-up or
/// `"{LEVEL} (…)"` steady-state.
///
/// `suggested_event_rate_limit` is hardcoded to 0 in PR-B2 (CRIT-2) — real
/// population lands in PR-B3 alongside the `EventRateLimiter` component.
fn build_hint(
    level: LoadLevel,
    policy: &LoadPolicy,
    cpu_pct: f32,
    memory_pct: f32,
    is_warmup: bool,
    reason_override: Option<&str>,
) -> ServerLoadHint {
    let (proto_level, tag) = match level {
        LoadLevel::Low => (ProtoLevel::LoadLevelLow as i32, "LOW"),
        LoadLevel::Medium => (ProtoLevel::LoadLevelMedium as i32, "MEDIUM"),
        LoadLevel::High => (ProtoLevel::LoadLevelHigh as i32, "HIGH"),
        LoadLevel::Critical => (ProtoLevel::LoadLevelCritical as i32, "CRITICAL"),
    };
    let suggested_interval_secs = policy
        .enforced_metrics_interval(level, 0)
        .as_secs()
        .min(u32::MAX as u64) as u32;
    let reason = match reason_override {
        Some(t) => format!("{t} (cpu={cpu_pct:.1}% mem={memory_pct:.1}%)"),
        None if is_warmup => format!("warmup {tag} (cpu={cpu_pct:.1}% mem={memory_pct:.1}%)"),
        None => format!("{tag} (cpu={cpu_pct:.1}% mem={memory_pct:.1}%)"),
    };
    ServerLoadHint {
        load_level: proto_level,
        cpu_pct,
        memory_pct,
        suggested_interval_secs,
        suggested_event_rate_limit: 0,
        reason,
        emitted_at: Some(now_proto_ts()),
    }
}

fn now_proto_ts() -> prost_types::Timestamp {
    let now = chrono::Utc::now();
    prost_types::Timestamp {
        seconds: now.timestamp(),
        nanos: now.timestamp_subsec_nanos() as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::config::LoadThresholds;

    fn policy() -> LoadPolicy {
        LoadPolicy::new(LoadThresholds::default())
    }

    #[test]
    fn first_call_always_emits() {
        let mut e = HintEmitter::new();
        let h = e.maybe_emit(LoadLevel::Low, &policy(), 10.0, 30.0, false);
        assert!(h.is_some(), "first emit must fire");
    }

    #[test]
    fn same_level_inside_heartbeat_does_not_emit() {
        let mut e = HintEmitter::new();
        let _ = e.maybe_emit(LoadLevel::Low, &policy(), 10.0, 30.0, false);
        let second = e.maybe_emit(LoadLevel::Low, &policy(), 11.0, 31.0, false);
        assert!(second.is_none());
    }

    #[test]
    fn level_transition_emits() {
        let mut e = HintEmitter::new();
        let _ = e.maybe_emit(LoadLevel::Low, &policy(), 10.0, 30.0, false);
        let t = e.maybe_emit(LoadLevel::High, &policy(), 85.0, 70.0, false);
        assert!(t.is_some());
    }

    #[test]
    fn heartbeat_after_30s_emits_same_level() {
        let mut e = HintEmitter::new();
        let _ = e.maybe_emit(LoadLevel::Medium, &policy(), 50.0, 40.0, false);
        // Simulate passage of time by rewinding the internal clock.
        e.last_emit_at = Some(Instant::now() - Duration::from_secs(31));
        let h = e.maybe_emit(LoadLevel::Medium, &policy(), 51.0, 41.0, false);
        assert!(h.is_some(), "30s+ heartbeat should emit");
    }

    #[test]
    fn warmup_reason_prefix_present_during_first_30s() {
        let mut e = HintEmitter::new();
        let h = e
            .maybe_emit(
                LoadLevel::Medium,
                &policy(),
                20.0,
                10.0,
                /*is_warmup=*/ true,
            )
            .expect("first emit fires");
        assert!(
            h.reason.starts_with("warmup"),
            "reason should start with 'warmup' during warm-up, got {}",
            h.reason
        );
    }

    #[test]
    fn force_emit_degraded_advances_heartbeat_clock() {
        // IMP-B2: after force_emit_degraded, a same-level maybe_emit INSIDE
        // the heartbeat window must NOT fire — the degraded emit should have
        // advanced last_emit_at just like a regular emission.
        let mut e = HintEmitter::new();
        let _ = e.maybe_emit(LoadLevel::Medium, &policy(), 50.0, 40.0, false);
        let _h = e.force_emit_degraded(
            LoadLevel::Medium,
            &policy(),
            50.0,
            40.0,
            "db_error_degraded",
        );
        let follow = e.maybe_emit(LoadLevel::Medium, &policy(), 51.0, 41.0, false);
        assert!(follow.is_none(), "heartbeat clock must have advanced");
    }
}
