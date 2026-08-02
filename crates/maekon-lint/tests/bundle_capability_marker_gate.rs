//! Workspace gate: a bundle that claims to be login-capable must actually be
//! one, and the default bundle must stay account-free (#9659).
//!
//! ## The defect this exists to stop recurring
//!
//! `scripts/build-macos-dev-bundle.sh` ran `cargo tauri build` with no
//! `--features`, so the only `.app` route the repository documented produced a
//! binary in which sign-in was not compiled at all. Both builds succeed, both
//! print the same summary, and the two artifacts are indistinguishable — the
//! failure surfaces when someone tries to log in on the demo machine.
//!
//! Documentation alone cannot hold this: the defect existed *because* the
//! documentation and the script drifted apart. So the invariants live here,
//! read out of the real files, and run under `cargo test -p maekon-lint`
//! (ADR-075 P-4: no dead gates).
//!
//! ## What each test actually proves — and what it does not
//!
//! | Test | Reads | Catches |
//! |---|---|---|
//! | `server_feature_is_not_a_default_feature` | `src-tauri/Cargo.toml` | someone quietly making sign-in mandatory |
//! | `grpc_feature_still_implies_server` | `src-tauri/Cargo.toml` | the verifier's implication table going stale |
//! | `marker_literals_match_between_rust_and_shell` | both files | a rename on one side only |
//! | `bundle_script_forwards_features_and_verifies_the_artifact` | the build script | the `--features` passthrough or the verification call being deleted |
//! | `verifier_script_is_exported_to_the_public_repo` | include manifest | exit 127 on the public runner |
//! | `verifier_*` (unix) | *executes* the verifier | the verifier silently ceasing to detect a mismatch |
//!
//! The first five are text assertions over source files: they prove the wiring
//! is present, not that it works. The last group is the one that proves the
//! mechanism bites — it runs the real script against fixture bytes and asserts
//! the exit codes. Neither group can prove that `cargo tauri build` honours
//! `--features` end to end; only a real macOS bundle build does that, and the
//! artifact check inside the build script is what turns that into a loud
//! failure rather than a silent one.

use std::path::{Path, PathBuf};

/// Absolute path to the maekon-client workspace root (two levels above the
/// `maekon-lint` crate manifest).
///
/// Deliberately not `tests/common/mod.rs::workspace_root`: that module also
/// carries the CI-YAML collectors, and `-D dead-code` fails any test binary
/// that imports the module without using them. This gate reads scripts and
/// manifests, not workflows.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("maekon-lint sits two levels under the workspace root")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let path = workspace_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

const MARKER_ON: &str = "maekon-build-capability:server=on";
const MARKER_OFF: &str = "maekon-build-capability:server=off";

const CAPABILITIES_RS: &str = "src-tauri/src/build_capabilities.rs";
const VERIFIER_SH: &str = "scripts/verify-bundle-capabilities.sh";
const BUNDLE_SH: &str = "scripts/build-macos-dev-bundle.sh";

/// Extract the `default = [ ... ]` array body from `[features]`.
fn default_feature_list(cargo_toml: &str) -> String {
    let features_idx = cargo_toml
        .find("\n[features]")
        .expect("src-tauri/Cargo.toml has a [features] table");
    let rest = &cargo_toml[features_idx..];
    let default_idx = rest
        .find("default = [")
        .expect("[features] declares a `default` list");
    let after = &rest[default_idx + "default = [".len()..];
    let end = after.find(']').expect("`default` list is closed");
    after[..end].to_string()
}

/// THE PRODUCT INVARIANT. Sign-in is strictly opt-in: the client is complete
/// without an account, and `server` must never become a default feature. A
/// change that flips this is not a bug fix for #9659 — it is a product
/// decision that deserves an explicit argument, not a quiet diff.
///
/// SCOPE (review #9676 L1): this reads the literal `default = [...]` list only.
/// A *transitive* default-on — say `analysis = [..., "server"]`, where
/// `analysis` is itself default — would slip past it. That hole is not silent,
/// though: the default bundle build runs the artifact verifier with
/// `--expect ""`, the binary would report `server=on`, and the build fails
/// with "the artifact carries the server feature although it was not
/// requested". So the escape route is fail-closed at the artifact, one layer
/// out, rather than caught here.
#[test]
fn server_feature_is_not_a_default_feature() {
    let cargo_toml = read("src-tauri/Cargo.toml");
    let defaults = default_feature_list(&cargo_toml);
    let has_server = defaults
        .split(',')
        .map(|entry| entry.trim().trim_matches('"'))
        .any(|entry| entry == "server" || entry == "grpc");

    assert!(
        !has_server,
        "`server` (or `grpc`, which implies it) appeared in src-tauri/Cargo.toml \
         [features] default = [{defaults}].\n\
         Sign-in is opt-in by design: the OSS build is a self-contained local-first \
         product and every feature works without an account. Making the server \
         transport default-on forces every user onto the account path.\n\
         If that is genuinely intended, change it deliberately and update this gate \
         with the reasoning — do not let it happen as a side effect."
    );
}

/// The shell verifier maps a requested feature list onto an expected
/// capability, and it has to know that `grpc` pulls `server` in. If Cargo.toml
/// ever stops implying it, that mapping silently starts rejecting correct
/// builds (or accepting wrong ones).
#[test]
fn grpc_feature_still_implies_server() {
    let cargo_toml = read("src-tauri/Cargo.toml");
    assert!(
        cargo_toml.contains(r#"grpc = ["server""#),
        "src-tauri/Cargo.toml no longer declares `grpc = [\"server\", ...]`.\n\
         {VERIFIER_SH} treats `grpc` as implying `server` when it checks a build \
         request against the artifact; that implication must keep matching Cargo."
    );
}

/// The marker is spelled in two places by necessity — Rust compiles it into
/// the binary, shell greps for it. A rename on one side only turns the artifact
/// check into "marker not found", which is loud but misleading. Pin them.
#[test]
fn marker_literals_match_between_rust_and_shell() {
    let rust = read(CAPABILITIES_RS);
    let shell = read(VERIFIER_SH);

    for (marker, cfg) in [(MARKER_ON, "server"), (MARKER_OFF, "not(server)")] {
        assert!(
            rust.contains(marker),
            "{CAPABILITIES_RS} no longer defines the `{cfg}` marker literal `{marker}`"
        );
        assert!(
            shell.contains(marker),
            "{VERIFIER_SH} no longer greps for `{marker}`, which {CAPABILITIES_RS} \
             still compiles into the binary — the verifier would report \
             'no marker found' on a perfectly good artifact"
        );
    }

    // Each literal must sit under the matching cfg, or the artifact would
    // report the opposite of the truth — worse than reporting nothing.
    let on_idx = rust
        .find(MARKER_ON)
        .expect("checked present immediately above");
    let off_idx = rust
        .find(MARKER_OFF)
        .expect("checked present immediately above");
    let before_on = &rust[..on_idx];
    let before_off = &rust[..off_idx];
    assert!(
        before_on.rfind("#[cfg(feature = \"server\")]") > before_on.rfind("#[cfg(not(feature"),
        "the `server=on` marker in {CAPABILITIES_RS} is not the one guarded by \
         `#[cfg(feature = \"server\")]` — the artifact would misreport its capability"
    );
    assert!(
        before_off.rfind("#[cfg(not(feature = \"server\"))]")
            > before_off.rfind("#[cfg(feature = \"server\")]"),
        "the `server=off` marker in {CAPABILITIES_RS} is not the one guarded by \
         `#[cfg(not(feature = \"server\"))]` — the artifact would misreport its capability"
    );
}

/// The two halves of the fix in the build script. Deleting either one restores
/// the #9659 defect: without the passthrough the bundle cannot log in, and
/// without the verification nothing says so.
#[test]
fn bundle_script_forwards_features_and_verifies_the_artifact() {
    let script = read(BUNDLE_SH);

    assert!(
        script.contains("MAEKON_DEV_BUNDLE_FEATURES"),
        "{BUNDLE_SH} no longer reads MAEKON_DEV_BUNDLE_FEATURES — there is again no \
         documented way to build a login-capable .app (#9659)"
    );
    assert!(
        script.contains(r#"--features "$BUNDLE_FEATURES""#),
        "{BUNDLE_SH} no longer forwards the requested features to `cargo tauri build`. \
         This is the original #9659 defect verbatim: the build succeeds and silently \
         produces a bundle in which sign-in is not compiled in."
    );
    assert!(
        script.contains("verify-bundle-capabilities.sh"),
        "{BUNDLE_SH} no longer verifies what it built. Without it the two bundle \
         kinds are indistinguishable from the build output again."
    );
    assert!(
        script.contains(r#"--expect "$BUNDLE_FEATURES""#),
        "{BUNDLE_SH} calls the verifier without deriving the expectation from the same \
         variable that drives the cargo invocation. Only that shared derivation makes \
         a dropped `--features` fail the build instead of passing quietly."
    );
}

/// `build-macos-dev-bundle.sh` is exported to the public repository. It now
/// shells out to the verifier, so an unexported verifier means exit 127 for
/// every public user of the bundle script — the forgotten-registration class
/// `workflow_script_export_gate` already guards for workflows.
#[test]
fn verifier_script_is_exported_to_the_public_repo() {
    let manifest_path = workspace_root().join("scripts/public-repo-include.txt");
    // Absent on a public checkout, where everything is already exported.
    let Ok(manifest) = std::fs::read_to_string(&manifest_path) else {
        eprintln!(
            "bundle_capability_marker_gate: no include manifest (public checkout) — skipping"
        );
        return;
    };
    let exported = |rel: &str| manifest.lines().any(|line| line.trim() == rel);

    assert!(
        exported(VERIFIER_SH),
        "{VERIFIER_SH} is missing from scripts/public-repo-include.txt, but the exported \
         {BUNDLE_SH} invokes it — public users would hit exit 127"
    );
    assert!(
        exported(BUNDLE_SH),
        "{BUNDLE_SH} is missing from scripts/public-repo-include.txt"
    );
}

// ---------------------------------------------------------------------------
// Behavioural tests: run the real verifier against fixture bytes.
//
// Unix-only because they invoke bash. The text assertions above are what hold
// on Windows; these are what prove the mechanism actually detects a mismatch
// rather than merely being wired up.
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod behaviour {
    use super::{workspace_root, MARKER_OFF, MARKER_ON, VERIFIER_SH};
    use std::process::{Command, Output};

    /// Write `contents` to a file in `dir` and return its path.
    fn fixture(dir: &std::path::Path, name: &str, contents: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        // Interleave NUL bytes so the fixture exercises the same binary-file
        // path a real Mach-O executable does — a verifier that only worked on
        // plain text would pass a naive fixture and fail on the real bundle.
        let bytes = format!("\u{0}prefix\u{0}{contents}\u{0}suffix\u{0}");
        std::fs::write(&path, bytes).expect("write fixture");
        path
    }

    fn run(args: &[&str]) -> Output {
        Command::new("bash")
            .arg(workspace_root().join(VERIFIER_SH))
            .args(args)
            .output()
            .expect("run verify-bundle-capabilities.sh")
    }

    /// THE BITE. This is the exact regression: a build was asked for `server`
    /// and produced an artifact without it. The verifier must fail.
    #[test]
    fn verifier_fails_when_server_was_requested_but_is_absent_from_the_artifact() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bin = fixture(dir.path(), "maekon", MARKER_OFF);

        let out = run(&["--expect", "server", bin.to_str().unwrap()]);

        assert!(
            !out.status.success(),
            "verifier accepted a server=off artifact for a `--features server` build — \
             the #9659 defect would pass silently again.\nstdout: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("capability mismatch"),
            "expected an explicit capability-mismatch diagnosis, got: {stderr}"
        );
    }

    /// The mirror direction: an artifact that carries `server` when nobody
    /// asked for it opts users onto the account path silently. That must fail
    /// too, or the gate only defends one half of the invariant.
    #[test]
    fn verifier_fails_when_server_is_present_but_was_not_requested() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bin = fixture(dir.path(), "maekon", MARKER_ON);

        let out = run(&["--expect", "", bin.to_str().unwrap()]);

        assert!(
            !out.status.success(),
            "verifier accepted a server=on artifact for a default build"
        );
    }

    #[test]
    fn verifier_accepts_matching_requests_in_both_directions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let on = fixture(dir.path(), "maekon-on", MARKER_ON);
        let off = fixture(dir.path(), "maekon-off", MARKER_OFF);

        for (bin, features) in [(&on, "server"), (&on, "grpc"), (&off, "")] {
            let out = run(&["--expect", features, bin.to_str().unwrap()]);
            assert!(
                out.status.success(),
                "verifier rejected a matching artifact ({} vs features '{features}')\nstderr: {}",
                bin.display(),
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    /// An artifact with no marker must never be reported as login-capable, and
    /// must not be waved through. "I could not tell" is a failure, not a pass.
    #[test]
    fn verifier_fails_on_an_artifact_without_any_marker() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("stranger");
        std::fs::write(&path, b"no marker here").expect("write fixture");

        let out = run(&["--expect", "server", path.to_str().unwrap()]);

        assert!(
            !out.status.success(),
            "verifier passed an artifact carrying no capability marker at all"
        );
    }

    /// Reaching into a real `.app` layout: the executable name depends on the
    /// Tauri product name, sidecars sit alongside it and carry no marker, and
    /// the bundle path contains a space. All three have to work.
    #[test]
    fn verifier_reads_the_executable_inside_an_app_bundle() {
        let dir = tempfile::tempdir().expect("tempdir");
        let macos = dir.path().join("Maekon Dev.app/Contents/MacOS");
        std::fs::create_dir_all(&macos).expect("create bundle layout");
        fixture(&macos, "Maekon Dev", MARKER_ON);
        std::fs::write(macos.join("maekon-sandbox-worker"), b"sidecar").expect("write sidecar");

        let app = dir.path().join("Maekon Dev.app");
        let out = run(&["--expect", "server", app.to_str().unwrap()]);

        assert!(
            out.status.success(),
            "verifier could not read the executable inside a .app bundle\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("server=on"),
            "expected the bundle capability in stdout, got: {stdout}"
        );
    }

    /// Both markers present means the cfg selection broke. Picking one would
    /// hand back a confident wrong answer — the failure mode this whole gate
    /// exists to prevent.
    #[test]
    fn verifier_fails_when_both_markers_are_present() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bin = fixture(dir.path(), "maekon", &format!("{MARKER_ON} {MARKER_OFF}"));

        let out = run(&[bin.to_str().unwrap()]);

        assert!(
            !out.status.success(),
            "verifier resolved an ambiguous artifact instead of refusing to guess"
        );
    }
}
