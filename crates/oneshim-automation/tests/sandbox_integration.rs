use oneshim_automation::sandbox::ipc::resolve_worker_path;
use std::io::Write;

#[test]
fn worker_binary_discoverable() {
    match resolve_worker_path() {
        Ok(path) => assert!(path.exists(), "path exists but file missing: {:?}", path),
        Err(_) => eprintln!("skipping: worker not found (expected in some envs)"),
    }
}

#[test]
fn worker_stdin_stdout_roundtrip() {
    let worker = match resolve_worker_path() {
        Ok(p) => p,
        Err(_) => {
            eprintln!("skipping: worker not found");
            return;
        }
    };

    let mut child = std::process::Command::new(&worker)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn failed");

    let stdin = child.stdin.as_mut().unwrap();
    stdin
        .write_all(br#"{"action":{"KeyType":{"text":"test"}}}"#)
        .unwrap();
    stdin.write_all(b"\n").unwrap();
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("wait failed");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""success":true"#), "got: {stdout}");
}

#[test]
fn worker_malformed_input() {
    let worker = match resolve_worker_path() {
        Ok(p) => p,
        Err(_) => {
            eprintln!("skipping: worker not found");
            return;
        }
    };

    let mut child = std::process::Command::new(&worker)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn failed");

    let stdin = child.stdin.as_mut().unwrap();
    stdin.write_all(b"not json\n").unwrap();
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("wait failed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""success":false"#), "got: {stdout}");
}
