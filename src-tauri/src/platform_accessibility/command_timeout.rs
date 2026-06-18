use std::io::Read;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

pub(super) const ACCESSIBILITY_COMMAND_TIMEOUT: Duration = Duration::from_secs(3);

pub(super) fn command_output_with_timeout(
    command: &mut Command,
    timeout: Duration,
) -> std::io::Result<Option<Output>> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let stdout_reader = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
        let mut buffer = Vec::new();
        if let Some(ref mut pipe) = stdout {
            pipe.read_to_end(&mut buffer)?;
        }
        Ok(buffer)
    });
    let stderr_reader = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
        let mut buffer = Vec::new();
        if let Some(ref mut pipe) = stderr {
            pipe.read_to_end(&mut buffer)?;
        }
        Ok(buffer)
    });

    let started_at = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            let stdout = stdout_reader
                .join()
                .unwrap_or_else(|_| Err(std::io::Error::other("stdout reader panicked")))?;
            let stderr = stderr_reader
                .join()
                .unwrap_or_else(|_| Err(std::io::Error::other("stderr reader panicked")))?;
            return Ok(Some(Output {
                status,
                stdout,
                stderr,
            }));
        }

        if started_at.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Ok(None);
        }

        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_output_with_timeout_returns_fast_output() {
        let mut command = fast_output_command();
        let output = command_output_with_timeout(&mut command, Duration::from_secs(2))
            .expect("command should launch")
            .expect("fast command should finish");

        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("ok"));
    }

    #[test]
    fn command_output_with_timeout_kills_slow_process() {
        let mut command = slow_command();
        let output = command_output_with_timeout(&mut command, Duration::from_millis(50))
            .expect("command should launch");

        assert!(output.is_none());
    }

    #[cfg(target_os = "windows")]
    fn fast_output_command() -> Command {
        let mut command = Command::new("cmd");
        command.args(["/C", "echo ok"]);
        command
    }

    #[cfg(not(target_os = "windows"))]
    fn fast_output_command() -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", "printf ok"]);
        command
    }

    #[cfg(target_os = "windows")]
    fn slow_command() -> Command {
        let mut command = Command::new("powershell");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Sleep -Milliseconds 500",
        ]);
        command
    }

    #[cfg(not(target_os = "windows"))]
    fn slow_command() -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 1"]);
        command
    }
}
