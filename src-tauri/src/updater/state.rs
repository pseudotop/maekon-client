//! Update state management: last check path, save/load check time, scheduling.
#![allow(dead_code)] // State helpers called from updater check/install paths

use std::path::PathBuf;

use super::{UpdateError, Updater};

impl Updater {
    pub fn last_check_path() -> PathBuf {
        directories::BaseDirs::new()
            .map(|d| d.cache_dir().join("oneshim").join("last_update_check"))
            .unwrap_or_else(|| PathBuf::from("/tmp/oneshim_last_update_check"))
    }

    pub fn save_last_check_time(&self) -> Result<(), UpdateError> {
        let path = Self::last_check_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let now = chrono::Utc::now().timestamp();
        std::fs::write(&path, now.to_string())?;
        Ok(())
    }

    pub fn should_check_for_updates(&self) -> bool {
        if !self.config.enabled {
            return false;
        }

        let path = Self::last_check_path();
        if !path.exists() {
            return true;
        }

        let Ok(content) = std::fs::read_to_string(&path) else {
            return true;
        };

        let Ok(last_check) = content.trim().parse::<i64>() else {
            return true;
        };

        let now = chrono::Utc::now().timestamp();
        let interval_secs = (self.config.check_interval_hours as i64) * 3600;

        now - last_check >= interval_secs
    }
}
