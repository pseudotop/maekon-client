use oneshim_api_contracts::sessions::SessionResponse;
use oneshim_core::models::activity::SessionStats;

pub(crate) fn assemble_session_response(session: SessionStats) -> SessionResponse {
    let active_duration_secs = session.ended_at.map(|ended_at| {
        let total_secs = (ended_at - session.started_at).num_seconds() as u64;
        total_secs.saturating_sub(session.total_idle_secs)
    });

    SessionResponse {
        session_id: session.session_id,
        started_at: session.started_at.to_rfc3339(),
        ended_at: session.ended_at.map(|timestamp| timestamp.to_rfc3339()),
        total_events: session.total_events,
        total_frames: session.total_frames,
        total_idle_secs: session.total_idle_secs,
        active_duration_secs,
    }
}
