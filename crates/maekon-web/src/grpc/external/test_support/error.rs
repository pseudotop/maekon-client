//! Fallible fixture operations retain their cause without printing key material.

#[derive(Debug, thiserror::Error)]
pub enum FixtureError {
    #[error("fixture I/O failed while {operation}")]
    Io {
        operation: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("fixture certificate generation failed")]
    Certificate(#[from] rcgen::Error),
    #[error("fixture JWT signing failed")]
    Jwt(#[from] jsonwebtoken::errors::Error),
    #[error("fixture clock is before the Unix epoch")]
    Clock(#[from] std::time::SystemTimeError),
    #[error("fixture timestamp is outside the supported range")]
    TimestampRange,
    #[error("fixture certificate lifetime is outside the supported range")]
    CertificateLifetime,
    #[error("fixture transport connection failed")]
    Transport(#[from] tonic::transport::Error),
    #[error("external test server failed before readiness")]
    Server(#[source] super::super::ServeExternalError),
    #[error("external test server task failed")]
    Task(#[from] tokio::task::JoinError),
    #[error("external test server stopped before readiness")]
    StoppedBeforeReady,
    #[error("external test server did not become ready within the startup deadline")]
    StartupTimeout,
}

impl FixtureError {
    pub(super) fn io(operation: &'static str, source: std::io::Error) -> Self {
        Self::Io { operation, source }
    }
}
