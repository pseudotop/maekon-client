use chrono::{DateTime, Utc};
use oneshim_core::models::analysis::{ActivityPattern, PatternType, TimeRange};
use oneshim_core::models::work_session::AppCategory;
use std::collections::{HashMap, HashSet};

/// Minimum duration (seconds) for a communication burst to be reported.
const COMM_BURST_MIN_SECS: i64 = 300;
/// Maximum gap (seconds) of non-comm app usage tolerated within a burst.
const BURST_GAP_TOLERANCE_SECS: i64 = 30;

// -- Context switching detector -------------------------------------------

pub(super) fn detect_context_switching(
    app_switches: &[(DateTime<Utc>, String)],
) -> Vec<ActivityPattern> {
    if app_switches.len() < 4 {
        return vec![];
    }

    let mut transitions: HashMap<(String, String), u32> = HashMap::new();
    for pair in app_switches.windows(2) {
        if pair[0].1 != pair[1].1 {
            let key = (pair[0].1.clone(), pair[1].1.clone());
            *transitions.entry(key).or_insert(0) += 1;
        }
    }

    let time_range = TimeRange {
        start: app_switches.first().map(|a| a.0).unwrap_or_else(Utc::now),
        end: app_switches.last().map(|a| a.0).unwrap_or_else(Utc::now),
    };

    transitions
        .into_iter()
        .filter(|(_, count)| *count >= 3)
        .map(|((from, to), count)| ActivityPattern {
            pattern_type: PatternType::ContextSwitch,
            description: format!("{from}\u{2194}{to} {count} times"),
            frequency: count,
            confidence: (count as f32 / app_switches.len() as f32).min(1.0),
            time_range: time_range.clone(),
            involved_apps: vec![from, to],
        })
        .collect()
}

// -- Work modes detector --------------------------------------------------

pub(super) fn detect_work_modes(app_switches: &[(DateTime<Utc>, String)]) -> Vec<ActivityPattern> {
    if app_switches.is_empty() {
        return vec![];
    }

    let mut coding_count = 0u32;
    let mut comm_count = 0u32;
    let mut browse_count = 0u32;

    for (_, app) in app_switches {
        let lower = app.to_lowercase();
        if is_coding_app(&lower) {
            coding_count += 1;
        } else if is_communication_app(&lower) {
            comm_count += 1;
        } else if is_browser_app(&lower) {
            browse_count += 1;
        }
    }

    let total = app_switches.len() as f32;
    let time_range = TimeRange {
        start: app_switches.first().map(|a| a.0).unwrap_or_else(Utc::now),
        end: app_switches.last().map(|a| a.0).unwrap_or_else(Utc::now),
    };

    let dominant = [
        ("coding", coding_count),
        ("communication", comm_count),
        ("browsing", browse_count),
    ]
    .into_iter()
    .max_by_key(|(_, c)| *c)
    .filter(|(_, c)| *c as f32 / total > 0.4);

    if let Some((mode, count)) = dominant {
        vec![ActivityPattern {
            pattern_type: PatternType::WorkMode,
            description: format!(
                "Dominant mode: {mode} ({count}/{} events)",
                app_switches.len()
            ),
            frequency: count,
            confidence: count as f32 / total,
            time_range,
            involved_apps: vec![],
        }]
    } else {
        vec![]
    }
}

// -- Deep work blocks detector --------------------------------------------

pub(super) fn detect_deep_work_blocks(
    app_switches: &[(DateTime<Utc>, String)],
) -> Vec<ActivityPattern> {
    if app_switches.len() < 2 {
        return vec![];
    }

    let mut patterns = Vec::new();
    let mut block_start = 0usize;

    for i in 1..app_switches.len() {
        let same_app = app_switches[i].1 == app_switches[block_start].1;
        let gap_secs = (app_switches[i].0 - app_switches[i - 1].0).num_seconds();

        if !same_app || gap_secs > 120 {
            let duration_secs = (app_switches[i - 1].0 - app_switches[block_start].0).num_seconds();

            if duration_secs >= 1800 && is_coding_app(&app_switches[block_start].1.to_lowercase()) {
                let block_len = i - block_start;
                patterns.push(ActivityPattern {
                    pattern_type: PatternType::DeepWorkBlock,
                    description: format!(
                        "Deep work in {} for {} min",
                        app_switches[block_start].1,
                        duration_secs / 60
                    ),
                    frequency: block_len as u32,
                    confidence: 0.8,
                    time_range: TimeRange {
                        start: app_switches[block_start].0,
                        end: app_switches[i - 1].0,
                    },
                    involved_apps: vec![app_switches[block_start].1.clone()],
                });
            }

            block_start = i;
        }
    }

    patterns
}

// -- Communication burst detector -----------------------------------------

/// Detect concentrated communication app usage periods (bursts).
///
/// A gap of up to [`BURST_GAP_TOLERANCE_SECS`] of non-communication app usage
/// (e.g. a brief Finder popup) is tolerated without breaking the burst.
pub(super) fn detect_communication_bursts(
    app_switches: &[(DateTime<Utc>, String)],
) -> Vec<ActivityPattern> {
    if app_switches.len() < 2 {
        return vec![];
    }

    let mut patterns = Vec::new();
    let mut burst_start: Option<usize> = None;
    let mut last_comm_idx: usize = 0;
    let mut gap_start: Option<DateTime<Utc>> = None;

    for (i, (ts, app)) in app_switches.iter().enumerate() {
        let is_comm = is_communication_app(&app.to_lowercase());

        if is_comm {
            gap_start = None;
            if burst_start.is_none() {
                burst_start = Some(i);
            }
            last_comm_idx = i;
        } else if let Some(start) = burst_start {
            // Non-comm app encountered during a burst — check gap tolerance
            if gap_start.is_none() {
                gap_start = Some(*ts);
            }
            let gap_duration = (*ts - app_switches[last_comm_idx].0).num_seconds();
            if gap_duration > BURST_GAP_TOLERANCE_SECS {
                // Gap exceeded tolerance — emit burst ending at last comm event
                emit_burst(app_switches, start, last_comm_idx, &mut patterns);
                burst_start = None;
                gap_start = None;
            }
        }

        // Handle last element: flush any open burst
        if i == app_switches.len() - 1 {
            if let Some(start) = burst_start {
                let end = if is_comm { i } else { last_comm_idx };
                emit_burst(app_switches, start, end, &mut patterns);
            }
        }
    }

    patterns
}

/// Helper to emit a communication burst pattern if it meets the minimum duration.
fn emit_burst(
    app_switches: &[(DateTime<Utc>, String)],
    start: usize,
    end: usize,
    patterns: &mut Vec<ActivityPattern>,
) {
    if end <= start {
        return;
    }
    let duration = (app_switches[end].0 - app_switches[start].0).num_seconds();
    if duration >= COMM_BURST_MIN_SECS {
        let involved: Vec<String> = app_switches[start..=end]
            .iter()
            .filter(|(_, a)| is_communication_app(&a.to_lowercase()))
            .map(|(_, a)| a.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        patterns.push(ActivityPattern {
            pattern_type: PatternType::CommunicationBurst,
            description: format!(
                "Communication burst: {} min of {}",
                duration / 60,
                involved.join(", ")
            ),
            frequency: (end - start + 1) as u32,
            confidence: 0.85,
            time_range: TimeRange {
                start: app_switches[start].0,
                end: app_switches[end].0,
            },
            involved_apps: involved,
        });
    }
}

// -- App category helpers -------------------------------------------------

/// Delegate to the canonical `AppCategory` in oneshim-core.
pub(crate) fn is_coding_app(name: &str) -> bool {
    AppCategory::is_coding(name)
}

pub(crate) fn is_communication_app(name: &str) -> bool {
    AppCategory::is_communication_app(name)
}

pub(crate) fn is_browser_app(name: &str) -> bool {
    AppCategory::is_browser(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use oneshim_core::models::event::{ContextEvent, Event};

    fn ctx_at(app: &str, ts: DateTime<Utc>) -> Event {
        Event::Context(ContextEvent {
            app_name: app.to_string(),
            window_title: format!("{app} Window"),
            prev_app_name: None,
            timestamp: ts,
            ..Default::default()
        })
    }

    #[test]
    fn communication_burst_detected() {
        let base = Utc::now() - Duration::minutes(30);
        let events = [
            ctx_at("VSCode", base),
            ctx_at("Slack", base + Duration::minutes(5)),
            ctx_at("Teams", base + Duration::minutes(7)),
            ctx_at("Slack", base + Duration::minutes(9)),
            ctx_at("Teams", base + Duration::minutes(11)),
            ctx_at("Slack", base + Duration::minutes(15)),
            ctx_at("VSCode", base + Duration::minutes(20)),
        ];

        let switches: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                Event::Context(ctx) => Some((ctx.timestamp, ctx.app_name.clone())),
                _ => None,
            })
            .collect();

        let patterns = detect_communication_bursts(&switches);
        assert!(!patterns.is_empty(), "should detect communication burst");
        assert_eq!(patterns[0].pattern_type, PatternType::CommunicationBurst);
    }

    #[test]
    fn burst_tolerates_brief_non_comm_gap() {
        // Slack(0m) → Finder(0m10s) → Slack(0m20s) → Slack(6m)
        // The 10-second Finder interruption is within the 30s tolerance,
        // so the entire sequence should be detected as a single burst.
        let base = Utc::now() - Duration::minutes(30);
        let switches = vec![
            (base, "Slack".to_string()),
            (base + Duration::seconds(10), "Finder".to_string()),
            (base + Duration::seconds(20), "Slack".to_string()),
            (base + Duration::minutes(6), "Slack".to_string()),
        ];

        let patterns = detect_communication_bursts(&switches);
        assert_eq!(patterns.len(), 1, "should detect exactly one burst");
        assert_eq!(patterns[0].pattern_type, PatternType::CommunicationBurst);
        // Duration should be ~6 minutes (360 seconds)
        let duration = (patterns[0].time_range.end - patterns[0].time_range.start).num_seconds();
        assert!(
            duration >= 300,
            "burst duration should be >= 5 min, got {duration}s"
        );
    }

    #[test]
    fn burst_detected_when_all_events_are_comm_apps() {
        let base = Utc::now() - Duration::minutes(30);
        let events = [
            ctx_at("Slack", base),
            ctx_at("Slack", base + Duration::minutes(2)),
            ctx_at("Teams", base + Duration::minutes(4)),
            ctx_at("Slack", base + Duration::minutes(6)),
            ctx_at("Slack", base + Duration::minutes(8)),
            ctx_at("Slack", base + Duration::minutes(10)),
        ];

        let app_switches: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                Event::Context(ctx) => Some((ctx.timestamp, ctx.app_name.clone())),
                _ => None,
            })
            .collect();

        let bursts = detect_communication_bursts(&app_switches);
        assert!(
            !bursts.is_empty(),
            "should detect burst when all events are comm apps"
        );
        assert_eq!(bursts[0].pattern_type, PatternType::CommunicationBurst);
    }

    #[test]
    fn no_burst_for_short_comm_usage() {
        let base = Utc::now() - Duration::minutes(10);
        let switches = vec![
            (base, "Slack".to_string()),
            (base + Duration::minutes(2), "Slack".to_string()),
            (base + Duration::minutes(5), "VSCode".to_string()),
        ];

        let patterns = detect_communication_bursts(&switches);
        assert!(
            patterns.is_empty(),
            "should not detect burst for < 5 min comm usage"
        );
    }
}
