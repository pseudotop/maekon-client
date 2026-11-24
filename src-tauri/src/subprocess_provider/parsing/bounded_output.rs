//! Resource bounds for the existing one-shot CLI transport (#12457).
//!
//! This is not a candidate authorization, billing or no-tools capability gate.

use std::io;
use std::process::Output;
use std::time::Duration;

use maekon_core::error::CoreError;
use maekon_core::error_codes::{InternalCode, NetworkCode};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Child;
use tokio::sync::oneshot;
use tokio::time::{timeout, timeout_at, Instant};

const MAX_STDIN_BYTES: usize = 1024 * 1024;
const MAX_STDOUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_STDERR_BYTES: usize = 256 * 1024;
const CLEANUP_GRACE: Duration = Duration::from_secs(1);

fn transport_error(message: &'static str) -> CoreError {
    CoreError::Internal {
        code: InternalCode::Generic,
        message: message.to_owned(),
    }
}

fn timed_out(budget: Duration) -> CoreError {
    CoreError::RequestTimeout {
        code: NetworkCode::Timeout,
        timeout_ms: budget.as_millis().min(u128::from(u64::MAX)) as u64,
    }
}

async fn read_limited<R: AsyncRead + Unpin>(
    reader: R,
    limit: usize,
    limit_error: &'static str,
) -> Result<Vec<u8>, CoreError> {
    let mut bytes = Vec::new();
    reader
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| transport_error("Subprocess output read failed"))?;
    if bytes.len() > limit {
        return Err(transport_error(limit_error));
    }
    Ok(bytes)
}

async fn collect(child: &mut Child, prompt: Vec<u8>) -> Result<Output, CoreError> {
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| transport_error("Subprocess stdin pipe is unavailable"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| transport_error("Subprocess stdout pipe is unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| transport_error("Subprocess stderr pipe is unavailable"))?;

    let write = async move {
        let result = stdin.write_all(&prompt).await;
        drop(stdin); // Deliver EOF; no detached writer survives cancellation.
        match result {
            Ok(()) => Ok(()),
            // Preserve the exit-status contract for a child that exits early.
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
            Err(_) => Err(transport_error("Subprocess stdin write failed")),
        }
    };
    let wait = async {
        child
            .wait()
            .await
            .map_err(|_| transport_error("Subprocess wait failed"))
    };
    // #6262: a child may fill both output pipes before it consumes stdin.
    let ((), stdout, stderr, status) = tokio::try_join!(
        write,
        read_limited(
            stdout,
            MAX_STDOUT_BYTES,
            "Subprocess stdout byte limit exceeded"
        ),
        read_limited(
            stderr,
            MAX_STDERR_BYTES,
            "Subprocess stderr byte limit exceeded"
        ),
        wait,
    )?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

async fn stop_and_reap(child: &mut Child) -> Result<(), CoreError> {
    // A racing exit can make the kill request fail. Observed wait completion,
    // not the kill request's return value, is the cleanup evidence.
    let _ = child.start_kill();
    match timeout(CLEANUP_GRACE, child.wait()).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(_)) | Err(_) => Err(transport_error("Subprocess cleanup did not complete")),
    }
}

/// Drain both pipes and write stdin concurrently under one execution budget.
///
/// An owning supervisor observes caller cancellation and kills/waits the direct
/// child. Its I/O futures are scoped, not separate writer/reader tasks. Failure
/// cleanup has an additional bounded one-second grace; it never returns output.
/// Callers retain kill_on_drop(true) for an unpolled call or runtime shutdown.
/// This does not contain descendants or establish a concurrency/approval budget.
pub(crate) async fn write_prompt_and_collect_output(
    mut child: Child,
    prompt: &str,
    _surface_label: &str,
    budget: Duration,
) -> Result<Output, CoreError> {
    let deadline = Instant::now().checked_add(budget);
    // Reject before copying: an overlarge caller-owned prompt is not duplicated.
    let prompt = (prompt.len() <= MAX_STDIN_BYTES).then(|| prompt.as_bytes().to_vec());
    let (mut reply, receive) = oneshot::channel();

    tokio::spawn(async move {
        let mut result = if let Some(deadline) = deadline {
            let work = async {
                let prompt = prompt
                    .ok_or_else(|| transport_error("Subprocess stdin byte limit exceeded"))?;
                collect(&mut child, prompt).await
            };
            tokio::select! {
                biased;
                _ = reply.closed() => None,
                output = timeout_at(deadline, work) => Some(match output {
                    Ok(Ok(output)) if Instant::now() < deadline => Ok(output),
                    Ok(Err(error)) => Err(error),
                    Ok(Ok(_)) | Err(_) => Err(timed_out(budget)),
                }),
            }
        } else {
            Some(Err(timed_out(budget)))
        };
        if !matches!(&result, Some(Ok(_))) {
            if let Err(error) = stop_and_reap(&mut child).await {
                if result.is_some() {
                    result = Some(Err(error));
                } else {
                    tracing::warn!("Cancelled CLI subprocess cleanup did not complete");
                }
            }
        }
        if let Some(result) = result {
            // A cancellation racing successful completion discards the output.
            let _ = reply.send(result);
        }
    });

    let output = receive
        .await
        .map_err(|_| transport_error("Subprocess supervisor stopped"))??;
    // The supervisor can finish before a delayed caller polls its receiver.
    if deadline.is_none_or(|deadline| Instant::now() >= deadline) {
        return Err(timed_out(budget));
    }
    Ok(output)
}

#[cfg(test)]
#[path = "bounded_output_tests.rs"]
mod tests;
