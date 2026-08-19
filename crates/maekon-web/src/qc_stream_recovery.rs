//! Debug-only stream disconnect/reconnect injection for isolated QC profiles.
//!
//! The fixture closes the first two subscriptions for each bounded channel and
//! then leaves the production stream untouched. Every runtime gate must be
//! present, so an ordinary debug session cannot activate it accidentally.

use std::sync::atomic::{AtomicUsize, Ordering};

const DEBUG_GATE_ENV: &str = "MAEKON_DEBUG_QC_FIXTURE_CLI";
const ISOLATED_GATE_ENV: &str = "MAEKON_TC_ISOLATED_PROFILE";
const FLAVOR_ENV: &str = "MAEKON_APP_FLAVOR";
const STREAM_RECOVERY_GATE_ENV: &str = "MAEKON_DEBUG_QC_STREAM_RECOVERY_FIXTURE";
const STREAM_RECOVERY_MODE_ENV: &str = "MAEKON_QC_STREAM_RECOVERY_MODE";
const DROP_FIRST_CONNECTIONS: usize = 2;

static UPDATE_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
static APP_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
static GUI_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamChannel {
    Update,
    App,
    Gui,
}

impl StreamChannel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Update => "update",
            Self::App => "app",
            Self::Gui => "gui",
        }
    }

    fn attempts(self) -> &'static AtomicUsize {
        match self {
            Self::Update => &UPDATE_ATTEMPTS,
            Self::App => &APP_ATTEMPTS,
            Self::Gui => &GUI_ATTEMPTS,
        }
    }
}

/// Returns the number of events the production stream may yield. Zero closes
/// the current SSE response immediately; `usize::MAX` leaves it unbounded.
pub(crate) fn stream_limit(channel: StreamChannel) -> usize {
    let enabled = fixture_enabled_from_values(
        std::env::var(DEBUG_GATE_ENV).ok().as_deref(),
        std::env::var(ISOLATED_GATE_ENV).ok().as_deref(),
        std::env::var(FLAVOR_ENV).ok().as_deref(),
        std::env::var(STREAM_RECOVERY_GATE_ENV).ok().as_deref(),
        std::env::var(STREAM_RECOVERY_MODE_ENV).ok().as_deref(),
    );
    if !enabled {
        return usize::MAX;
    }

    let attempt = channel.attempts().fetch_add(1, Ordering::SeqCst) + 1;
    let limit = stream_limit_for_attempt(true, attempt);
    if limit == 0 {
        tracing::warn!(
            qc_stream_channel = channel.as_str(),
            qc_stream_attempt = attempt,
            "isolated QC stream-recovery fixture closed SSE subscription"
        );
    } else {
        tracing::info!(
            qc_stream_channel = channel.as_str(),
            qc_stream_attempt = attempt,
            "isolated QC stream-recovery fixture restored SSE subscription"
        );
    }
    limit
}

fn stream_limit_for_attempt(enabled: bool, attempt: usize) -> usize {
    if enabled && attempt <= DROP_FIRST_CONNECTIONS {
        0
    } else {
        usize::MAX
    }
}

fn fixture_enabled_from_values(
    debug_gate: Option<&str>,
    isolated_gate: Option<&str>,
    flavor: Option<&str>,
    stream_gate: Option<&str>,
    mode: Option<&str>,
) -> bool {
    debug_gate == Some("1")
        && isolated_gate == Some("1")
        && stream_gate == Some("1")
        && mode == Some("drop-first-two")
        && flavor.is_some_and(is_isolated_flavor)
}

fn is_isolated_flavor(flavor: &str) -> bool {
    let trimmed = flavor.trim();
    (trimmed.starts_with("qc-") || trimmed.starts_with("tc-"))
        && trimmed.len() > 3
        && trimmed
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_requires_every_isolation_gate() {
        let valid = [
            Some("1"),
            Some("1"),
            Some("qc-stream-recovery"),
            Some("1"),
            Some("drop-first-two"),
        ];

        for missing_index in 0..valid.len() {
            let mut values = valid;
            values[missing_index] = None;
            assert!(
                !fixture_enabled_from_values(values[0], values[1], values[2], values[3], values[4]),
                "gate {missing_index} must fail closed"
            );
        }
    }

    #[test]
    fn fixture_accepts_only_bounded_mode_and_flavor() {
        assert!(fixture_enabled_from_values(
            Some("1"),
            Some("1"),
            Some("qc-stream-recovery"),
            Some("1"),
            Some("drop-first-two"),
        ));
        assert!(!fixture_enabled_from_values(
            Some("1"),
            Some("1"),
            Some("production"),
            Some("1"),
            Some("drop-first-two"),
        ));
        assert!(!fixture_enabled_from_values(
            Some("1"),
            Some("1"),
            Some("tc-stream-recovery"),
            Some("1"),
            Some("unbounded-drop"),
        ));
    }

    #[test]
    fn fixture_plan_is_two_drops_then_unbounded_recovery() {
        assert_eq!(stream_limit_for_attempt(true, 1), 0);
        assert_eq!(stream_limit_for_attempt(true, 2), 0);
        assert_eq!(stream_limit_for_attempt(true, 3), usize::MAX);
        assert_eq!(stream_limit_for_attempt(false, 1), usize::MAX);
    }
}
