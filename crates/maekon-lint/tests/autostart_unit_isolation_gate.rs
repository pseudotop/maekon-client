//! #12441: compile the actual test attributes with OS-free, tripwire adapters.
//! Included by the app unit suite and explicitly registered in private CI's
//! standalone lint target list; the app catalog alone does not run every test.

#![cfg(test)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn client_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("src-tauri/src/autostart/tests.rs").is_file())
        .expect("client source root")
        .to_path_buf()
}

fn test_item(source: &str, name: &str) -> String {
    // Parse equivalent LF/CRLF source on Windows without changing the file.
    let source = source.replace("\r\n", "\n");
    let source = source.as_str();
    let signature = source.find(&format!("fn {name}(")).expect("test function");
    let start = source[..signature].rfind("\n\n").map_or(0, |i| i + 2);
    let body = signature + source[signature..].find('{').expect("test body");
    let mut depth = 0;
    for (offset, ch) in source[body..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return source[start..=body + offset].to_owned();
                }
            }
            _ => {}
        }
    }
    panic!("unterminated test body");
}

fn remove_attribute(item: &str, name: &str) -> String {
    let start = item.find(&format!("#[{name}")).expect("guard attribute");
    let end = start + item[start..].find(']').expect("attribute end") + 1;
    format!("{}{}", &item[..start], &item[end..])
}

fn run_fixture(item: &str, target: &str) -> Output {
    let dir = tempfile::tempdir().expect("fixture directory");
    let source = dir.path().join("fixture.rs");
    let binary = dir
        .path()
        .join(format!("fixture{}", std::env::consts::EXE_SUFFIX));
    // Only the cfg key is renamed: the real boolean expression is compiled for
    // each simulated OS without redefining rustc's built-in target_os. These
    // fixtures never link platform modules or native registration APIs.
    let item = item.replace("target_os", "autostart_test_os");
    std::fs::write(
        &source,
        format!(
            r#"
#![allow(dead_code)]
fn enable_autostart() -> Result<(), String> {{
    if cfg!(autostart_test_os = "unsupported") {{ Ok(()) }}
    else {{ panic!("native enable tripwire") }}
}}
fn disable_autostart() -> Result<(), String> {{
    if cfg!(autostart_test_os = "unsupported") {{ Ok(()) }}
    else {{ panic!("native disable tripwire") }}
}}
fn is_autostart_enabled() -> Result<bool, String> {{ Ok(false) }}
{item}
"#
        ),
    )
    .expect("write isolated fixture");
    let compile = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .args(["--edition=2021", "--test"])
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .arg("--cfg")
        .arg(format!("autostart_test_os={target:?}"))
        .output()
        .expect("run rustc");
    assert!(
        compile.status.success(),
        "fixture must compile before a verdict: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    Command::new(binary)
        .arg("--test-threads=1")
        .output()
        .expect("run isolated tests")
}

#[test]
fn supported_platforms_exclude_native_roundtrip_with_removal_control() {
    let source = std::fs::read_to_string(client_root().join("src-tauri/src/autostart/tests.rs"))
        .expect("read actual unit test");
    let item = test_item(&source, "enable_disable_roundtrip_unsupported_platform");
    assert_eq!(
        test_item(
            &source.replace("\r\n", "\n").replace('\n', "\r\n"),
            "enable_disable_roundtrip_unsupported_platform"
        ),
        item
    );
    for target in ["macos", "windows", "linux"] {
        let guarded = run_fixture(&item, target);
        assert!(
            guarded.status.success(),
            "{target}: {}",
            String::from_utf8_lossy(&guarded.stdout)
        );
        assert!(String::from_utf8_lossy(&guarded.stdout).contains("0 passed; 0 failed"));
        let removed = run_fixture(&remove_attribute(&item, "cfg"), target);
        assert!(
            !removed.status.success(),
            "{target}: removing cfg must expose the mutation"
        );
        assert!(String::from_utf8_lossy(&removed.stdout).contains("native enable tripwire"));
    }
    // Positive control: the unsupported test is discovered and actually runs.
    let unsupported = run_fixture(&item, "unsupported");
    assert!(
        unsupported.status.success(),
        "{}",
        String::from_utf8_lossy(&unsupported.stdout)
    );
    assert!(String::from_utf8_lossy(&unsupported.stdout).contains("1 passed; 0 failed"));
}

#[test]
fn native_command_canary_is_ignored_with_removal_control() {
    let source = std::fs::read_to_string(client_root().join("src-tauri/src/commands/autostart.rs"))
        .expect("read actual command test");
    let item = test_item(&source, "enable_then_disable_round_trip");
    assert_eq!(
        test_item(
            &source.replace("\r\n", "\n").replace('\n', "\r\n"),
            "enable_then_disable_round_trip"
        ),
        item
    );
    // Preserve the actual attributes, but use a synchronous tripwire body so
    // this control cannot touch the OS even if ignore is accidentally removed.
    let attributes = item.split_once("async fn").expect("async canary").0;
    let fixture = format!(
        "{} fn native_canary() {{ panic!(\"native canary tripwire\"); }}",
        attributes.replace("#[tokio::test]", "#[test]")
    );
    let guarded = run_fixture(&fixture, "macos");
    assert!(guarded.status.success());
    assert!(String::from_utf8_lossy(&guarded.stdout).contains("0 passed; 0 failed; 1 ignored"));
    let removed = run_fixture(&remove_attribute(&fixture, "ignore"), "macos");
    assert!(
        !removed.status.success(),
        "removing ignore must expose the mutation"
    );
    assert!(String::from_utf8_lossy(&removed.stdout).contains("native canary tripwire"));
}
