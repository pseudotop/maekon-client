mod focus_metrics;
mod retention;
mod segments;
mod suggestions;
mod work_sessions;

#[cfg(test)]
mod tests;

// Re-export pub(crate) helpers for sibling modules
// (events.rs, calibration_store_impl.rs, integration_query_impl.rs)
pub(crate) use suggestions::map_local_suggestion_row;
pub(crate) use work_sessions::enum_to_sql_str;
