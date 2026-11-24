use super::*;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tempfile::TempDir;
use tokio::process::Command;

// This executable only reads/writes its owned pipes, supplied test paths and
// fixed bytes. It never invokes a model, shell, user CLI or network endpoint.
struct FakeCli {
    _directory: TempDir,
    binary: PathBuf,
}

impl FakeCli {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("owned fake executable directory");
        let source = temp.path().join("fake.rs");
        let binary = temp
            .path()
            .join(if cfg!(windows) { "fake.exe" } else { "fake" });
        std::fs::write(
            &source,
            r#"
use std::io::{Read, Write};
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let mode = args[0].as_str();
    let n: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    match mode {
        "echo" => {
            let mut input = Vec::new();
            std::io::stdin().read_to_end(&mut input).unwrap();
            std::io::stdout().write_all(&input).unwrap();
            std::io::stderr().write_all(b"diagnostic").unwrap();
        }
        "duplex" => {
            std::io::stdout().write_all(&vec![b'o'; n]).unwrap();
            std::io::stderr().write_all(&vec![b'e'; n]).unwrap();
            let mut input = Vec::new();
            std::io::stdin().read_to_end(&mut input).unwrap();
            std::io::stdout().write_all(&input).unwrap();
        }
        "stdout" | "stderr" => {
            if mode == "stdout" { std::io::stdout().write_all(&vec![b'o'; n]).unwrap(); }
            else { std::io::stderr().write_all(&vec![b'e'; n]).unwrap(); }
            // Keep the pipe open to distinguish a streaming cap from a size
            // check that only runs after EOF. TTL also bounds a broken control.
            if args.get(2).map(String::as_str) == Some("hold") {
                std::thread::sleep(std::time::Duration::from_secs(3));
            }
        }
        "stall" => {
            std::fs::write(&args[2], b"started").unwrap();
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
        "early-exit" => {
            std::io::stderr().write_all(b"early").unwrap();
            std::process::exit(7);
        }
        _ => panic!("unexpected fixture mode"),
    }
}
"#,
        )
        .expect("fake executable source");
        let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let status = std::process::Command::new(compiler)
            .arg(&source)
            .arg("-o")
            .arg(&binary)
            .status()
            .expect("compile owned executable");
        assert!(status.success(), "the process fixture must compile");
        Self {
            _directory: temp,
            binary,
        }
    }

    fn spawn(&self, mode: &str, size: usize, extra: Option<&Path>) -> Child {
        let mut command = Command::new(&self.binary);
        command.arg(mode).arg(size.to_string());
        if let Some(extra) = extra {
            command.arg(extra);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
            .kill_on_drop(true)
            .spawn()
            .expect("spawn owned executable")
    }
}

async fn invoke(child: Child, prompt: &str, budget: Duration) -> Result<Output, CoreError> {
    tokio::time::timeout(
        Duration::from_secs(6),
        write_prompt_and_collect_output(child, prompt, "owned fake CLI", budget),
    )
    .await
    .expect("transport itself must remain bounded")
}

#[tokio::test]
async fn exact_stdin_limit_and_utf8_are_delivered_without_shell_interpretation() {
    let fixture = FakeCli::new();
    let prompt = "가;\n".repeat(128 * 1024); // Five UTF-8 bytes per repetition.
    let output = invoke(
        fixture.spawn("echo", 0, None),
        &prompt,
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, prompt.as_bytes());
    assert_eq!(output.stderr, b"diagnostic");
    let prompt = "x".repeat(1024 * 1024);
    let output = invoke(
        fixture.spawn("echo", 0, None),
        &prompt,
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    assert_eq!(output.stdout, prompt.as_bytes());
}

#[tokio::test]
async fn both_pipes_are_drained_while_a_large_stdin_write_is_pending() {
    let fixture = FakeCli::new();
    let prompt = "p".repeat(512 * 1024);
    let output = invoke(
        fixture.spawn("duplex", 96 * 1024, None),
        &prompt,
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    assert!(output.status.success());
    assert_eq!(&output.stdout[..96 * 1024], vec![b'o'; 96 * 1024]);
    assert_eq!(&output.stdout[96 * 1024..], prompt.as_bytes());
    assert_eq!(output.stderr, vec![b'e'; 96 * 1024]);
}

#[tokio::test]
async fn stdout_and_stderr_exact_limits_are_not_truncated() {
    let fixture = FakeCli::new();
    for (mode, limit) in [("stdout", 4 * 1024 * 1024), ("stderr", 256 * 1024)] {
        let output = invoke(fixture.spawn(mode, limit, None), "", Duration::from_secs(2))
            .await
            .unwrap();
        assert!(output.status.success());
        if mode == "stdout" {
            assert_eq!(output.stdout, vec![b'o'; limit]);
        } else {
            assert_eq!(output.stderr, vec![b'e'; limit]);
        }
    }
}

#[tokio::test]
async fn streaming_stdout_and_stderr_overflow_fail_before_eof_and_reap() {
    let fixture = FakeCli::new();
    for (mode, limit) in [("stdout", 4 * 1024 * 1024), ("stderr", 256 * 1024)] {
        let child = fixture.spawn(mode, limit + 1, Some(Path::new("hold")));
        let pid = child.id().unwrap();
        let error = invoke(child, "", Duration::from_millis(700))
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("{mode} byte limit exceeded")),
            "{error}"
        );
        assert_reaped(pid);
    }
}

#[tokio::test]
async fn excess_stdin_is_rejected_without_retaining_the_child_or_echoing_input() {
    let fixture = FakeCli::new();
    let child = fixture.spawn("echo", 0, None);
    let pid = child.id().unwrap();
    let prompt = "private@example.invalid".repeat(60_000);
    let error = invoke(child, &prompt, Duration::from_secs(2))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("stdin byte limit exceeded"));
    assert!(!error.to_string().contains("private@example.invalid"));
    assert_reaped(pid);
}

#[tokio::test]
async fn early_nonzero_exit_preserves_status_even_with_pending_stdin() {
    let fixture = FakeCli::new();
    let output = invoke(
        fixture.spawn("early-exit", 0, None),
        &"p".repeat(512 * 1024),
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(output.stderr, b"early");
}

#[tokio::test]
async fn nonconsuming_stdin_times_out_and_the_direct_child_is_reaped() {
    let fixture = FakeCli::new();
    let temp = tempfile::tempdir().unwrap();
    let ready = temp.path().join("ready");
    let child = fixture.spawn("stall", 0, Some(&ready));
    let pid = child.id().unwrap();
    let error = invoke(child, &"p".repeat(512 * 1024), Duration::from_millis(250))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "network.timeout");
    assert!(
        ready.exists(),
        "the owned process must actually have started"
    );
    assert_reaped(pid);
}

#[tokio::test]
async fn cancellation_of_the_parent_reaps_the_running_direct_child() {
    let fixture = FakeCli::new();
    let temp = tempfile::tempdir().unwrap();
    let ready = temp.path().join("ready");
    let child = fixture.spawn("stall", 0, Some(&ready));
    let pid = child.id().unwrap();
    let task = tokio::spawn(async move {
        write_prompt_and_collect_output(
            child,
            &"p".repeat(512 * 1024),
            "owned fake CLI",
            Duration::from_secs(5),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !ready.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("owned process handshake");
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    #[cfg(target_os = "linux")]
    tokio::time::timeout(Duration::from_secs(1), async {
        while Path::new(&format!("/proc/{pid}")).exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("cancelled direct child must be reaped, including zombies");
    assert_reaped(pid);
}

fn assert_reaped(pid: u32) {
    #[cfg(target_os = "linux")]
    assert!(
        !Path::new(&format!("/proc/{pid}")).exists(),
        "owned PID {pid} is still present"
    );
    #[cfg(not(target_os = "linux"))]
    let _ = pid; // Linux /proc is the explicit native reaping observation.
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn delayed_caller_cannot_receive_success_after_the_original_deadline() {
    use std::future::{poll_fn, Future};
    use std::task::Poll;

    let fixture = FakeCli::new();
    let child = fixture.spawn("echo", 0, None);
    let pid = child.id().unwrap();
    let started = Instant::now();
    let mut call = Box::pin(write_prompt_and_collect_output(
        child,
        "finished-before-deadline",
        "owned fake CLI",
        Duration::from_secs(1),
    ));
    poll_fn(|context| {
        assert!(call.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    tokio::time::timeout(Duration::from_millis(500), async {
        while Path::new(&format!("/proc/{pid}")).exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("positive control: direct child finished before the execution deadline");
    // Leave the ready receiver unpolled until the original execution deadline.
    tokio::time::sleep_until(started + Duration::from_millis(1100)).await;
    assert_eq!(call.await.unwrap_err().code(), "network.timeout");
    assert_reaped(pid);
}
