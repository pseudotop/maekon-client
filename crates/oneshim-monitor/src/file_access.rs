use oneshim_core::config::{FileAccessConfig, PiiFilterLevel};
use oneshim_core::ports::pii_sanitizer::PiiSanitizer;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

// Event types are canonical in oneshim-core; re-exported here.
pub use oneshim_core::models::event::{FileAccessEvent, FileEventType};

pub struct FileAccessFilter {
    config: FileAccessConfig,
    events_this_minute: Arc<AtomicU32>,
    /// D5 iter-11: sanitize file paths before FileAccessEvent construction.
    /// Filenames can contain PII (`Resume_JohnDoe.pdf`, `2024_TaxReturn.xlsx`).
    /// The `/Users/<name>/` prefix is stripped by `to_relative_path`, but the
    /// filename itself is not sanitized. Apply at event boundary.
    pii_sanitizer: Option<Arc<dyn PiiSanitizer>>,
    pii_level: PiiFilterLevel,
}

impl FileAccessFilter {
    pub fn new(config: FileAccessConfig) -> Self {
        Self {
            config,
            events_this_minute: Arc::new(AtomicU32::new(0)),
            pii_sanitizer: None,
            pii_level: PiiFilterLevel::Standard,
        }
    }

    /// D5 iter-11: attach PII sanitizer for path sanitization.
    pub fn with_pii_sanitizer(
        mut self,
        sanitizer: Arc<dyn PiiSanitizer>,
        level: PiiFilterLevel,
    ) -> Self {
        self.pii_sanitizer = Some(sanitizer);
        self.pii_level = level;
        self
    }

    pub fn should_collect(&self, path: &Path) -> bool {
        if !self.config.enabled {
            return false;
        }

        let count = self.events_this_minute.load(Ordering::Relaxed);
        if count >= self.config.max_events_per_minute {
            return false;
        }

        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            let ext_with_dot = format!(".{ext}");
            if self.config.excluded_extensions.contains(&ext_with_dot) {
                return false;
            }
        }

        if !self.config.monitored_folders.is_empty() {
            let in_whitelist = self
                .config
                .monitored_folders
                .iter()
                .any(|folder| path.starts_with(folder));
            if !in_whitelist {
                return false;
            }
        }

        true
    }

    pub fn record_event(&self) {
        self.events_this_minute.fetch_add(1, Ordering::Relaxed);
    }

    pub fn reset_minute_counter(&self) {
        self.events_this_minute.store(0, Ordering::Relaxed);
    }

    pub fn to_relative_path(&self, absolute_path: &Path) -> PathBuf {
        for folder in &self.config.monitored_folders {
            if let Ok(rel) = absolute_path.strip_prefix(folder) {
                return rel.to_path_buf();
            }
        }
        absolute_path
            .file_name()
            .map(PathBuf::from)
            .unwrap_or_else(|| absolute_path.to_path_buf())
    }

    pub fn create_event(
        &self,
        absolute_path: &Path,
        event_type: FileEventType,
    ) -> Option<FileAccessEvent> {
        if !self.should_collect(absolute_path) {
            return None;
        }

        self.record_event();

        // D5 iter-11: sanitize path and extension before emitting event.
        let raw_path = self.to_relative_path(absolute_path);
        let relative_path = match &self.pii_sanitizer {
            Some(s) => {
                let raw_str = raw_path.to_string_lossy();
                PathBuf::from(s.sanitize_text(&raw_str, self.pii_level))
            }
            None => raw_path,
        };
        let extension = absolute_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_string());

        Some(FileAccessEvent {
            timestamp: chrono::Utc::now(),
            relative_path,
            event_type,
            extension,
        })
    }
}

/// Maximum number of files tracked per poll to bound memory usage.
const MAX_TRACKED_FILES: usize = 10_000;

/// Polling-based file access watcher.
///
/// Scans monitored directories for modification time changes each poll cycle.
/// Tracks files that were created, modified, or deleted since the last scan.
/// Thread-safe — designed to be wrapped in `Arc` and called from an async loop.
///
/// # Design notes
///
/// This uses polling (stat-based) detection rather than OS-native file watching
/// (e.g., `notify` crate / inotify / FSEvents) because:
/// 1. The `notify` crate is not in the workspace dependency set.
/// 2. Polling at the scheduler cadence (30-60s) is sufficient for our
///    use case (activity tracking, not real-time sync).
/// 3. Simpler error handling — no watcher thread management.
///
/// When `notify` is added to the workspace, replace `poll_changes()` with an
/// event-driven receiver.
pub struct FileAccessWatcher {
    filter: FileAccessFilter,
    /// Last-seen modification times for files in monitored directories.
    /// Bounded to `max_tracked_files` entries per poll cycle.
    file_mtimes: Mutex<HashMap<PathBuf, SystemTime>>,
    /// Cumulative count of file changes since last `take_modified_count()`.
    modified_count: AtomicU32,
    /// Per-poll cap on the number of tracked files.
    max_tracked_files: usize,
}

impl FileAccessWatcher {
    pub fn new(config: FileAccessConfig) -> Self {
        Self::new_with_limit(config, MAX_TRACKED_FILES)
    }

    /// Construct a watcher with a custom tracked-file cap.
    ///
    /// Used by tests to exercise the truncation path without having to
    /// create 10_000+ files.
    pub(crate) fn new_with_limit(config: FileAccessConfig, max_tracked_files: usize) -> Self {
        Self {
            filter: FileAccessFilter::new(config),
            file_mtimes: Mutex::new(HashMap::new()),
            modified_count: AtomicU32::new(0),
            max_tracked_files,
        }
    }

    /// Scan monitored directories and detect file changes since the last poll.
    ///
    /// Returns a list of `FileAccessEvent` for created and modified files.
    /// Deleted files are also detected (present in previous scan, absent now).
    ///
    /// Only scans top-level files in each monitored folder (non-recursive) to
    /// keep the scan lightweight. Deep scanning can be enabled by walking
    /// subdirectories if needed.
    // P2 PR-A: `file_mtimes` mutex is held across the directory scan + state
    // update as the atomicity guard for "see-current-state + detect-changes +
    // record-new-state". Tightening would race with concurrent poll calls.
    #[allow(clippy::significant_drop_tightening)]
    pub fn poll_changes(&self) -> Vec<FileAccessEvent> {
        let mut events = Vec::new();

        let mut mtimes = match self.file_mtimes.lock() {
            Ok(m) => m,
            Err(_) => return events,
        };

        let monitored_folders: Vec<PathBuf> = self.filter.config.monitored_folders.clone();

        if monitored_folders.is_empty() || !self.filter.config.enabled {
            return events;
        }

        let mut current_files: HashMap<PathBuf, SystemTime> = HashMap::new();

        for folder in &monitored_folders {
            let entries = match std::fs::read_dir(folder) {
                Ok(e) => e,
                Err(_) => continue,
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }

                let mtime = match entry.metadata().and_then(|m| m.modified()) {
                    Ok(t) => t,
                    Err(_) => continue,
                };

                if !self.filter.should_collect(&path) {
                    continue;
                }

                if current_files.len() >= self.max_tracked_files {
                    tracing::warn!(
                        folder = %folder.display(),
                        limit = self.max_tracked_files,
                        "File access polling truncated: tracked-file cap reached"
                    );
                    break;
                }

                current_files.insert(path.clone(), mtime);

                match mtimes.get(&path) {
                    None => {
                        // New file — created since last scan
                        if let Some(event) = self.filter.create_event(&path, FileEventType::Created)
                        {
                            events.push(event);
                            self.modified_count.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Some(prev_mtime) if *prev_mtime != mtime => {
                        // Existing file with changed mtime — modified
                        if let Some(event) =
                            self.filter.create_event(&path, FileEventType::Modified)
                        {
                            events.push(event);
                            self.modified_count.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    _ => {
                        // No change
                    }
                }
            }
        }

        // Detect deleted files (in previous scan but not in current)
        for old_path in mtimes.keys() {
            if !current_files.contains_key(old_path) {
                if let Some(event) = self.filter.create_event(old_path, FileEventType::Deleted) {
                    events.push(event);
                    self.modified_count.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        // Replace the stored mtimes with the current scan
        *mtimes = current_files;

        // Reset rate limiter for next minute
        self.filter.reset_minute_counter();

        events
    }

    /// Return the number of file modifications since the last call and reset
    /// the counter to zero. Designed for periodic snapshot collection.
    pub fn take_modified_count(&self) -> u32 {
        self.modified_count.swap(0, Ordering::Relaxed)
    }

    /// Access the underlying filter for manual event creation.
    pub fn filter(&self) -> &FileAccessFilter {
        &self.filter
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> FileAccessConfig {
        FileAccessConfig {
            enabled: true,
            monitored_folders: vec![PathBuf::from("/home/user/projects")],
            excluded_extensions: vec![".tmp".to_string(), ".log".to_string()],
            max_events_per_minute: 10,
        }
    }

    #[test]
    fn filter_disabled() {
        let mut config = test_config();
        config.enabled = false;
        let filter = FileAccessFilter::new(config);
        assert!(!filter.should_collect(&PathBuf::from("/home/user/projects/file.rs")));
    }

    #[test]
    fn filter_excluded_extension() {
        let filter = FileAccessFilter::new(test_config());
        assert!(!filter.should_collect(&PathBuf::from("/home/user/projects/debug.tmp")));
        assert!(!filter.should_collect(&PathBuf::from("/home/user/projects/app.log")));
    }

    #[test]
    fn filter_outside_whitelist() {
        let filter = FileAccessFilter::new(test_config());
        assert!(!filter.should_collect(&PathBuf::from("/home/user/downloads/file.rs")));
    }

    #[test]
    fn filter_allows_valid_path() {
        let filter = FileAccessFilter::new(test_config());
        assert!(filter.should_collect(&PathBuf::from("/home/user/projects/src/main.rs")));
    }

    #[test]
    fn rate_limit() {
        let mut config = test_config();
        config.max_events_per_minute = 2;
        let filter = FileAccessFilter::new(config);

        let path = PathBuf::from("/home/user/projects/file.rs");
        assert!(filter.should_collect(&path));
        filter.record_event();
        assert!(filter.should_collect(&path));
        filter.record_event();
        assert!(!filter.should_collect(&path));

        filter.reset_minute_counter();
        assert!(filter.should_collect(&path));
    }

    #[test]
    fn relative_path_extraction() {
        let filter = FileAccessFilter::new(test_config());
        let rel = filter.to_relative_path(&PathBuf::from("/home/user/projects/src/main.rs"));
        assert_eq!(rel, PathBuf::from("src/main.rs"));
    }

    #[test]
    fn create_event_success() {
        let filter = FileAccessFilter::new(test_config());
        let event = filter.create_event(
            &PathBuf::from("/home/user/projects/src/lib.rs"),
            FileEventType::Modified,
        );
        assert!(event.is_some());
        let evt = event.unwrap();
        assert_eq!(evt.event_type, FileEventType::Modified);
        assert_eq!(evt.extension, Some("rs".to_string()));
    }

    #[test]
    fn watcher_disabled_returns_empty() {
        let mut config = test_config();
        config.enabled = false;
        let watcher = FileAccessWatcher::new(config);
        assert!(watcher.poll_changes().is_empty());
    }

    #[test]
    fn watcher_empty_folders_returns_empty() {
        let mut config = test_config();
        config.monitored_folders.clear();
        let watcher = FileAccessWatcher::new(config);
        assert!(watcher.poll_changes().is_empty());
    }

    #[test]
    fn watcher_detects_new_file() {
        let tmp = tempfile::tempdir().unwrap();
        let config = FileAccessConfig {
            enabled: true,
            monitored_folders: vec![tmp.path().to_path_buf()],
            excluded_extensions: vec![],
            max_events_per_minute: 100,
        };
        let watcher = FileAccessWatcher::new(config);

        // First poll — empty directory
        let events = watcher.poll_changes();
        assert!(events.is_empty());

        // Create a file
        std::fs::write(tmp.path().join("new_file.rs"), "fn main() {}").unwrap();

        // Second poll — should detect the new file as Created
        let events = watcher.poll_changes();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, FileEventType::Created);
        assert_eq!(watcher.take_modified_count(), 1);
    }

    #[test]
    fn watcher_detects_modification() {
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("file.rs");
        std::fs::write(&file_path, "v1").unwrap();

        let config = FileAccessConfig {
            enabled: true,
            monitored_folders: vec![tmp.path().to_path_buf()],
            excluded_extensions: vec![],
            max_events_per_minute: 100,
        };
        let watcher = FileAccessWatcher::new(config);

        // First poll — establishes baseline
        let events = watcher.poll_changes();
        assert_eq!(events.len(), 1); // Created (first time seen)

        // Modify the file — force a different mtime by sleeping briefly
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&file_path, "v2").unwrap();

        // Second poll — should detect modification
        let events = watcher.poll_changes();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, FileEventType::Modified);
    }

    #[test]
    fn watcher_detects_deletion() {
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("doomed.rs");
        std::fs::write(&file_path, "bye").unwrap();

        let config = FileAccessConfig {
            enabled: true,
            monitored_folders: vec![tmp.path().to_path_buf()],
            excluded_extensions: vec![],
            max_events_per_minute: 100,
        };
        let watcher = FileAccessWatcher::new(config);

        // First poll — baseline
        watcher.poll_changes();

        // Delete the file
        std::fs::remove_file(&file_path).unwrap();

        // Second poll — should detect deletion
        let events = watcher.poll_changes();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, FileEventType::Deleted);
    }

    #[test]
    fn watcher_respects_extension_filter() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("good.rs"), "code").unwrap();
        std::fs::write(tmp.path().join("bad.tmp"), "temp").unwrap();

        let config = FileAccessConfig {
            enabled: true,
            monitored_folders: vec![tmp.path().to_path_buf()],
            excluded_extensions: vec![".tmp".to_string()],
            max_events_per_minute: 100,
        };
        let watcher = FileAccessWatcher::new(config);

        let events = watcher.poll_changes();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].extension, Some("rs".to_string()));
    }

    #[test]
    fn poll_changes_respects_max_tracked_files_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let limit = 20usize;
        let overflow = 30usize;

        for i in 0..(limit + overflow) {
            std::fs::write(tmp.path().join(format!("file_{i}.rs")), "x").unwrap();
        }

        let config = FileAccessConfig {
            enabled: true,
            monitored_folders: vec![tmp.path().to_path_buf()],
            excluded_extensions: vec![],
            max_events_per_minute: u32::MAX,
        };
        let watcher = FileAccessWatcher::new_with_limit(config, limit);

        let events = watcher.poll_changes();
        // Truncation: first poll sees all files as Created; the limit bounds
        // the count even though 50 files exist on disk.
        assert!(
            events.len() <= limit,
            "first poll emitted {} events, limit={limit}",
            events.len()
        );
    }
}
