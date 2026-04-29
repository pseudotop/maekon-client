use chrono::{DateTime, Duration, Utc};
use oneshim_core::models::analysis::{ActivityPattern, PatternType, TimeRange};
use std::collections::{BTreeSet, HashSet};

/// Minimum fraction of time windows for a co-occurrence pattern (0.0..1.0).
const MIN_SUPPORT: f32 = 0.4;
/// Duration of each time window for co-occurrence analysis (minutes).
const WINDOW_MINS: i64 = 30;

/// Simplified FP-Growth-inspired frequent itemset mining.
/// Detects apps frequently used together within the same time window.
///
/// Steps:
/// 1. Bucket events into fixed 30-minute windows
/// 2. Collect unique app names per window
/// 3. Count how many windows each pair/triple appears in
/// 4. Filter by minimum support threshold (40% of windows)
pub(super) fn detect_co_occurrence_patterns(
    app_switches: &[(DateTime<Utc>, String)],
) -> Vec<ActivityPattern> {
    if app_switches.len() < 2 {
        return vec![];
    }

    // Ensure chronological order — callers may not guarantee sorted input.
    let mut sorted = app_switches.to_vec();
    sorted.sort_by_key(|(ts, _)| *ts);

    let first_ts = sorted.first().expect("len >= 2").0;
    let last_ts = sorted.last().expect("len >= 2").0;
    let window_dur = Duration::minutes(WINDOW_MINS);

    // Bucket events into fixed time windows and collect unique apps per window
    let mut windows: Vec<HashSet<String>> = Vec::new();
    let mut win_start = first_ts;
    while win_start <= last_ts {
        let win_end = win_start + window_dur;
        let apps: HashSet<String> = sorted
            .iter()
            .filter(|(ts, _)| *ts >= win_start && *ts < win_end)
            .map(|(_, app)| app.clone())
            .collect();
        if !apps.is_empty() {
            windows.push(apps);
        }
        win_start = win_end;
    }

    if windows.len() < 2 {
        return vec![];
    }

    let total_windows = windows.len() as f32;
    let time_range = TimeRange {
        start: first_ts,
        end: last_ts,
    };

    // Collect all unique app names, sorted for deterministic iteration
    let all_apps: BTreeSet<String> = windows.iter().flat_map(|w| w.iter().cloned()).collect();
    let app_list: Vec<String> = all_apps.into_iter().collect();

    let mut patterns = Vec::new();

    // Count pairs
    for i in 0..app_list.len() {
        for j in (i + 1)..app_list.len() {
            let count = windows
                .iter()
                .filter(|w| w.contains(&app_list[i]) && w.contains(&app_list[j]))
                .count() as f32;
            let support = count / total_windows;
            if support >= MIN_SUPPORT {
                let apps = vec![app_list[i].clone(), app_list[j].clone()];
                patterns.push(ActivityPattern {
                    pattern_type: PatternType::CoOccurrence,
                    description: format!(
                        "{{{}, {}}} co-occur in {:.0}% of windows",
                        apps[0],
                        apps[1],
                        support * 100.0
                    ),
                    frequency: count as u32,
                    confidence: support,
                    time_range: time_range.clone(),
                    involved_apps: apps,
                });
            }
        }
    }

    // Count triples
    for i in 0..app_list.len() {
        for j in (i + 1)..app_list.len() {
            for k in (j + 1)..app_list.len() {
                let count = windows
                    .iter()
                    .filter(|w| {
                        w.contains(&app_list[i])
                            && w.contains(&app_list[j])
                            && w.contains(&app_list[k])
                    })
                    .count() as f32;
                let support = count / total_windows;
                if support >= MIN_SUPPORT {
                    let apps = vec![
                        app_list[i].clone(),
                        app_list[j].clone(),
                        app_list[k].clone(),
                    ];
                    patterns.push(ActivityPattern {
                        pattern_type: PatternType::CoOccurrence,
                        description: format!(
                            "{{{}, {}, {}}} co-occur in {:.0}% of windows",
                            apps[0],
                            apps[1],
                            apps[2],
                            support * 100.0
                        ),
                        frequency: count as u32,
                        confidence: support,
                        time_range: time_range.clone(),
                        involved_apps: apps,
                    });
                }
            }
        }
    }

    patterns
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use oneshim_core::models::event::{ContextEvent, Event};

    fn ctx_at(app: &str, ts: DateTime<Utc>) -> (DateTime<Utc>, String) {
        (ts, app.to_string())
    }

    fn ctx_event_at(app: &str, ts: DateTime<Utc>) -> Event {
        Event::Context(ContextEvent {
            app_name: app.to_string(),
            window_title: format!("{app} Window"),
            prev_app_name: None,
            timestamp: ts,
            ..Default::default()
        })
    }

    #[test]
    fn detect_pair_co_occurrence() {
        // 4 windows of 30 min. Slack+Chrome appear in 3/4 (75%).
        let base = Utc::now() - Duration::hours(2);
        let switches = vec![
            // Window 0 (0-30 min): Slack, Chrome
            ctx_at("Slack", base),
            ctx_at("Chrome", base + Duration::minutes(10)),
            // Window 1 (30-60 min): Slack, Chrome, VSCode
            ctx_at("Slack", base + Duration::minutes(35)),
            ctx_at("Chrome", base + Duration::minutes(40)),
            ctx_at("VSCode", base + Duration::minutes(45)),
            // Window 2 (60-90 min): VSCode only
            ctx_at("VSCode", base + Duration::minutes(65)),
            // Window 3 (90-120 min): Slack, Chrome
            ctx_at("Slack", base + Duration::minutes(95)),
            ctx_at("Chrome", base + Duration::minutes(100)),
        ];

        let patterns = detect_co_occurrence_patterns(&switches);

        let slack_chrome = patterns.iter().find(|p| {
            p.pattern_type == PatternType::CoOccurrence
                && p.involved_apps.contains(&"Slack".to_string())
                && p.involved_apps.contains(&"Chrome".to_string())
                && p.involved_apps.len() == 2
        });
        assert!(
            slack_chrome.is_some(),
            "should detect Slack+Chrome co-occurrence"
        );
        assert!(slack_chrome.unwrap().confidence >= 0.5);
    }

    #[test]
    fn no_co_occurrence_for_single_window() {
        // All events in one window — need >= 2 windows
        let switches = vec![
            (Utc::now(), "Slack".to_string()),
            (Utc::now() + Duration::minutes(1), "Chrome".to_string()),
        ];

        let patterns = detect_co_occurrence_patterns(&switches);
        assert!(
            patterns.is_empty(),
            "should not detect co-occurrence with < 2 windows"
        );
    }

    #[test]
    fn below_support_threshold_not_reported() {
        // 5 windows, pair only in 1 (20% < 40%)
        let base = Utc::now() - Duration::hours(3);
        let switches = vec![
            ctx_at("Slack", base),
            ctx_at("Chrome", base + Duration::minutes(10)),
            ctx_at("VSCode", base + Duration::minutes(35)),
            ctx_at("Finder", base + Duration::minutes(65)),
            ctx_at("Terminal", base + Duration::minutes(95)),
            ctx_at("Notes", base + Duration::minutes(125)),
        ];

        let patterns = detect_co_occurrence_patterns(&switches);
        // No pair should reach 40% since each window has only 1 app
        assert!(
            patterns.is_empty(),
            "should not detect patterns below 40% support"
        );
    }

    #[test]
    fn integration_via_pattern_miner() {
        let base = Utc::now() - Duration::hours(2);
        let events = vec![
            ctx_event_at("Slack", base),
            ctx_event_at("Chrome", base + Duration::minutes(10)),
            ctx_event_at("Slack", base + Duration::minutes(35)),
            ctx_event_at("Chrome", base + Duration::minutes(40)),
            ctx_event_at("VSCode", base + Duration::minutes(65)),
            ctx_event_at("Slack", base + Duration::minutes(95)),
            ctx_event_at("Chrome", base + Duration::minutes(100)),
        ];

        let miner = super::super::PatternMiner::new();
        let patterns = miner.detect(&events);

        let co_occur: Vec<_> = patterns
            .iter()
            .filter(|p| p.pattern_type == PatternType::CoOccurrence)
            .collect();
        assert!(!co_occur.is_empty());
    }
}
