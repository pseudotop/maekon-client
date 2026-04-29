use oneshim_api_contracts::processes::{ProcessEntryResponse, ProcessSnapshotResponse};
use oneshim_core::models::activity::ProcessSnapshot;

pub(crate) fn assemble_process_snapshot_response(
    snapshot: ProcessSnapshot,
) -> ProcessSnapshotResponse {
    ProcessSnapshotResponse {
        timestamp: snapshot.timestamp.to_rfc3339(),
        processes: snapshot
            .processes
            .into_iter()
            .map(|process| ProcessEntryResponse {
                pid: process.pid,
                name: process.name,
                cpu_usage: process.cpu_usage as f64,
                memory_bytes: process.memory_bytes,
            })
            .collect(),
    }
}
