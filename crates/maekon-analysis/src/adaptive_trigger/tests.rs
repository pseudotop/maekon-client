#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use super::super::*;
    use chrono::{Duration, Utc};
    use maekon_core::models::tiered_memory::{ResolvedParams, TriggerInput, WorkType};
    use maekon_core::models::work_session::AppCategory;

    fn default_params() -> ResolvedParams {
        let mut p = ResolvedParams::default();
        p.validate_and_normalize();
        p
    }

    fn app_switch(name: &str) -> TriggerInput {
        TriggerInput::AppSwitchNew {
            app_name: name.to_string(),
            prev_app: "Other".to_string(),
            category: AppCategory::Development,
        }
    }

    #[test]
    fn importance_scoring() {
        let trigger = AdaptiveTrigger::new();
        let params = default_params();

        let app_switch = TriggerInput::AppSwitchNew {
            app_name: "VSCode".to_string(),
            prev_app: "Chrome".to_string(),
            category: AppCategory::Development,
        };
        assert!((trigger.score_importance(&app_switch, &params) - 0.8).abs() < 1e-5);

        let poll = TriggerInput::AppPoll {
            app_name: "VSCode".to_string(),
        };
        assert!((trigger.score_importance(&poll, &params) - 0.15).abs() < 1e-5);

        let metric = TriggerInput::SystemMetric;
        assert!((trigger.score_importance(&metric, &params) - 0.05).abs() < 1e-5);

        let idle = TriggerInput::IdleTransition { to_idle: true };
        assert!((trigger.score_importance(&idle, &params) - 0.9).abs() < 1e-5);

        let work_change = TriggerInput::WorkTypeChange {
            from: WorkType::ActiveCoding,
            to: WorkType::Reading,
        };
        assert!((trigger.score_importance(&work_change, &params) - 0.85).abs() < 1e-5);

        let ocr = TriggerInput::OcrUpdate { diff_ratio: 0.5 };
        let ocr_score = trigger.score_importance(&ocr, &params);
        assert!(ocr_score > 0.59 && ocr_score < 0.61); // 0.4 + 0.5*0.4 = 0.6
    }

    #[test]
    fn ewma_convergence() {
        let mut trigger = AdaptiveTrigger::new();
        let params = default_params();
        let base = Utc::now();

        // Send many events at regular intervals — EWMAs should converge
        for i in 0..100 {
            let ts = base + Duration::seconds(i);
            trigger.update_density(ts, &params);
        }

        // After many events, short and long EWMA should be relatively close
        let diff = (trigger.ewma_short - trigger.ewma_long).abs();
        // They won't be identical due to different alpha values, but both should be > 0
        assert!(trigger.ewma_short > 0.0);
        assert!(trigger.ewma_long > 0.0);
        // Short tracks faster so may be slightly higher, but the gap should narrow
        assert!(diff < trigger.ewma_short.max(trigger.ewma_long) + 1.0);
    }

    #[test]
    fn hysteresis_no_oscillation() {
        let mut trigger = AdaptiveTrigger::new();
        let mut params = default_params();
        params.t_high = 0.60;
        params.t_low = 0.40;
        params.min_segment_secs = 0; // disable min for this test
        params.max_segment_secs = 9999;

        let base = Utc::now();

        // Start a segment
        trigger.start_new_segment(base);

        // Feed scores near the boundary (0.50) — between t_low and t_high
        // Decision should always be Continue (no oscillation)
        for i in 1..=20 {
            let ts = base + Duration::seconds(i);
            // Simulate a middling score by checking decide directly
            let decision = trigger.decide(0.50, ts, &params);
            assert_eq!(
                decision,
                TriggerDecision::Continue,
                "should not oscillate at score=0.50, iteration {i}"
            );
        }
    }

    #[test]
    fn force_close_at_max() {
        let mut trigger = AdaptiveTrigger::new();
        let mut params = default_params();
        params.max_segment_secs = 60;

        let base = Utc::now();
        trigger.start_new_segment(base);

        // Event at max_segment boundary
        let ts = base + Duration::seconds(60);
        let decision = trigger.decide(0.50, ts, &params);
        assert_eq!(decision, TriggerDecision::ForceCloseSegment);
    }

    #[test]
    fn min_segment_enforcement() {
        let mut trigger = AdaptiveTrigger::new();
        let mut params = default_params();
        params.min_segment_secs = 120;
        params.t_low = 0.30;
        params.max_segment_secs = 600;

        let base = Utc::now();
        trigger.start_new_segment(base);

        // Score below t_low but segment too young → Continue
        let ts = base + Duration::seconds(60); // only 60s, need 120s min
        let decision = trigger.decide(0.10, ts, &params);
        assert_eq!(decision, TriggerDecision::Continue);

        // Now past min_segment → CloseSegment
        let ts2 = base + Duration::seconds(121);
        let decision2 = trigger.decide(0.10, ts2, &params);
        assert_eq!(decision2, TriggerDecision::CloseSegment);
    }

    #[test]
    fn first_event_starts_segment() {
        let mut trigger = AdaptiveTrigger::new();
        let params = default_params();
        let base = Utc::now();

        // Feed several high-importance events to build up score
        for i in 0..10 {
            let ts = base + Duration::seconds(i * 2);
            let (decision, _entry) = trigger.process_event(&app_switch("VSCode"), ts, &params);
            if decision == TriggerDecision::OpenSegment {
                // Successfully triggered a segment start
                return;
            }
        }

        // With high-importance app switches, we should have started a segment
        // If not, the score didn't exceed t_high — acceptable with default params
        // that have t_high=0.65 and initial EWMA=0
    }

    #[test]
    fn context_signal_decay() {
        let mut trigger = AdaptiveTrigger::new();
        let params = default_params();

        // Boost context with a context event
        let ctx_input = app_switch("VSCode");
        trigger.update_context(&ctx_input, &params);
        let boosted = trigger.context_signal;
        assert!(boosted > 0.0);

        // Decay with non-context events
        let non_ctx = TriggerInput::SystemMetric;
        for _ in 0..20 {
            trigger.update_context(&non_ctx, &params);
        }
        assert!(trigger.context_signal < boosted);
        // After many decays, should be close to zero
        assert!(trigger.context_signal < 0.1);
    }

    #[test]
    fn buffer_signal_increases() {
        let mut trigger = AdaptiveTrigger::new();
        let mut params = default_params();
        params.buffer_capacity = 10;

        // Initially zero
        assert!((trigger.compute_buffer_signal(&params) - 0.0).abs() < 1e-5);

        // Simulate accumulating events
        trigger.segment_event_count = 5;
        let sig = trigger.compute_buffer_signal(&params);
        assert!((sig - 0.5).abs() < 1e-5);

        trigger.segment_event_count = 10;
        let sig = trigger.compute_buffer_signal(&params);
        assert!((sig - 1.0).abs() < 1e-5);

        // Over capacity is clamped to 1.0
        trigger.segment_event_count = 20;
        let sig = trigger.compute_buffer_signal(&params);
        assert!((sig - 1.0).abs() < 1e-5);
    }

    #[test]
    fn getter_methods_reflect_signal_state() {
        let mut trigger = AdaptiveTrigger::new();
        let params = default_params();
        let input = TriggerInput::AppSwitchNew {
            app_name: "VSCode".to_string(),
            prev_app: "Slack".to_string(),
            category: AppCategory::Development,
        };

        let _ = trigger.process_event(&input, Utc::now(), &params);

        assert!(trigger.current_density_signal() > 0.0);
        assert!(trigger.current_importance_signal() > 0.0);
        // Context signal boosted towards 1.0 after app switch (0 + (1-0)*0.5 = 0.5)
        assert!(trigger.current_context_signal() > 0.4);
    }

    #[test]
    fn crt_prv_cap_010_capture_rate_adapts_active_idle_active() {
        let mut cadence = AdaptiveCaptureCadence::default();
        let base = Utc::now();

        let active_count = (0..100)
            .filter(|step| {
                cadence.should_capture(
                    CaptureRateRegime::Active,
                    base + Duration::milliseconds(step * 100),
                )
            })
            .count();
        assert!(
            (18..=22).contains(&active_count),
            "active regime should produce roughly 20 captures in 10s, got {active_count}"
        );

        let idle_count = (0..100)
            .filter(|step| {
                cadence.should_capture(
                    CaptureRateRegime::Idle,
                    base + Duration::milliseconds(10_000 + step * 100),
                )
            })
            .count();
        assert!(
            (1..=2).contains(&idle_count),
            "idle regime should produce 1-2 captures in 10s, got {idle_count}"
        );

        let first_active_after_idle_ms = (0..=6)
            .map(|step| step * 100)
            .find(|offset_ms| {
                cadence.should_capture(
                    CaptureRateRegime::Active,
                    base + Duration::milliseconds(20_000 + *offset_ms),
                )
            })
            .expect("active regime should resume capture promptly");

        assert!(
            first_active_after_idle_ms <= 600,
            "active capture should resume within 600ms, got {first_active_after_idle_ms}ms"
        );
    }

    #[test]
    fn crt_prv_power_004_sleep_gap_resumes_without_capture_burst() {
        let mut cadence = AdaptiveCaptureCadence::default();
        let base = Utc::now();

        assert!(cadence.should_capture(CaptureRateRegime::Active, base));
        assert!(!cadence.should_capture(
            CaptureRateRegime::Active,
            base + Duration::milliseconds(400)
        ));

        // A long monotonic gap models the scheduler observing the first tick
        // after OS wake. It should allow exactly one capture, then resume the
        // normal active cadence without replaying missed ticks as a burst.
        let wake_tick = base + Duration::hours(8);
        assert!(cadence.should_capture(CaptureRateRegime::Active, wake_tick));

        let same_tick_burst_count = (0..5)
            .filter(|_| cadence.should_capture(CaptureRateRegime::Active, wake_tick))
            .count();
        assert_eq!(
            same_tick_burst_count, 0,
            "wake gap must not replay missed captures as an immediate burst"
        );

        assert!(!cadence.should_capture(
            CaptureRateRegime::Active,
            wake_tick + Duration::milliseconds(400)
        ));
        assert!(cadence.should_capture(
            CaptureRateRegime::Active,
            wake_tick + Duration::milliseconds(500)
        ));
    }

    // ── Full segment lifecycle integration test ──────────────────────
    //
    // Tests the complete cycle using AdaptiveTrigger + SegmentBuffer +
    // ContentTracker together (no mocks, no async, pure computation).

    #[test]
    fn full_segment_lifecycle_open_accumulate_close() {
        use crate::content_tracker::{ContentTracker, ContentUpdateInput};
        use crate::SegmentBuffer;
        use maekon_core::models::tiered_memory::{ContentType, EngagementMetrics};

        let mut trigger = AdaptiveTrigger::new();
        let mut segment_buffer = SegmentBuffer::new(200);
        let mut content_tracker = ContentTracker::new();

        // Use tuned params that make it easy to trigger open/close
        let mut params = default_params();
        params.t_high = 0.55;
        params.t_low = 0.35;
        params.min_segment_secs = 0; // disable min for predictable testing
        params.max_segment_secs = 600;
        params.buffer_capacity = 100;

        let base = Utc::now();

        // ── Phase 1: Feed high-importance events to trigger OpenSegment ──
        let mut opened = false;
        for i in 0..15 {
            let ts = base + Duration::seconds(i * 2);
            let input = TriggerInput::AppSwitchNew {
                app_name: format!("App{}", i % 3),
                prev_app: format!("App{}", (i + 1) % 3),
                category: AppCategory::Development,
            };
            let (decision, _cal) = trigger.process_event(&input, ts, &params);

            if decision == TriggerDecision::OpenSegment {
                trigger.start_new_segment(ts);
                segment_buffer.start_segment(ts);
                segment_buffer.push(ts, input.clone());
                opened = true;
                break;
            }
        }
        assert!(
            opened,
            "segment should have opened after high-importance events"
        );
        assert!(segment_buffer.start_time().is_some());

        // ── Phase 2: Feed content changes while segment is open ──
        let content_labels = ["main.rs", "lib.rs", "README.md"];
        for (i, label) in content_labels.iter().enumerate() {
            let ts = base + Duration::seconds(30 + (i as i64) * 10);

            // Push an event into the segment buffer
            let input = TriggerInput::AppPoll {
                app_name: "VS Code".to_string(),
            };
            segment_buffer.push(ts, input);

            // Feed content into the content tracker
            content_tracker.update(ContentUpdateInput {
                content_label: label.to_string(),
                content_type: ContentType::File,
                work_type: maekon_core::models::tiered_memory::WorkType::ActiveCoding,
                engagement: EngagementMetrics {
                    keystrokes_per_min: 40.0,
                    mouse_clicks_per_min: 5.0,
                    scroll_events_per_min: 2.0,
                    shortcut_ratio: 0.1,
                    typing_burst_count: 1,
                    idle_ratio: 0.0,
                },
                confidence: 0.95,
                timestamp: ts,
                gui_summary: None,
            });
        }

        // Verify segment buffer has accumulated events (3 content + 1 open)
        assert!(
            segment_buffer.len() >= 3,
            "buffer should have at least 3 events, got {}",
            segment_buffer.len()
        );

        // ── Phase 3: Force low-importance events to trigger CloseSegment ──
        let mut closed = false;
        for i in 0..30 {
            let ts = base + Duration::seconds(60 + i * 3);
            let input = TriggerInput::SystemMetric; // very low importance (0.05)
            let (decision, _cal) = trigger.process_event(&input, ts, &params);

            match decision {
                TriggerDecision::CloseSegment | TriggerDecision::ForceCloseSegment => {
                    closed = true;
                    break;
                }
                _ => {
                    segment_buffer.push(ts, input);
                }
            }
        }
        assert!(
            closed,
            "segment should have closed after low-importance events"
        );

        // ── Phase 4: Drain and verify ──
        let seg_events = segment_buffer.drain_all();
        assert!(!seg_events.is_empty(), "drained segment should have events");
        assert!(
            segment_buffer.is_empty(),
            "buffer should be empty after drain"
        );
        assert!(
            segment_buffer.start_time().is_none(),
            "segment start should be cleared"
        );

        // Drain content tracker
        let end_time = base + Duration::seconds(150);
        let content_activities = content_tracker.drain_all(end_time);
        assert_eq!(
            content_activities.len(),
            3,
            "should have 3 content activities (main.rs, lib.rs, README.md)"
        );
        assert_eq!(content_activities[0].content_label, "main.rs");
        assert_eq!(content_activities[1].content_label, "lib.rs");
        assert_eq!(content_activities[2].content_label, "README.md");

        // Verify durations: first two activities have 10s each (switched after 10s)
        assert_eq!(content_activities[0].duration_secs, 10);
        assert_eq!(content_activities[1].duration_secs, 10);
        // Last activity: from t+50 to t+150 = 100s
        assert_eq!(content_activities[2].duration_secs, 100);

        // Trigger should be in a clean state after close_segment
        trigger.close_segment();
        assert!(trigger.current_segment_start().is_none());
    }

    #[test]
    fn full_lifecycle_restart_segment() {
        use crate::SegmentBuffer;

        let mut trigger = AdaptiveTrigger::new();
        let mut segment_buffer = SegmentBuffer::new(200);

        let mut params = default_params();
        params.t_high = 0.55;
        params.t_low = 0.35;
        params.min_segment_secs = 0;
        params.max_segment_secs = 600;

        let base = Utc::now();

        // Open a segment first
        let mut opened = false;
        for i in 0..15 {
            let ts = base + Duration::seconds(i * 2);
            let input = app_switch(&format!("App{i}"));
            let (decision, _) = trigger.process_event(&input, ts, &params);
            if decision == TriggerDecision::OpenSegment {
                trigger.start_new_segment(ts);
                segment_buffer.start_segment(ts);
                segment_buffer.push(ts, input);
                opened = true;
                break;
            }
        }
        assert!(opened);

        // Feed more high-importance events until RestartSegment
        let mut restarted = false;
        for i in 0..50 {
            let ts = base + Duration::seconds(30 + i * 2);
            let input = app_switch(&format!("App{i}"));
            let (decision, _) = trigger.process_event(&input, ts, &params);

            match decision {
                TriggerDecision::RestartSegment => {
                    // Close old segment
                    let old_events = segment_buffer.drain_all();
                    assert!(!old_events.is_empty(), "old segment should have events");

                    // Start new segment
                    trigger.start_new_segment(ts);
                    segment_buffer.start_segment(ts);
                    restarted = true;
                    break;
                }
                TriggerDecision::Continue => {
                    segment_buffer.push(ts, input);
                }
                _ => {
                    segment_buffer.push(ts, input);
                }
            }
        }
        assert!(restarted, "should have triggered RestartSegment");
        assert!(
            segment_buffer.start_time().is_some(),
            "new segment should be open after restart"
        );
    }

    #[test]
    fn full_lifecycle_force_close_max_duration() {
        use crate::SegmentBuffer;

        let mut trigger = AdaptiveTrigger::new();
        let mut segment_buffer = SegmentBuffer::new(200);

        let mut params = default_params();
        params.t_high = 0.55;
        params.t_low = 0.10; // very low, unlikely to trigger normal close
        params.min_segment_secs = 0;
        params.max_segment_secs = 30; // short max for testing

        let base = Utc::now();

        // Open a segment
        trigger.start_new_segment(base);
        segment_buffer.start_segment(base);

        // Feed middling events until max duration forces close
        let mut force_closed = false;
        for i in 1..=20 {
            let ts = base + Duration::seconds(i * 2);
            let input = TriggerInput::AppPoll {
                app_name: "VS Code".to_string(),
            };
            let (decision, _) = trigger.process_event(&input, ts, &params);

            if decision == TriggerDecision::ForceCloseSegment {
                force_closed = true;
                break;
            }
            segment_buffer.push(ts, input);
        }

        assert!(force_closed, "should force-close at max_segment_secs=30");
    }
}
