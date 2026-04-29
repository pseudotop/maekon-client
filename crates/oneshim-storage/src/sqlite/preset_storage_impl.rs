use chrono::Utc;
use oneshim_core::error::CoreError;
use oneshim_core::models::intent::{PresetCategory, WorkflowPreset, WorkflowStep};
use oneshim_core::ports::preset_storage::PresetStorage;

use super::SqliteStorage;

impl PresetStorage for SqliteStorage {
    /// List all custom presets from the `automation_presets` table.
    fn list_presets(&self) -> Result<Vec<WorkflowPreset>, CoreError> {
        let conn = self.conn.lock().map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("lock: {e}"),
        })?;

        let mut stmt = conn
            .prepare(
                "SELECT id, name, description, category, steps_json, builtin, platform, ai_profile_id
                 FROM automation_presets
                 ORDER BY name",
            )
            .map_err(|e| CoreError::Storage { code: oneshim_core::error_codes::StorageCode::Failed, message: format!("prepare: {e}") })?;

        let rows = stmt
            .query_map([], |row| {
                Ok(PresetRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    category: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    steps_json: row.get(4)?,
                    builtin: row.get::<_, i32>(5)? != 0,
                    platform: row.get(6)?,
                    ai_profile_id: row.get(7)?,
                })
            })
            .map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("query: {e}"),
            })?;

        let mut result = Vec::new();
        for row in rows {
            let row = row.map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("row: {e}"),
            })?;
            result.push(row.into_preset()?);
        }
        Ok(result)
    }

    /// Get a single preset by ID.
    fn get_preset(&self, id: &str) -> Result<Option<WorkflowPreset>, CoreError> {
        let conn = self.conn.lock().map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("lock: {e}"),
        })?;

        let result = conn.query_row(
            "SELECT id, name, description, category, steps_json, builtin, platform, ai_profile_id
             FROM automation_presets
             WHERE id = ?1",
            [id],
            |row| {
                Ok(PresetRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    category: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    steps_json: row.get(4)?,
                    builtin: row.get::<_, i32>(5)? != 0,
                    platform: row.get(6)?,
                    ai_profile_id: row.get(7)?,
                })
            },
        );

        match result {
            Ok(row) => Ok(Some(row.into_preset()?)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("query: {e}"),
            }),
        }
    }

    /// Insert or replace a preset. Sets `updated_at` to now; sets `created_at`
    /// only for new rows.
    fn save_preset(&self, preset: &WorkflowPreset) -> Result<(), CoreError> {
        let conn = self.conn.lock().map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("lock: {e}"),
        })?;

        let steps_json = serde_json::to_string(&preset.steps).map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("serialize steps: {e}"),
        })?;
        let category_str =
            serde_json::to_string(&preset.category).map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("serialize category: {e}"),
            })?;
        let now = Utc::now().to_rfc3339();

        conn.execute(
            "INSERT INTO automation_presets
             (id, name, description, category, steps_json, builtin, platform, ai_profile_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                description = excluded.description,
                category = excluded.category,
                steps_json = excluded.steps_json,
                builtin = excluded.builtin,
                platform = excluded.platform,
                ai_profile_id = excluded.ai_profile_id,
                updated_at = excluded.updated_at",
            rusqlite::params![
                preset.id,
                preset.name,
                preset.description,
                category_str,
                steps_json,
                preset.builtin as i32,
                preset.platform,
                preset.ai_profile_id,
                now,
                now,
            ],
        )
        .map_err(|e| CoreError::Storage { code: oneshim_core::error_codes::StorageCode::Failed, message: format!("upsert: {e}") })?;

        Ok(())
    }

    /// Delete a preset by ID. Built-in presets (builtin=1) are protected and
    /// will not be deleted. Returns true if a row was actually removed.
    fn delete_preset(&self, id: &str) -> Result<bool, CoreError> {
        let conn = self.conn.lock().map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("lock: {e}"),
        })?;

        let affected = conn
            .execute(
                "DELETE FROM automation_presets WHERE id = ?1 AND builtin = 0",
                [id],
            )
            .map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("delete: {e}"),
            })?;

        Ok(affected > 0)
    }
}

/// Internal helper struct for reading preset rows from SQLite.
struct PresetRow {
    id: String,
    name: String,
    description: String,
    category: String,
    steps_json: String,
    builtin: bool,
    platform: Option<String>,
    ai_profile_id: Option<String>,
}

impl PresetRow {
    fn into_preset(self) -> Result<WorkflowPreset, CoreError> {
        let steps: Vec<WorkflowStep> =
            serde_json::from_str(&self.steps_json).map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("deserialize steps: {e}"),
            })?;
        let category: PresetCategory =
            serde_json::from_str(&self.category).unwrap_or(PresetCategory::Custom);

        Ok(WorkflowPreset {
            id: self.id,
            name: self.name,
            description: self.description,
            category,
            steps,
            builtin: self.builtin,
            platform: self.platform,
            ai_profile_id: self.ai_profile_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::models::intent::{AutomationIntent, PresetCategory, WorkflowStep};
    use oneshim_core::ports::preset_storage::PresetStorage;

    fn test_preset(id: &str, builtin: bool) -> WorkflowPreset {
        WorkflowPreset {
            id: id.to_string(),
            name: format!("Preset {id}"),
            description: format!("Description for {id}"),
            category: PresetCategory::Custom,
            steps: vec![WorkflowStep {
                name: "Step 1".to_string(),
                intent: AutomationIntent::ExecuteHotkey {
                    keys: vec!["Ctrl".to_string(), "S".to_string()],
                },
                delay_ms: 0,
                stop_on_failure: true,
            }],
            builtin,
            platform: None,
            ai_profile_id: None,
        }
    }

    #[test]
    fn preset_crud_roundtrip() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        // Save a preset
        let preset = test_preset("my-preset", false);
        PresetStorage::save_preset(&storage, &preset).unwrap();

        // List should return it
        let list = PresetStorage::list_presets(&storage).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "my-preset");
        assert_eq!(list[0].name, "Preset my-preset");
        assert_eq!(list[0].category, PresetCategory::Custom);
        assert_eq!(list[0].steps.len(), 1);

        // Get by ID
        let fetched = PresetStorage::get_preset(&storage, "my-preset")
            .unwrap()
            .expect("should find preset");
        assert_eq!(fetched.id, "my-preset");
        assert_eq!(fetched.description, "Description for my-preset");

        // Update it (save_preset is upsert)
        let mut updated = preset.clone();
        updated.name = "Updated Name".to_string();
        updated.category = PresetCategory::Productivity;
        PresetStorage::save_preset(&storage, &updated).unwrap();

        let fetched = PresetStorage::get_preset(&storage, "my-preset")
            .unwrap()
            .expect("should find preset");
        assert_eq!(fetched.name, "Updated Name");
        assert_eq!(fetched.category, PresetCategory::Productivity);

        // Delete
        let deleted = PresetStorage::delete_preset(&storage, "my-preset").unwrap();
        assert!(deleted);

        // Verify deletion
        let fetched = PresetStorage::get_preset(&storage, "my-preset").unwrap();
        assert!(fetched.is_none());

        let list = PresetStorage::list_presets(&storage).unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn delete_builtin_noop() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        // Save a builtin preset
        let preset = test_preset("builtin-preset", true);
        PresetStorage::save_preset(&storage, &preset).unwrap();

        // Attempting to delete a builtin preset should not remove it
        let deleted = PresetStorage::delete_preset(&storage, "builtin-preset").unwrap();
        assert!(!deleted);

        // Verify it still exists
        let fetched = PresetStorage::get_preset(&storage, "builtin-preset")
            .unwrap()
            .expect("builtin preset should remain");
        assert_eq!(fetched.id, "builtin-preset");
        assert!(fetched.builtin);
    }

    #[test]
    fn list_empty_returns_empty() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let list = PresetStorage::list_presets(&storage).unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn get_nonexistent_returns_none() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let result = PresetStorage::get_preset(&storage, "nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn delete_nonexistent_returns_false() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let deleted = PresetStorage::delete_preset(&storage, "nonexistent").unwrap();
        assert!(!deleted);
    }

    #[test]
    fn save_preset_with_all_fields() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        let preset = WorkflowPreset {
            id: "full-preset".to_string(),
            name: "Full Preset".to_string(),
            description: "A preset with all fields".to_string(),
            category: PresetCategory::Workflow,
            steps: vec![
                WorkflowStep {
                    name: "Open App".to_string(),
                    intent: AutomationIntent::ActivateApp {
                        app_name: "VSCode".to_string(),
                    },
                    delay_ms: 500,
                    stop_on_failure: false,
                },
                WorkflowStep {
                    name: "Save".to_string(),
                    intent: AutomationIntent::ExecuteHotkey {
                        keys: vec!["Cmd".to_string(), "S".to_string()],
                    },
                    delay_ms: 0,
                    stop_on_failure: true,
                },
            ],
            builtin: false,
            platform: Some("macos".to_string()),
            ai_profile_id: Some("profile-123".to_string()),
        };

        PresetStorage::save_preset(&storage, &preset).unwrap();

        let fetched = PresetStorage::get_preset(&storage, "full-preset")
            .unwrap()
            .expect("should find preset");
        assert_eq!(fetched.category, PresetCategory::Workflow);
        assert_eq!(fetched.steps.len(), 2);
        assert_eq!(fetched.platform, Some("macos".to_string()));
        assert_eq!(fetched.ai_profile_id, Some("profile-123".to_string()));
        assert_eq!(fetched.steps[0].delay_ms, 500);
        assert!(!fetched.steps[0].stop_on_failure);
    }
}
