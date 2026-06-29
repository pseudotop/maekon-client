//! SQLite suggestion persistence — split from the original monolithic
//! `suggestions.rs`. Public API and re-exports are unchanged.

mod context_columns;
mod feedback_retry;
mod legacy;
mod row_mapper;
mod unified;

// Re-export the shared row mapper that sibling modules (events.rs,
// calibration_store_impl.rs, integration_query_impl.rs) use directly.
pub(crate) use row_mapper::map_local_suggestion_row;
// Re-export the D2 HLC stamp gate so the `events.rs` save_suggestion path uses the SAME
// single-sourced gate as the unified.rs INSERTs (no duplicated privacy invariant).
pub(crate) use unified::suggestion_stamp;

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use maekon_core::models::suggestion::{
        Priority, Suggestion, SuggestionContextScope, SuggestionSource, SuggestionType,
    };

    use super::super::super::SqliteStorage;

    fn suggestion(id: &str, content: &str, created_offset_secs: i64) -> Suggestion {
        Suggestion {
            suggestion_id: id.to_string(),
            suggestion_type: SuggestionType::WorkGuidance,
            content: content.to_string(),
            priority: Priority::Medium,
            confidence_score: 0.82,
            relevance_score: 0.91,
            is_actionable: true,
            created_at: Utc::now() + Duration::seconds(created_offset_secs),
            expires_at: None,
            source: SuggestionSource::LlmLocal,
            reasoning: None,
            context_scope: None,
        }
    }

    fn suggestion_with_scope(
        id: &str,
        content: &str,
        app_name: &str,
        window_title: &str,
        target_id: &str,
    ) -> Suggestion {
        let mut suggestion = suggestion(id, content, 0);
        suggestion.context_scope = Some(SuggestionContextScope {
            app_name: Some(app_name.to_string()),
            window_title: Some(window_title.to_string()),
            target_id: Some(target_id.to_string()),
        });
        suggestion
    }

    #[test]
    fn d2_stamps_local_suggestions_but_not_llm_server() {
        // F0/#5186 + D2: RuleBased/LlmLocal suggestions get a monotonic HLC so they sync;
        // LlmServer suggestions are deliberately NOT stamped (stay (0,0,'')) so server-
        // synthesized prose never cross-broadcasts between a user's devices.
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        {
            // open_in_memory creates device_identity but not its row; insert one so the
            // stamp carries a real origin_device_id.
            let c = storage.conn.test_lock();
            c.execute(
                "INSERT INTO device_identity (id, device_id, device_name) VALUES (1, 'dev-x', 'X')",
                [],
            )
            .unwrap();
        }

        let mut local = suggestion("sugg-local", "local", 0);
        local.source = SuggestionSource::RuleBased;
        storage.save_rule_suggestion_sync(&local).unwrap();

        let mut server = suggestion("sugg-server", "server", 0);
        server.source = SuggestionSource::LlmServer;
        storage.save_rule_suggestion_sync(&server).unwrap();

        let read = |id: &str| -> (i64, String) {
            let c = storage.conn.test_lock();
            c.query_row(
                "SELECT hlc_wall_ms, origin_device_id FROM suggestions WHERE suggestion_id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
        };

        let (local_wall, local_dev) = read("sugg-local");
        assert!(local_wall > 0, "RuleBased suggestion must be HLC-stamped");
        assert_eq!(local_dev, "dev-x", "origin_device_id must be stamped");

        let (server_wall, server_dev) = read("sugg-server");
        assert_eq!(
            (server_wall, server_dev.as_str()),
            (0, ""),
            "LlmServer suggestion must NOT be stamped (D2 — no cross-broadcast)"
        );
    }

    #[tokio::test]
    async fn d2_gate_holds_on_save_suggestion_trait_path() {
        // The `StorageService::save_suggestion` path (events.rs) is the actual LlmServer
        // production intake. Verify it uses the SAME D2 gate: LlmLocal stamped, LlmServer not.
        use maekon_core::ports::storage::StorageService;

        let storage = SqliteStorage::open_in_memory(30).unwrap();
        {
            let c = storage.conn.test_lock();
            c.execute(
                "INSERT INTO device_identity (id, device_id, device_name) VALUES (1, 'dev-y', 'Y')",
                [],
            )
            .unwrap();
        }

        let mut local = suggestion("trait-local", "local", 0);
        local.source = SuggestionSource::LlmLocal;
        storage.save_suggestion(&local).await.unwrap();

        let mut server = suggestion("trait-server", "server", 0);
        server.source = SuggestionSource::LlmServer;
        storage.save_suggestion(&server).await.unwrap();

        let wall = |id: &str| -> i64 {
            let c = storage.conn.test_lock();
            c.query_row(
                "SELECT hlc_wall_ms FROM suggestions WHERE suggestion_id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert!(
            wall("trait-local") > 0,
            "LlmLocal via save_suggestion must be HLC-stamped"
        );
        assert_eq!(
            wall("trait-server"),
            0,
            "LlmServer via save_suggestion must NOT be stamped (D2)"
        );
    }

    #[test]
    fn list_suggestions_excludes_expired_rows() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let mut expired = suggestion("expired-suggestion", "Expired candidate", -20);
        expired.expires_at = Some(Utc::now() - Duration::minutes(5));
        storage.save_rule_suggestion_sync(&expired).unwrap();

        let active = suggestion("active-suggestion", "Active candidate", 0);
        storage.save_rule_suggestion_sync(&active).unwrap();

        let ids = storage
            .list_suggestions(10)
            .unwrap()
            .into_iter()
            .map(|row| row.suggestion_id)
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["active-suggestion".to_string()]);
    }

    #[test]
    fn list_suggestions_suppresses_duplicate_active_candidates() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        storage
            .save_rule_suggestion_sync(&suggestion(
                "duplicate-older",
                "Summarize the calculator result",
                -60,
            ))
            .unwrap();
        storage
            .save_rule_suggestion_sync(&suggestion(
                "duplicate-newer",
                "  Summarize   the calculator result  ",
                0,
            ))
            .unwrap();

        let ids = storage
            .list_suggestions(10)
            .unwrap()
            .into_iter()
            .map(|row| row.suggestion_id)
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["duplicate-newer".to_string()]);
    }

    #[test]
    fn list_suggestions_preserves_distinct_context_scopes() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        storage
            .save_rule_suggestion_sync(&suggestion_with_scope(
                "calculator-suggestion",
                "Summarize the current result",
                "Calculator",
                "Calculator",
                "display-result",
            ))
            .unwrap();
        storage
            .save_rule_suggestion_sync(&suggestion_with_scope(
                "notes-suggestion",
                "Summarize the current result",
                "Notes",
                "Planning note",
                "selected-text",
            ))
            .unwrap();

        let mut ids = storage
            .list_suggestions(10)
            .unwrap()
            .into_iter()
            .map(|row| row.suggestion_id)
            .collect::<Vec<_>>();
        ids.sort();

        assert_eq!(
            ids,
            vec![
                "calculator-suggestion".to_string(),
                "notes-suggestion".to_string()
            ]
        );
    }

    #[test]
    fn list_suggestions_by_state_restores_context_scope() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        storage
            .save_suggestion_with_state(
                &suggestion_with_scope(
                    "scoped-pending",
                    "Explain the calculator result",
                    "Calculator",
                    "Calculator",
                    "display-result",
                ),
                "pending",
                None,
            )
            .unwrap();

        let restored = storage
            .list_suggestions_by_state("pending", 10)
            .unwrap()
            .into_iter()
            .next()
            .and_then(|record| record.try_into_suggestion())
            .and_then(|suggestion| suggestion.context_scope)
            .unwrap();

        assert_eq!(restored.app_name.as_deref(), Some("Calculator"));
        assert_eq!(restored.window_title.as_deref(), Some("Calculator"));
        assert_eq!(restored.target_id.as_deref(), Some("display-result"));
    }

    /// #6938: under the LIMIT, deferred restore must keep the SOONEST-resurfacing
    /// snooze, not the newest-created. created_at and resurface_at are decoupled:
    /// here the OLDER-created suggestion resurfaces sooner, so with LIMIT 1 it must
    /// win. The old `created_at DESC` ordering would wrongly keep the newer-created
    /// one (whose resurface is far off) and drop the imminent snooze.
    #[test]
    fn list_deferred_by_resurface_keeps_soonest_not_newest_created() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let now = Utc::now();

        // OLD created (-3600s) but resurfaces SOON (+60s).
        let soon = suggestion("deferred-soon", "resurfaces soon", -3600);
        storage
            .save_suggestion_with_state(
                &soon,
                "deferred",
                Some(&(now + Duration::seconds(60)).to_rfc3339()),
            )
            .unwrap();
        // NEW created (0s) but resurfaces LATER (+3600s).
        let later = suggestion("deferred-later", "resurfaces later", 0);
        storage
            .save_suggestion_with_state(
                &later,
                "deferred",
                Some(&(now + Duration::seconds(3600)).to_rfc3339()),
            )
            .unwrap();

        // LIMIT 1 must keep the soonest-resurfacing (old-created) one.
        let restored = storage.list_deferred_suggestions_by_resurface(1).unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(
            restored[0].suggestion_id, "deferred-soon",
            "resurface-order must keep the soonest-resurfacing snooze under the limit"
        );

        // Sanity: the pre-fix created_at DESC ordering would have kept 'deferred-later'.
        let by_created = storage.list_suggestions_by_state("deferred", 1).unwrap();
        assert_eq!(by_created[0].suggestion_id, "deferred-later");
    }
}
