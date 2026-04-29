use chrono::{DateTime, Utc};
use oneshim_core::models::analysis::{ActivityPattern, PatternType, TimeRange};
use std::collections::HashMap;

/// Minimum number of occurrences for a sequential pattern to be reported.
const MIN_SUPPORT: u32 = 3;

/// PrefixSpan-inspired sequential pattern mining.
/// Detects frequently recurring app-switch sequences of length 2, 3, and 4.
///
/// Steps:
/// 1. Deduplicate consecutive same-app entries (only count actual switches)
/// 2. Slide windows of size 2, 3, 4 over the deduped sequence
/// 3. Count frequency of each unique subsequence
/// 4. Filter by minimum support threshold
pub(super) fn detect_sequential_patterns(
    app_switches: &[(DateTime<Utc>, String)],
) -> Vec<ActivityPattern> {
    // Deduplicate consecutive same-app entries to get actual switches
    let deduped: Vec<&(DateTime<Utc>, String)> = app_switches
        .iter()
        .filter({
            let mut prev: Option<&str> = None;
            move |entry| {
                let dominated = prev == Some(entry.1.as_str());
                prev = Some(&entry.1);
                !dominated
            }
        })
        .collect();

    if deduped.len() < 2 {
        return vec![];
    }

    let time_range = TimeRange {
        start: app_switches.first().map(|a| a.0).unwrap_or_else(Utc::now),
        end: app_switches.last().map(|a| a.0).unwrap_or_else(Utc::now),
    };

    let mut patterns = Vec::new();

    // Slide windows of size 2, 3, 4 over deduped sequence
    for window_size in 2..=4usize {
        if deduped.len() < window_size {
            continue;
        }

        let mut freq: HashMap<Vec<String>, u32> = HashMap::new();
        for win in deduped.windows(window_size) {
            let key: Vec<String> = win.iter().map(|(_, app)| app.clone()).collect();
            *freq.entry(key).or_insert(0) += 1;
        }

        for (seq, count) in freq {
            if count >= MIN_SUPPORT {
                let desc = format!("Sequence {} occurs {count} times", seq.join(" -> "));
                patterns.push(ActivityPattern {
                    pattern_type: PatternType::AppSequence,
                    description: desc,
                    frequency: count,
                    confidence: {
                        let possible_windows = (deduped.len() - window_size + 1) as f32;
                        (count as f32 / possible_windows).min(1.0)
                    },
                    time_range: time_range.clone(),
                    involved_apps: seq,
                });
            }
        }
    }

    patterns
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use oneshim_core::models::analysis::PatternType;
    use oneshim_core::models::event::Event;

    use super::super::tests::make_ctx_event;

    fn make_switch(app: &str, mins_offset: i64) -> (DateTime<Utc>, String) {
        (Utc::now() - Duration::minutes(mins_offset), app.to_string())
    }

    #[test]
    fn repeating_triple_detected() {
        // Slack -> Chrome -> VSCode repeated 4 times (12 events, all distinct)
        let apps = ["Slack", "Chrome", "VSCode"];
        let switches: Vec<(DateTime<Utc>, String)> = (0..12)
            .map(|i| make_switch(apps[i as usize % 3], 60 - i))
            .collect();

        let patterns = detect_sequential_patterns(&switches);

        assert!(!patterns.is_empty(), "should detect sequential patterns");

        let triple = patterns
            .iter()
            .find(|p| p.involved_apps == vec!["Slack", "Chrome", "VSCode"] && p.frequency >= 3);
        assert!(
            triple.is_some(),
            "should find Slack->Chrome->VSCode with freq >= 3"
        );
    }

    #[test]
    fn deduplicates_consecutive_same_app() {
        // VSCode, VSCode, Slack, Slack, VSCode... -> deduped: VSCode, Slack, VSCode...
        let switches = vec![
            make_switch("VSCode", 10),
            make_switch("VSCode", 9),
            make_switch("Slack", 8),
            make_switch("Slack", 7),
            make_switch("VSCode", 6),
            make_switch("VSCode", 5),
            make_switch("Slack", 4),
            make_switch("Slack", 3),
            make_switch("VSCode", 2),
            make_switch("VSCode", 1),
            make_switch("Slack", 0),
        ];

        let patterns = detect_sequential_patterns(&switches);

        // Deduped: VSCode, Slack, VSCode, Slack, VSCode, Slack (6 entries)
        // Pair "VSCode -> Slack" should appear 3 times
        let pair = patterns
            .iter()
            .find(|p| p.involved_apps == vec!["VSCode", "Slack"] && p.frequency >= 3);
        assert!(
            pair.is_some(),
            "should detect VSCode->Slack pair after dedup"
        );
    }

    #[test]
    fn no_patterns_below_threshold() {
        // Only 2 distinct switches — not enough for min_support=3
        let switches = vec![
            make_switch("Slack", 3),
            make_switch("Chrome", 2),
            make_switch("Slack", 1),
        ];

        let patterns = detect_sequential_patterns(&switches);
        assert!(
            patterns.is_empty(),
            "should not detect patterns below threshold"
        );
    }

    #[test]
    fn single_app_after_dedup_returns_empty() {
        let switches: Vec<(DateTime<Utc>, String)> =
            (0..5).map(|i| make_switch("VSCode", 10 - i)).collect();

        let patterns = detect_sequential_patterns(&switches);
        assert!(patterns.is_empty(), "single app should yield no patterns");
    }

    #[test]
    fn integration_via_extract() {
        // Verify it works with real Event objects through PatternMiner
        let apps = ["Slack", "Chrome", "VSCode"];
        let events: Vec<Event> = (0..12)
            .map(|i| make_ctx_event(apps[i as usize % 3], 60 - i))
            .collect();

        let miner = super::super::PatternMiner::new();
        let patterns = miner.detect(&events);

        let seqs: Vec<_> = patterns
            .iter()
            .filter(|p| p.pattern_type == PatternType::AppSequence)
            .collect();
        assert!(!seqs.is_empty());
    }
}
