use oneshim_core::models::frame::FrameMetadata;
use tracing::debug;

#[derive(Debug, Clone)]
pub struct TimelineEntry {
    pub id: u64,
    pub metadata: FrameMetadata,
    pub has_image: bool,
}

#[derive(Debug, Clone, Default)]
pub struct TimelineFilter {
    pub app_name: Option<String>,
    pub min_importance: Option<f32>,
    pub text_search: Option<String>,
    pub limit: usize,
}

impl TimelineFilter {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            ..Default::default()
        }
    }

    pub fn with_app(mut self, app_name: &str) -> Self {
        self.app_name = Some(app_name.to_string());
        self
    }

    pub fn with_min_importance(mut self, importance: f32) -> Self {
        self.min_importance = Some(importance);
        self
    }

    pub fn with_text(mut self, text: &str) -> Self {
        self.text_search = Some(text.to_string());
        self
    }

    pub fn matches(&self, meta: &FrameMetadata) -> bool {
        if let Some(app) = &self.app_name {
            if !meta.app_name.contains(app) {
                return false;
            }
        }
        if let Some(min) = self.min_importance {
            if meta.importance < min {
                return false;
            }
        }
        if let Some(text) = &self.text_search {
            let lower = text.to_lowercase();
            if !meta.window_title.to_lowercase().contains(&lower)
                && !meta.app_name.to_lowercase().contains(&lower)
            {
                return false;
            }
        }
        true
    }
}

pub struct Timeline {
    entries: Vec<TimelineEntry>,
    max_entries: usize,
    next_id: u64,
}

impl Timeline {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_entries,
            next_id: 1,
        }
    }

    pub fn add_frame(&mut self, metadata: FrameMetadata, has_image: bool) -> u64 {
        let id = self.next_id;
        self.next_id += 1;

        self.entries.push(TimelineEntry {
            id,
            metadata,
            has_image,
        });

        if self.entries.len() > self.max_entries {
            self.entries.remove(0);
        }

        debug!("frame add: ID {id}");
        id
    }

    pub fn query(&self, filter: &TimelineFilter) -> Vec<&TimelineEntry> {
        self.entries
            .iter()
            .rev()
            .filter(|e| filter.matches(&e.metadata))
            .take(filter.limit)
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_metadata(app: &str, title: &str, importance: f32) -> FrameMetadata {
        FrameMetadata {
            timestamp: Utc::now(),
            trigger_type: "WindowChange".to_string(),
            app_name: app.to_string(),
            window_title: title.to_string(),
            resolution: (1920, 1080),
            importance,
        }
    }

    #[test]
    fn add_and_query() {
        let mut timeline = Timeline::new(100);
        timeline.add_frame(make_metadata("Code", "main.rs", 0.8), true);
        timeline.add_frame(make_metadata("Firefox", "Google", 0.3), false);

        let filter = TimelineFilter::new(10);
        assert_eq!(timeline.query(&filter).len(), 2);
    }

    #[test]
    fn filter_by_app() {
        let mut timeline = Timeline::new(100);
        timeline.add_frame(make_metadata("Code", "main.rs", 0.8), true);
        timeline.add_frame(make_metadata("Firefox", "Google", 0.3), false);

        let filter = TimelineFilter::new(10).with_app("Code");
        assert_eq!(timeline.query(&filter).len(), 1);
    }

    #[test]
    fn filter_by_importance() {
        let mut timeline = Timeline::new(100);
        timeline.add_frame(make_metadata("Code", "main.rs", 0.8), true);
        timeline.add_frame(make_metadata("Firefox", "Google", 0.3), false);

        let filter = TimelineFilter::new(10).with_min_importance(0.5);
        assert_eq!(timeline.query(&filter).len(), 1);
    }

    #[test]
    fn max_entries_eviction() {
        let mut timeline = Timeline::new(2);
        timeline.add_frame(make_metadata("A", "a", 0.5), false);
        timeline.add_frame(make_metadata("B", "b", 0.5), false);
        timeline.add_frame(make_metadata("C", "c", 0.5), false);

        assert_eq!(timeline.len(), 2);
    }
}
