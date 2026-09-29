//! #12094 exercise factory admission, probe ordering and both Codex writers.

use super::*;
use crate::session_adapters::policy_tests::{
    config, send, InertCli, APP_SERVER, CLAUDE, CODEX, GEMINI,
};
use crate::subprocess_provider::{ProbedSubprocessCli, SubprocessCliAuthStatus};
use maekon_core::config::CodexAppServerRollout;

fn exec_sibling(fixture: &InertCli) -> Vec<ProbedSubprocessCli> {
    vec![ProbedSubprocessCli {
        detected: fixture.surface(CODEX),
        auth_status: SubprocessCliAuthStatus::Authenticated,
        auth_detail: None,
    }]
}

async fn assert_denied_without_spawn(
    result: Result<Arc<dyn ConversationSession>, CoreError>,
    fixture: &InertCli,
) {
    let denied = matches!(&result, Err(CoreError::PolicyDenied { .. }));
    // A mistakenly admitted request continues through the real inert child,
    // exposing the forbidden side effect in a constructor-removal control.
    if let Ok(session) = result {
        send(session.as_ref()).await;
        session.terminate().await;
    }
    assert!(
        !fixture.spawned(),
        "policy denial must precede every child spawn"
    );
    assert!(denied, "expected policy.denied");
}

#[tokio::test]
async fn subprocess_policy_factory_denies_before_probe_with_isolated_path_control() {
    const WORKER: &str = "MAEKON_POLICY_FACTORY_FIXTURE";
    const TEST: &str = "session_manager::tests::subprocess_policy::subprocess_policy_factory_denies_before_probe_with_isolated_path_control";
    if let Some(root) = std::env::var_os(WORKER) {
        let fixture = InertCli::attached(root.into());
        for surface in [
            None,
            Some(CODEX),
            Some(APP_SERVER),
            Some(CLAUDE),
            Some(GEMINI),
            Some("unknown"),
        ] {
            let manager = test_manager();
            let mut request = config(CODEX);
            request.surface_id = surface.map(str::to_string);
            request.tools_enabled = false;
            assert_denied_without_spawn(manager.create_session(request).await, &fixture).await;
            assert!(manager.list_sessions().await.is_empty());
        }
        // Same PATH, executable, model and prompt now exercise discovery and
        // a real factory-created conversation. No inherited provider can run.
        let manager = test_manager();
        let session = manager
            .create_session(config(CODEX))
            .await
            .expect("enabled factory");
        assert_eq!(session.info().turn_count, 0);
        send(session.as_ref()).await;
        assert!(fixture.spawned());
        assert_eq!(session.info().turn_count, 1);
        assert_eq!(manager.list_sessions().await.len(), 1);
        assert!(fixture.read("stdin.txt").contains("policy prompt"));
        return;
    }

    // Isolate PATH in another copy of this test binary instead of racing the
    // process-wide environment used by unrelated tests or installed CLIs.
    let fixture = InertCli::new("codex", None);
    let mut command =
        tokio::process::Command::new(std::env::current_exe().expect("test executable"));
    command
        .args([TEST, "--exact", "--nocapture"])
        .env(WORKER, &fixture.root)
        .env("PATH", &fixture.root)
        .env("PATHEXT", ".EXE")
        .kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), command.output())
        .await
        .expect("bounded factory worker")
        .expect("factory worker process");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        fixture.spawned(),
        "positive worker control must execute the inert CLI"
    );
}

#[tokio::test]
async fn subprocess_policy_codex_entry_points_deny_without_probe_or_fallback() {
    for rollout in [
        CodexAppServerRollout::Off,
        CodexAppServerRollout::OptIn,
        CodexAppServerRollout::Default,
    ] {
        let fixture = InertCli::new("codex", None);
        let manager = test_manager().with_codex_app_server_rollout(rollout);
        let mut request = config(APP_SERVER);
        request.tools_enabled = false;
        assert_denied_without_spawn(
            manager
                .create_codex_session_with_fallback(
                    &fixture.surface(APP_SERVER),
                    &exec_sibling(&fixture),
                    &request,
                    &None,
                )
                .await,
            &fixture,
        )
        .await;
        // Empty discovery proves the direct exec guard precedes even sibling
        // selection; this must remain PolicyDenied rather than NotFound.
        assert_denied_without_spawn(
            manager.build_codex_exec_session(&[], &request, &None, "policy_test"),
            &fixture,
        )
        .await;
        #[cfg(feature = "analysis")]
        assert_denied_without_spawn(
            manager
                .connect_codex_app_server(&fixture.surface(APP_SERVER), &request)
                .await,
            &fixture,
        )
        .await;
        assert!(manager.list_sessions().await.is_empty());
    }
}

#[tokio::test]
async fn subprocess_policy_codex_factory_rejects_unverified_overrides() {
    let fixture = InertCli::new("codex", None);
    let manager = test_manager().with_codex_app_server_rollout(CodexAppServerRollout::Default);
    for (sandbox, approval) in [
        (Some("workspace-write"), None),
        (Some("danger-full-access"), None),
        (Some("invalid"), None),
        (Some(""), None),
        (None, Some("untrusted")),
        (None, Some("on-failure")),
        (None, Some("invalid")),
        (None, Some("")),
    ] {
        let mut request = config(APP_SERVER);
        request.sandbox_policy = sandbox.map(str::to_string);
        request.approval_policy = approval.map(str::to_string);
        assert_denied_without_spawn(
            manager
                .create_codex_session_with_fallback(
                    &fixture.surface(APP_SERVER),
                    &exec_sibling(&fixture),
                    &request,
                    &None,
                )
                .await,
            &fixture,
        )
        .await;
        assert_denied_without_spawn(
            manager.build_codex_exec_session(&[], &request, &None, "policy_test"),
            &fixture,
        )
        .await;
        #[cfg(feature = "analysis")]
        assert_denied_without_spawn(
            manager
                .connect_codex_app_server(&fixture.surface(APP_SERVER), &request)
                .await,
            &fixture,
        )
        .await;
    }
}

#[tokio::test]
async fn subprocess_policy_exec_fallback_preserves_default_and_explicit_approval() {
    for rollout in [CodexAppServerRollout::Off, CodexAppServerRollout::OptIn] {
        for approval in [None, Some("never"), Some("on-request")] {
            let fixture = InertCli::new("codex", approval);
            // Model a transport failure only. It remains eligible for exec,
            // while the policy-denial cases above never reach this probe.
            std::fs::write(fixture.root.join("fail-version"), "synthetic failure")
                .expect("fixture");
            let manager = test_manager().with_codex_app_server_rollout(rollout);
            let mut request = config(APP_SERVER);
            request.sandbox_policy = Some("read-only".to_string());
            request.approval_policy = approval.map(str::to_string);
            let session = manager
                .create_codex_session_with_fallback(
                    &fixture.surface(APP_SERVER),
                    &exec_sibling(&fixture),
                    &request,
                    &None,
                )
                .await
                .expect("eligible exec fallback");
            send(session.as_ref()).await;
            assert!(fixture.read("argv.txt").starts_with("exec\n"));
            #[cfg(feature = "analysis")]
            assert_eq!(
                fixture.read("spawns.log").contains("--version"),
                rollout == CodexAppServerRollout::OptIn
            );
        }
    }
}

#[cfg(feature = "analysis")]
#[tokio::test]
async fn subprocess_policy_app_server_thread_start_preserves_verified_policy() {
    for sandbox in [None, Some("read-only")] {
        for approval in [None, Some("never"), Some("on-request")] {
            let fixture = InertCli::new("codex", approval);
            let manager = test_manager();
            let mut request = config(APP_SERVER);
            request.cwd = Some(fixture.root.to_string_lossy().into_owned());
            request.sandbox_policy = sandbox.map(str::to_string);
            request.approval_policy = approval.map(str::to_string);
            let session = manager
                .connect_codex_app_server(&fixture.surface(APP_SERVER), &request)
                .await
                .expect("inert app-server handshake");
            send(session.as_ref()).await;
            let thread: serde_json::Value = fixture
                .read("requests.jsonl")
                .lines()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("request JSON"))
                .find(|request| request["method"] == "thread/start")
                .expect("actual thread/start request");
            assert_eq!(thread["params"]["sandbox"], "read-only");
            assert_eq!(thread["params"]["model"], "policy-fixture");
            assert_eq!(thread["params"]["cwd"].as_str(), request.cwd.as_deref());
            assert_eq!(
                thread["params"]
                    .get("approvalPolicy")
                    .and_then(serde_json::Value::as_str),
                approval
            );
            session.terminate().await;
        }
    }
}
