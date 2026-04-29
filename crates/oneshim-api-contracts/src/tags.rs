use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct TagResponse {
    pub id: i64,
    pub name: String,
    pub color: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateTagRequest {
    pub name: String,
    pub color: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTagRequest {
    pub name: String,
    pub color: String,
}

#[derive(Debug, Deserialize)]
pub struct BatchTagRequest {
    pub frame_ids: Vec<i64>,
    pub tag_id: i64,
}

#[derive(Debug, Serialize)]
pub struct BatchTagResponse {
    pub tagged_count: u32,
}
