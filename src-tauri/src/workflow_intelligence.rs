//! Workflow Intelligence.

use chrono::{DateTime, Utc};
use oneshim_core::models::work_session::AppCategory;
use std::collections::{HashMap, HashSet};

const MAX_SEGMENT_STEPS: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GuiIntent {
    Communicate,
    Compose,
    Review,
    Execute,
    Analyze,
    Explore,
    Unknown,
}

impl GuiIntent {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Communicate => "communicate",
            Self::Compose => "compose",
            Self::Review => "review",
            Self::Execute => "execute",
            Self::Analyze => "analyze",
            Self::Explore => "explore",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone)]
struct AppUsageStats {
    category: AppCategory,
    total_secs: u64,
    switch_count: u32,
    last_seen_at: DateTime<Utc>,
    relevance: f32,
}

#[derive(Debug, Clone)]
struct WorkflowStep {
    app_name: String,
    category: AppCategory,
    intent: GuiIntent,
    relevance: f32,
}

#[derive(Debug, Clone)]
struct WorkflowSegment {
    started_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
    steps: Vec<WorkflowStep>,
}

#[derive(Debug, Clone)]
struct PlaybookStats {
    occurrences: u32,
    total_duration_secs: u64,
    last_seen_at: DateTime<Utc>,
    representative_path: String,
    representative_intents: String,
}

#[derive(Debug, Clone)]
pub struct PlaybookSignal {
    pub description: String,
    pub confidence: f32,
}

#[derive(Debug, Default)]
pub struct WorkflowIntelligence {
    usage: HashMap<String, AppUsageStats>,
    active_segment: Option<WorkflowSegment>,
    playbooks: HashMap<String, PlaybookStats>,
}

impl WorkflowIntelligence {
    pub fn update_usage(
        &mut self,
        app_name: &str,
        category: AppCategory,
        duration_secs: u64,
        now: DateTime<Utc>,
    ) -> f32 {
        if app_name.trim().is_empty() {
            return 0.0;
        }

        let entry = self
            .usage
            .entry(app_name.to_string())
            .or_insert_with(|| AppUsageStats {
                category,
                total_secs: 0,
                switch_count: 0,
                last_seen_at: now,
                relevance: base_category_relevance(category),
            });

        entry.category = category;
        entry.total_secs = entry.total_secs.saturating_add(duration_secs);
        entry.switch_count = entry.switch_count.saturating_add(1);
        entry.last_seen_at = now;
        entry.relevance = compute_relevance(
            category,
            entry.total_secs,
            entry.switch_count,
            entry.last_seen_at,
            now,
        );
        entry.relevance
    }

    pub fn touch_app(&mut self, app_name: &str, category: AppCategory, now: DateTime<Utc>) -> f32 {
        if app_name.trim().is_empty() {
            return 0.0;
        }

        let entry = self
            .usage
            .entry(app_name.to_string())
            .or_insert_with(|| AppUsageStats {
                category,
                total_secs: 0,
                switch_count: 0,
                last_seen_at: now,
                relevance: base_category_relevance(category),
            });

        entry.category = category;
        entry.switch_count = entry.switch_count.saturating_add(1);
        entry.last_seen_at = now;
        entry.relevance = compute_relevance(
            category,
            entry.total_secs,
            entry.switch_count,
            entry.last_seen_at,
            now,
        );
        entry.relevance
    }

    #[allow(clippy::too_many_arguments)]
    pub fn advance_workflow(
        &mut self,
        app_name: &str,
        category: AppCategory,
        window_title: &str,
        ocr_hint: Option<&str>,
        now: DateTime<Utc>,
        min_relevance: f32,
        split_idle_secs: u64,
    ) -> Option<PlaybookSignal> {
        let relevance = self
            .usage
            .get(app_name)
            .map(|s| s.relevance)
            .unwrap_or_else(|| base_category_relevance(category));
        let intent = infer_gui_intent(category, window_title, ocr_hint);
        let step = WorkflowStep {
            app_name: app_name.to_string(),
            category,
            intent,
            relevance,
        };

        let should_split = self
            .active_segment
            .as_ref()
            .map(|segment| {
                should_split_segment(segment, &step, now, min_relevance, split_idle_secs)
            })
            .unwrap_or(false);

        if should_split {
            let completed = self.active_segment.take();
            self.active_segment = Some(WorkflowSegment {
                started_at: now,
                last_seen_at: now,
                steps: vec![step],
            });
            return completed
                .and_then(|segment| self.register_segment(segment, min_relevance, now));
        }

        match self.active_segment.as_mut() {
            Some(segment) => {
                segment.last_seen_at = now;
                let should_append = segment
                    .steps
                    .last()
                    .map(|last| last.app_name != app_name || last.intent != intent)
                    .unwrap_or(true);
                if should_append {
                    segment.steps.push(step);
                    if segment.steps.len() > MAX_SEGMENT_STEPS {
                        segment.steps.remove(0);
                    }
                }
            }
            None => {
                self.active_segment = Some(WorkflowSegment {
                    started_at: now,
                    last_seen_at: now,
                    steps: vec![step],
                });
            }
        }

        None
    }

    pub fn flush_stale_segment(
        &mut self,
        now: DateTime<Utc>,
        min_relevance: f32,
        stale_secs: u64,
    ) -> Option<PlaybookSignal> {
        let stale = self
            .active_segment
            .as_ref()
            .map(|segment| (now - segment.last_seen_at).num_seconds().max(0) as u64 >= stale_secs)
            .unwrap_or(false);

        if !stale {
            return None;
        }

        let segment = self.active_segment.take()?;
        self.register_segment(segment, min_relevance, now)
    }
}

fn should_split_segment(
    segment: &WorkflowSegment,
    incoming: &WorkflowStep,
    now: DateTime<Utc>,
    min_relevance: f32,
    split_idle_secs: u64,
) -> bool {
    let idle_secs = (now - segment.last_seen_at).num_seconds().max(0) as u64;
    if idle_secs >= split_idle_secs {
        return true;
    }

    if segment.steps.len() >= MAX_SEGMENT_STEPS {
        return true;
    }

    let last = match segment.steps.last() {
        Some(last) => last,
        None => return false,
    };

    if last.category.is_deep_work()
        && !incoming.category.is_deep_work()
        && incoming.relevance < min_relevance
    {
        return true;
    }

    last.intent != GuiIntent::Communicate
        && incoming.intent == GuiIntent::Communicate
        && incoming.relevance < min_relevance
}

impl WorkflowIntelligence {
    fn register_segment(
        &mut self,
        segment: WorkflowSegment,
        min_relevance: f32,
        now: DateTime<Utc>,
    ) -> Option<PlaybookSignal> {
        let duration_secs = (segment.last_seen_at - segment.started_at)
            .num_seconds()
            .max(1) as u64;

        let filtered: Vec<&WorkflowStep> = segment
            .steps
            .iter()
            .filter(|s| s.relevance >= min_relevance)
            .filter(|s| !matches!(s.category, AppCategory::Media | AppCategory::System))
            .collect();

        if filtered.len() < 3 {
            return None;
        }

        let mut tokens: Vec<String> = Vec::new();
        let mut path_labels: Vec<String> = Vec::new();
        let mut intents_order: Vec<GuiIntent> = Vec::new();
        let mut last_token = String::new();

        for step in filtered {
            let app = normalize_app(&step.app_name);
            let token = format!("{}:{}", app, step.intent.as_str());
            if token == last_token {
                continue;
            }
            last_token = token.clone();
            tokens.push(token);
            path_labels.push(step.app_name.clone());
            intents_order.push(step.intent);
        }

        if tokens.len() < 3 {
            return None;
        }

        let key = tokens.join("|");
        let mut unique_intents: Vec<&str> = Vec::new();
        let mut seen_intents = HashSet::new();
        for intent in &intents_order {
            let label = intent.as_str();
            if seen_intents.insert(label) {
                unique_intents.push(label);
            }
        }

        let app_path = join_limited(&path_labels, 4);
        let intents_label = unique_intents.join(" -> ");

        let entry = self.playbooks.entry(key).or_insert_with(|| PlaybookStats {
            occurrences: 0,
            total_duration_secs: 0,
            last_seen_at: now,
            representative_path: app_path.clone(),
            representative_intents: intents_label.clone(),
        });

        entry.occurrences = entry.occurrences.saturating_add(1);
        entry.total_duration_secs = entry.total_duration_secs.saturating_add(duration_secs);
        entry.last_seen_at = now;
        entry.representative_path = app_path;
        entry.representative_intents = intents_label;

        let emit = entry.occurrences == 3 || (entry.occurrences > 3 && entry.occurrences % 5 == 0);
        if !emit {
            return None;
        }

        let avg_duration_mins = (entry.total_duration_secs / entry.occurrences as u64) / 60;
        let confidence = (0.4
            + (entry.occurrences as f32 / 6.0).min(0.35)
            + (tokens.len() as f32 / 8.0).min(0.2))
        .clamp(0.0, 0.95);

        Some(PlaybookSignal {
            description: format!(
                "반복 업무 흐름 detection ({}회): {} / intent: {} / 평균 {}분",
                entry.occurrences,
                entry.representative_path,
                entry.representative_intents,
                avg_duration_mins.max(1)
            ),
            confidence,
        })
    }
}

fn base_category_relevance(category: AppCategory) -> f32 {
    match category {
        AppCategory::Development => 0.95,
        AppCategory::Documentation => 0.9,
        AppCategory::Design => 0.85,
        AppCategory::Browser => 0.55,
        AppCategory::Other => 0.4,
        AppCategory::Communication => 0.22,
        AppCategory::System => 0.15,
        AppCategory::Media => 0.05,
    }
}

fn compute_relevance(
    category: AppCategory,
    total_secs: u64,
    switch_count: u32,
    last_seen_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> f32 {
    let base = base_category_relevance(category);
    let duration_signal = (total_secs as f32 / 5400.0).min(1.0) * 0.25;
    let frequency_signal = (switch_count as f32 / 20.0).min(1.0) * 0.15;

    let recency_secs = (now - last_seen_at).num_seconds().max(0) as u64;
    let recency_signal = if recency_secs <= 300 {
        0.1
    } else if recency_secs <= 1800 {
        0.05
    } else {
        0.0
    };

    let mut score = (base * 0.65) + duration_signal + frequency_signal + recency_signal;
    if matches!(category, AppCategory::Communication | AppCategory::Media) {
        score *= 0.65;
    }
    score.clamp(0.0, 1.0)
}

fn normalize_app(app_name: &str) -> String {
    app_name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect::<String>()
}

fn join_limited(items: &[String], max: usize) -> String {
    items
        .iter()
        .take(max)
        .cloned()
        .collect::<Vec<_>>()
        .join(" -> ")
}

fn contains_any(text: &str, keywords: &[&str]) -> bool {
    keywords.iter().any(|k| text.contains(k))
}

fn infer_gui_intent(
    category: AppCategory,
    window_title: &str,
    ocr_hint: Option<&str>,
) -> GuiIntent {
    if category.is_communication() {
        return GuiIntent::Communicate;
    }

    let mut context = window_title.to_lowercase();
    if let Some(ocr) = ocr_hint {
        let compact = ocr
            .split_whitespace()
            .take(20)
            .collect::<Vec<_>>()
            .join(" ");
        if !compact.is_empty() {
            context.push(' ');
            context.push_str(&compact.to_lowercase());
        }
    }

    if contains_any(
        &context,
        &[
            "run",
            "build",
            "deploy",
            "submit",
            "publish",
            "commit",
            "merge",
            "release",
            "execution",
            "배포",
            "제출",
        ],
    ) {
        return GuiIntent::Execute;
    }

    if contains_any(
        &context,
        &[
            "review",
            "diff",
            "pull request",
            "approve",
            "comment",
            "read",
            "preview",
            "검토",
            "리뷰",
            "승인",
        ],
    ) {
        return GuiIntent::Review;
    }

    if contains_any(
        &context,
        &[
            "compose", "write", "draft", "edit", "reply", "new", "작성", "편집", "답장", "create",
        ],
    ) {
        return GuiIntent::Compose;
    }

    if contains_any(
        &context,
        &[
            "dashboard",
            "metrics",
            "chart",
            "analytics",
            "report",
            "error",
            "failed",
            "분석",
            "통계",
            "리port",
            "error",
        ],
    ) {
        return GuiIntent::Analyze;
    }

    if contains_any(
        &context,
        &[
            "search",
            "browse",
            "find",
            "documentation",
            "docs",
            "help",
            "검색",
            "탐색",
            "문서",
        ],
    ) {
        return GuiIntent::Explore;
    }

    match category {
        AppCategory::Development | AppCategory::Documentation | AppCategory::Design => {
            GuiIntent::Compose
        }
        AppCategory::Browser => GuiIntent::Explore,
        _ => GuiIntent::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn communication_apps_are_downweighted() {
        let now = Utc::now();
        let comm = compute_relevance(AppCategory::Communication, 7200, 20, now, now);
        let dev = compute_relevance(AppCategory::Development, 7200, 20, now, now);
        assert!(dev > comm);
    }

    #[test]
    fn infer_intent_from_keywords() {
        let intent = infer_gui_intent(
            AppCategory::Development,
            "Pull Request Review",
            Some("approve changes"),
        );
        assert_eq!(intent, GuiIntent::Review);
    }

    #[test]
    fn emits_playbook_signal_after_repetition() {
        let mut wf = WorkflowIntelligence::default();
        let base = Utc::now();
        let mut signal_count = 0u32;

        for i in 0..3 {
            let t0 = base + chrono::Duration::minutes((i * 30) as i64);
            let t1 = t0 + chrono::Duration::minutes(4);
            let t2 = t1 + chrono::Duration::minutes(3);
            let t3 = t2 + chrono::Duration::minutes(2);
            let t_end = t3 + chrono::Duration::minutes(15);

            wf.update_usage("Visual Studio Code", AppCategory::Development, 600, t0);
            wf.touch_app("Visual Studio Code", AppCategory::Development, t0);
            let _ = wf.advance_workflow(
                "Visual Studio Code",
                AppCategory::Development,
                "Implement feature",
                Some("new file edit"),
                t0,
                0.3,
                300,
            );

            wf.update_usage("Google Chrome", AppCategory::Browser, 240, t1);
            wf.touch_app("Google Chrome", AppCategory::Browser, t1);
            let _ = wf.advance_workflow(
                "Google Chrome",
                AppCategory::Browser,
                "PR review",
                Some("review comment approve"),
                t1,
                0.3,
                300,
            );

            wf.update_usage("Terminal", AppCategory::Development, 180, t2);
            wf.touch_app("Terminal", AppCategory::Development, t2);
            let _ = wf.advance_workflow(
                "Terminal",
                AppCategory::Development,
                "cargo test",
                Some("run test"),
                t2,
                0.3,
                300,
            );

            let signal = wf.flush_stale_segment(t_end, 0.3, 600);
            if signal.is_some() {
                signal_count += 1;
            }
        }

        assert!(signal_count >= 1);
    }
}
