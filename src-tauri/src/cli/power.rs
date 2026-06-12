use super::{emit_debug_power_cli_json, DebugPowerCliCommand};

pub(crate) fn debug_power_capture_burst_audit_payload() -> serde_json::Value {
    let mut cadence = maekon_analysis::AdaptiveCaptureCadence::default();
    let base = chrono::Utc::now();

    let initial_capture = cadence.should_capture(maekon_analysis::CaptureRateRegime::Active, base);
    let pre_wake_immediate_capture = cadence.should_capture(
        maekon_analysis::CaptureRateRegime::Active,
        base + chrono::Duration::milliseconds(400),
    );

    let wake_tick = base + chrono::Duration::hours(8);
    let wake_gap_capture =
        cadence.should_capture(maekon_analysis::CaptureRateRegime::Active, wake_tick);
    let same_tick_burst_count = (0..5)
        .filter(|_| cadence.should_capture(maekon_analysis::CaptureRateRegime::Active, wake_tick))
        .count();
    let post_wake_immediate_capture = cadence.should_capture(
        maekon_analysis::CaptureRateRegime::Active,
        wake_tick + chrono::Duration::milliseconds(400),
    );
    let post_wake_interval_capture = cadence.should_capture(
        maekon_analysis::CaptureRateRegime::Active,
        wake_tick + chrono::Duration::milliseconds(500),
    );
    let no_spurious_capture_burst = initial_capture
        && !pre_wake_immediate_capture
        && wake_gap_capture
        && same_tick_burst_count == 0
        && !post_wake_immediate_capture
        && post_wake_interval_capture;

    serde_json::json!({
        "debug_power": true,
        "command": "capture-burst-audit",
        "initial_capture": initial_capture,
        "pre_wake_immediate_capture": pre_wake_immediate_capture,
        "wake_gap_hours": 8,
        "wake_gap_capture": wake_gap_capture,
        "same_tick_probe_count": 5,
        "same_tick_burst_count": same_tick_burst_count,
        "post_wake_immediate_capture": post_wake_immediate_capture,
        "post_wake_interval_capture": post_wake_interval_capture,
        "no_spurious_capture_burst": no_spurious_capture_burst,
    })
}

pub(crate) fn run_debug_power_cli_command(command: DebugPowerCliCommand) -> i32 {
    let payload = match command {
        DebugPowerCliCommand::CaptureBurstAudit => debug_power_capture_burst_audit_payload(),
    };
    let payload = serde_json::to_string(&payload).unwrap_or_else(|_| {
        "{\"debug_power\":true,\"ok\":false,\"error\":\"json serialization failed\"}".to_string()
    });
    emit_debug_power_cli_json(&payload)
}
