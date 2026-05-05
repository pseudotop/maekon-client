use serde::Deserialize;

/// Query parameters for listing weekly digests.
#[derive(Debug, Deserialize)]
pub struct DigestListQuery {
    /// Maximum number of digests to return (default: 4).
    pub limit: Option<usize>,
}
