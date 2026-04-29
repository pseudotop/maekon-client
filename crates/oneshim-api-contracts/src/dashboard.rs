use serde::Deserialize;

/// Query parameters for the dashboard day endpoint.
#[derive(Debug, Deserialize)]
pub struct DashboardDayQuery {
    /// Date in YYYY-MM-DD format. Defaults to today.
    pub date: Option<String>,
}

/// Internal deserialization helper for content activity JSON blobs.
#[derive(Debug, Deserialize)]
pub struct RawContentActivity {
    pub content_label: Option<String>,
    pub duration_secs: Option<u64>,
    pub work_type: Option<String>,
}

/// Internal deserialization helper for content activity JSON (minimal fields).
#[derive(Debug, Deserialize)]
pub struct RawContentActivityBrief {
    pub content_label: Option<String>,
    pub duration_secs: Option<u64>,
}
