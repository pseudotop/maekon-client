use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ProcessEntryResponse {
    pub pid: u32,
    pub name: String,
    pub cpu_usage: f64,
    pub memory_bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct ProcessSnapshotResponse {
    pub timestamp: String,
    pub processes: Vec<ProcessEntryResponse>,
}
