//! Shared log file helpers -- used by both Tauri commands and RuntimeLogProvider.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// Compute the runtime log directory from the platform config path.
pub fn runtime_log_dir() -> PathBuf {
    oneshim_core::config_manager::ConfigManager::data_dir()
        .map(|d| d.join("logs"))
        .unwrap_or_else(|_| PathBuf::from("logs"))
}

/// Find the most recently modified log file in the given directory.
pub fn newest_log_file(log_dir: &Path) -> Result<Option<PathBuf>, String> {
    if !log_dir.exists() {
        return Ok(None);
    }

    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    let entries = std::fs::read_dir(log_dir).map_err(|err| {
        format!(
            "Failed to read runtime log directory '{}': {err}",
            log_dir.display()
        )
    })?;

    for entry in entries {
        let entry =
            entry.map_err(|err| format!("Failed to inspect runtime log directory entry: {err}"))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

        match newest.as_ref() {
            Some((current_modified, _)) if modified <= *current_modified => {}
            _ => newest = Some((modified, path)),
        }
    }

    Ok(newest.map(|(_, path)| path))
}

/// Read the last `line_limit` lines from a log file.
pub fn tail_log_file(path: &Path, line_limit: usize) -> Result<(usize, String), String> {
    let file = File::open(path).map_err(|err| {
        format!(
            "Failed to open runtime log file '{}': {err}",
            path.display()
        )
    })?;
    let reader = BufReader::new(file);
    let mut lines = VecDeque::with_capacity(line_limit);

    for line in reader.lines() {
        let line = line.map_err(|err| {
            format!(
                "Failed to read runtime log file '{}': {err}",
                path.display()
            )
        })?;
        if lines.len() == line_limit {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    let count = lines.len();
    let recent_text = lines.into_iter().collect::<Vec<_>>().join("\n");
    Ok((count, recent_text))
}
