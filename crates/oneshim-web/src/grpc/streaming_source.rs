//! Dual-mode source for streaming config fields shared between loopback
//! and external gRPC servers (spec §5.8 / D24).
//!
//! Loopback `DashboardServiceImpl::from_spawn_config` constructs `Fixed`;
//! external `from_external_spawn_config` constructs `Live`. Handlers call
//! `.streaming_enabled()` / `.load_policy()` uniformly.

use std::sync::Arc;

use crate::grpc::external::live_config::LiveExternalConfig;
use crate::grpc::load_policy::LoadPolicy;

#[derive(Clone)]
#[allow(dead_code)] // Phase 1 scaffold; consumed in Phase 5 (DashboardServiceImpl.streaming_source)
pub(crate) enum StreamingSource {
    /// Boot-time captured values. Loopback server uses this variant.
    Fixed {
        streaming_enabled: bool,
        load_policy: Arc<LoadPolicy>,
    },
    /// Live-reloadable via ConfigReloadTask. External server uses this variant.
    Live(Arc<LiveExternalConfig>),
}

#[allow(dead_code)] // Phase 1 scaffold; consumed in Phase 5
impl StreamingSource {
    pub fn streaming_enabled(&self) -> bool {
        match self {
            Self::Fixed {
                streaming_enabled, ..
            } => *streaming_enabled,
            Self::Live(live) => live.snapshot().streaming_enabled,
        }
    }

    pub fn load_policy(&self) -> Arc<LoadPolicy> {
        match self {
            Self::Fixed { load_policy, .. } => load_policy.clone(),
            Self::Live(live) => live.snapshot().load_policy.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grpc::external::live_config::LiveSnapshot;
    use oneshim_core::config::LoadThresholds;

    fn fixture_policy() -> Arc<LoadPolicy> {
        Arc::new(LoadPolicy::new(LoadThresholds {
            cpu_low_pct: 30.0,
            cpu_medium_pct: 60.0,
            cpu_high_pct: 85.0,
            min_free_mem_gb: 1.0,
        }))
    }

    #[test]
    fn fixed_returns_captured_values() {
        let policy = fixture_policy();
        let src = StreamingSource::Fixed {
            streaming_enabled: true,
            load_policy: policy.clone(),
        };
        assert!(src.streaming_enabled());
        assert!(Arc::ptr_eq(&src.load_policy(), &policy));
    }

    #[test]
    fn live_reads_from_snapshot() {
        let policy = fixture_policy();
        let live = Arc::new(LiveExternalConfig::new(LiveSnapshot {
            streaming_enabled: false,
            load_policy: policy.clone(),
        }));
        let src = StreamingSource::Live(live.clone());
        assert!(!src.streaming_enabled());
        assert!(Arc::ptr_eq(&src.load_policy(), &policy));
    }

    #[test]
    fn clone_is_cheap_and_preserves_semantics() {
        let live = Arc::new(LiveExternalConfig::new(LiveSnapshot {
            streaming_enabled: true,
            load_policy: fixture_policy(),
        }));
        let src = StreamingSource::Live(live.clone());
        let clone = src.clone();
        assert_eq!(src.streaming_enabled(), clone.streaming_enabled());
    }
}
