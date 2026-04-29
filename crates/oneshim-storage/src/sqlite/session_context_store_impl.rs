use oneshim_core::error::CoreError;
use oneshim_core::models::storage_records::SuggestionRecord;
use oneshim_core::ports::session_context_store::SessionContextStorePort;

use super::SqliteStorage;

impl SessionContextStorePort for SqliteStorage {
    fn list_suggestions(&self, limit: usize) -> Result<Vec<SuggestionRecord>, CoreError> {
        SqliteStorage::list_suggestions(self, limit).map_err(CoreError::from)
    }
}

#[cfg(test)]
mod tests {
    //! Smoke test for SessionContextStorePort trait impl.
    //! Single-method thin delegator to edge_intelligence/suggestions.rs.

    use oneshim_core::ports::session_context_store::SessionContextStorePort;

    use super::SqliteStorage;

    #[test]
    fn session_context_store_port_smoke_list_empty_db() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        let results =
            <SqliteStorage as SessionContextStorePort>::list_suggestions(&storage, 100).unwrap();
        assert!(results.is_empty(), "fresh DB has no suggestions");
    }
}
