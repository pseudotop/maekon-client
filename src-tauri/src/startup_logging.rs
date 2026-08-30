use std::path::Path;

use tracing_appender::{
    non_blocking::{NonBlocking, WorkerGuard},
    rolling::{InitError, RollingFileAppender, Rotation},
};

/// Keeps the asynchronous file writer alive for the lifetime of the app.
#[allow(dead_code)]
pub(crate) struct LogWorkerGuard(pub(crate) Option<WorkerGuard>);

pub(crate) fn try_file_log_writer(log_dir: &Path) -> Result<(NonBlocking, WorkerGuard), InitError> {
    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("maekon.log")
        .build(log_dir)?;

    Ok(tracing_appender::non_blocking(file_appender))
}

#[cfg(test)]
mod tests {
    #[test]
    fn returns_error_instead_of_panicking_for_blocked_path() {
        let temp_dir = tempfile::tempdir().expect("temporary directory must be created");
        let blocking_file = temp_dir.path().join("not-a-directory");
        std::fs::write(&blocking_file, b"blocks child directory creation")
            .expect("blocking fixture must be written");

        let error = match super::try_file_log_writer(&blocking_file.join("logs")) {
            Err(error) => error,
            Ok(_) => panic!("file logging must reject a child path beneath a regular file"),
        };

        assert_ne!(error.to_string(), "");
    }

    #[test]
    fn returns_worker_guard_for_writable_path() {
        let temp_dir = tempfile::tempdir().expect("temporary directory must be created");

        let (_writer, _guard) = super::try_file_log_writer(&temp_dir.path().join("logs"))
            .expect("writable log directory must initialize");
    }
}
