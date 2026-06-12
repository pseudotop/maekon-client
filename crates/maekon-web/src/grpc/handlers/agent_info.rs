//! ADR-013: Agent info + health-check RPC handler group.

use crate::proto::dashboard::v1::health_check_response::Status as HealthStatus;
use crate::proto::dashboard::v1::{
    AgentInfoResponse, GetAgentInfoRequest, HealthCheckRequest, HealthCheckResponse,
};
use std::time::Instant;
use tonic::{Request, Response, Status};

pub async fn get_agent_info(
    started_at: Instant,
    _req: Request<GetAgentInfoRequest>,
) -> Result<Response<AgentInfoResponse>, Status> {
    let platform = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    };

    let build_profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };

    let response = AgentInfoResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
        build_profile: build_profile.to_string(),
        uptime_secs: started_at.elapsed().as_secs() as i64,
        platform: platform.to_string(),
    };

    Ok(Response::new(response))
}

pub async fn health_check(
    _req: Request<HealthCheckRequest>,
) -> Result<Response<HealthCheckResponse>, Status> {
    // v1: always SERVING once the server is up. Future iters can probe
    // storage / scheduler state for a richer readiness signal.
    let response = HealthCheckResponse {
        status: HealthStatus::Serving as i32,
        message: String::new(),
    };
    Ok(Response::new(response))
}
