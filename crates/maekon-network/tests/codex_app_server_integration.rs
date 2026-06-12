//! Integration tests for the Codex `app-server` transport + session (E21 #4872).
//!
//! Unlike the inline unit tests (which use in-process `tokio::io::duplex`), these
//! drive the REAL `AppServerProcess` + `CodexAppServerSession` end-to-end through
//! a SPAWNED subprocess mock — the only deterministic means to exercise the
//! production spawn -> setpgid -> JSONL-wire -> `initialize` -> `thread/*` ->
//! `turn/start` chain (deep-review I5).
//!
//! Coverage:
//! - E21-3 (#4865): `initialize` handshake through the spawn path.
//! - E21-4 (#4866): `thread/start` (+ R6 sandbox clamp on the wire), `thread/resume`
//!   continuity (echo Ok + drift Err), multi-turn no-history-resend.
//! - E21-5 (#4867): streaming delta ORDER, tool-internal/unknown ignored.
//! - R2 (#24347): transport close -> whole child-tree reap regression.
//! - R4 (AC3): `initialize` request CONTRACT SNAPSHOT + userAgent parse / forward
//!   compat + unsupported/garbage handling (pure test; relies on #4871 fallback).
//! - E21-8 (#4870): FAIL-CLOSED approval round-trip — the mock emits an id-bearing
//!   server→client `requestApproval` REQUEST, the client classifies it on the
//!   reverse-request channel, the decider decides, and a correlated RESPONSE is
//!   written back (captured via `$MAEKON_REQ_LOG`). Covers auto-accept,
//!   fail-closed decline (no match / policy error), dangerous-command block, and
//!   network default-deny vs explicit opt-in. See the `approval_round_trip` mod.
//!
//! Unix-only (the spawn/reap path + every existing codex test are unix-gated).

#![cfg(unix)]

mod fake_codex_app_server;

use std::time::Duration;

use fake_codex_app_server::{
    read_log, unique_request_log, unique_turn_log, AccountReadBehavior, FakeCodexAppServer,
    Handshake, ResumeBehavior, TurnStep,
};
use futures::StreamExt;

use maekon_core::error::CoreError;
use maekon_core::models::ai_session::{
    MessageRole, OutboundMessage, SessionMessage, SessionState, SessionTransport,
};
use maekon_core::ports::conversation_session::ConversationSession;
use maekon_network::codex_app_server::{AppServerProcess, ClientInfo};
use maekon_network::codex_app_server_session::{CodexAppServerSession, ThreadConfig};

// ── shared helpers (mirror the inline test module's local helpers) ───────────

fn client_info() -> ClientInfo {
    ClientInfo {
        name: "maekon".to_string(),
        title: "Maekon".to_string(),
        version: "0.0.1".to_string(),
    }
}

fn thread_config() -> ThreadConfig {
    ThreadConfig {
        model: "gpt-5.4".to_string(),
        cwd: None,
        sandbox_policy: None,
        approval_policy: None,
    }
}

fn user_msg(content: &str) -> SessionMessage {
    SessionMessage {
        role: MessageRole::User,
        content: content.to_string(),
        attachments: vec![],
        tools: None,
        context: None,
        response_format: None,
    }
}

/// Drive one turn to completion, returning all emitted `OutboundMessage`s.
async fn collect_turn(session: &CodexAppServerSession, content: &str) -> Vec<OutboundMessage> {
    session
        .send_message(&user_msg(content))
        .await
        .expect("turn/start")
        .map(|r| r.expect("stream item"))
        .collect()
        .await
}

// ── E21-3: handshake through the real spawn path ─────────────────────────────

#[tokio::test]
async fn handshake_connects_and_exposes_server_useragent() {
    // The REAL spawn -> setpgid -> JSONL-wire -> initialize handshake succeeds
    // end-to-end against a SPAWNED subprocess (distinct from the duplex-based
    // unit tests), and ServerInfo flows into the session surface.
    let cmd = FakeCodexAppServer::new()
        .user_agent(Some("codex-app-server/1.2"))
        .thread_id("thr_42")
        .into_command();

    let session = CodexAppServerSession::connect(cmd, &client_info(), thread_config())
        .await
        .expect("connect + initialize + thread/start over a spawned subprocess");

    assert_eq!(session.session_id(), "thr_42");
    assert_eq!(session.provider_name(), "codex");
    assert!(
        session.is_external(),
        "app-server egresses chat off-device -> must stay external (#4869)"
    );
    assert_eq!(session.info().turn_count, 0);
    assert_eq!(session.info().state, SessionState::Active);
    assert_eq!(session.info().transport, SessionTransport::Subprocess);
}

// ── E21-4: thread/start carries model + clamps sandbox on the wire (R6) ───────

#[tokio::test]
async fn thread_start_sends_model_and_clamps_sandbox_read_only() {
    // Through the REAL connect path the thread/start request must carry the model
    // AND sandbox=="read-only", even though the caller requested "workspace-write"
    // (R6 not weakened — verified END-TO-END on the wire, not just in the unit
    // effective_sandbox test).
    // Capture the full inbound request stream so we can inspect the thread/start
    // line (only turn/start is appended by the turn-log branch).
    let req_log = unique_request_log("thread_start_clamp");
    let cmd = FakeCodexAppServer::new()
        .with_request_log(req_log.clone())
        .into_command();
    let config = ThreadConfig {
        model: "gpt-5.4".to_string(),
        cwd: None,
        sandbox_policy: Some("workspace-write".to_string()),
        approval_policy: None,
    };
    let _session = CodexAppServerSession::connect(cmd, &client_info(), config)
        .await
        .expect("connect");

    let logged = read_log(&req_log);
    let start_line = logged
        .lines()
        .find(|l| l.contains(r#""thread/start""#))
        .expect("a thread/start request was captured");
    assert!(
        start_line.contains(r#""model":"gpt-5.4""#),
        "thread/start carries the model: {start_line}"
    );
    assert!(
        start_line.contains(r#""sandbox":"read-only""#),
        "R6: caller-requested workspace-write is clamped to read-only on the wire: {start_line}"
    );
    assert!(
        !start_line.contains("workspace-write"),
        "R6: workspace-write must NOT reach the wire: {start_line}"
    );
    let _ = std::fs::remove_file(&req_log);
}

// ── E21-4: resume continuity (echo Ok + drift Err) ───────────────────────────

#[tokio::test]
async fn resume_reattaches_same_thread_stays_external() {
    // resume() against a server echoing the SAME threadId preserves continuity:
    // session_id == requested thread_id, external, fresh turn count.
    let cmd = FakeCodexAppServer::new()
        .resume(ResumeBehavior::Echo)
        .into_command();

    let session = CodexAppServerSession::resume(
        cmd,
        &client_info(),
        "thr_persist".to_string(),
        "gpt-5.4".to_string(),
    )
    .await
    .expect("resume re-attaches the existing thread");

    assert_eq!(session.session_id(), "thr_persist");
    assert!(session.is_external());
    assert_eq!(session.provider_name(), "codex");
    assert_eq!(session.info().turn_count, 0);
    assert_eq!(session.info().state, SessionState::Active);
}

#[tokio::test]
async fn resume_rejects_thread_id_drift() {
    // Continuity guard: a server echoing a DIFFERENT threadId makes resume() Err
    // (CoreError::Network) naming both ids — no silent drift onto a fresh thread.
    let cmd = FakeCodexAppServer::new()
        .resume(ResumeBehavior::Drift("thr_other".to_string()))
        .into_command();

    let result = CodexAppServerSession::resume(
        cmd,
        &client_info(),
        "thr_persist".to_string(),
        "gpt-5.4".to_string(),
    )
    .await;

    let err = result
        .err()
        .expect("resume must reject a different threadId");
    match err {
        CoreError::Network { message, .. } => {
            assert!(
                message.contains("thr_persist") && message.contains("thr_other"),
                "drift error names both threadIds: {message}"
            );
        }
        other => panic!("expected Err(Network) on threadId drift, got {other:?}"),
    }
}

#[tokio::test]
async fn resume_treats_omitted_thread_id_as_continuity_match() {
    // Defensive posture (production `resume`): when the server replies success
    // but OMITS the echoed threadId, the absence is treated as a MATCH (not a
    // drift) — resume re-attaches the requested thread without error.
    let cmd = FakeCodexAppServer::new()
        .resume(ResumeBehavior::OmitThreadId)
        .into_command();

    let session = CodexAppServerSession::resume(
        cmd,
        &client_info(),
        "thr_persist".to_string(),
        "gpt-5.4".to_string(),
    )
    .await
    .expect("an omitted threadId on resume is treated as a continuity match");

    assert_eq!(session.session_id(), "thr_persist");
    assert!(session.is_external());
}

// ── E21-5: streaming delta ORDER, tool-internal/unknown ignored ──────────────

#[tokio::test]
async fn streaming_delta_order_ignores_tool_and_unknown() {
    // A full turn through the REAL send_message must emit EXACTLY the ordered
    // Thinking, Text("Hello"), Text(" world"), Result("Hello world", usage 12/34)
    // — reasoning before answer deltas before the terminal Result; tool-internal
    // and unknown events ignored; answer-only accumulation. Mirrors the inline
    // fake_app_server_rich scenario, end-to-end via a spawned subprocess.
    let cmd = FakeCodexAppServer::new()
        .turn(vec![
            TurnStep::Reasoning("pondering"),
            TurnStep::ToolInternal("item/started"),
            TurnStep::Answer("Hello"),
            TurnStep::ToolInternal("item/commandExecution/outputDelta"),
            TurnStep::Answer(" world"),
            TurnStep::ToolInternal("turn/plan/updated"),
            TurnStep::Unknown("some/unknown/event"),
            TurnStep::Completed {
                usage: Some((12, 34)),
            },
        ])
        .into_command();

    let session = CodexAppServerSession::connect(cmd, &client_info(), thread_config())
        .await
        .unwrap();

    let outputs = collect_turn(&session, "hi").await;

    assert_eq!(
        outputs.len(),
        4,
        "tool-internal/unknown events must not yield; got {outputs:?}"
    );
    assert!(matches!(
        &outputs[0],
        OutboundMessage::Thinking { content, done } if content == "pondering" && !done
    ));
    assert!(matches!(
        &outputs[1],
        OutboundMessage::Text { content, done } if content == "Hello" && !done
    ));
    assert!(matches!(
        &outputs[2],
        OutboundMessage::Text { content, done } if content == " world" && !done
    ));
    match &outputs[3] {
        OutboundMessage::Result {
            content,
            done,
            usage,
        } => {
            assert_eq!(
                content, "Hello world",
                "Result accumulates only answer deltas"
            );
            assert!(done);
            let u = usage.as_ref().expect("usage mapped from turn/completed");
            assert_eq!((u.input_tokens, u.output_tokens), (12, 34));
        }
        other => panic!("expected terminal Result, got {other:?}"),
    }
}

// #5071: real codex 0.130.0 carries per-turn token usage in a
// `thread/tokenUsage/updated` notification (`tokenUsage.last`), NOT on
// `turn/completed`. The client must capture it and attach it to the terminal
// Result even when turn/completed carries no usage.
#[tokio::test]
async fn token_usage_from_token_usage_updated_when_turn_completed_has_none() {
    let cmd = FakeCodexAppServer::new()
        .turn(vec![
            TurnStep::Answer("ok"),
            TurnStep::TokenUsage {
                input_tokens: 7,
                output_tokens: 9,
            },
            // turn/completed carries NO usage (the real-codex shape).
            TurnStep::Completed { usage: None },
        ])
        .into_command();
    let session = CodexAppServerSession::connect(cmd, &client_info(), thread_config())
        .await
        .unwrap();

    let outputs = collect_turn(&session, "hi").await;
    let result = outputs
        .iter()
        .find_map(|m| match m {
            OutboundMessage::Result { usage, .. } => Some(usage.clone()),
            _ => None,
        })
        .expect("terminal Result emitted");
    let u = result.expect("usage captured from thread/tokenUsage/updated (not turn/completed)");
    assert_eq!((u.input_tokens, u.output_tokens), (7, 9));
}

#[tokio::test]
async fn approval_request_is_not_surfaced_as_chat_output() {
    // The approval request must NEVER appear in the chat answer stream — it is
    // round-tripped on the reverse-request channel, not mapped to OutboundMessage.
    // (The round-trip itself is asserted by the *_round_trip_* tests below.)
    let cmd = FakeCodexAppServer::new()
        .turn(vec![
            TurnStep::ApprovalRequest {
                method: "item/commandExecution/requestApproval",
                approval_id: 9001,
                command: "git status",
                with_network: false,
            },
            TurnStep::Answer("done"),
            TurnStep::Completed { usage: None },
        ])
        .into_command();

    // No decider → inbound requests are drained-and-dropped; either way they are
    // not chat output.
    let session = CodexAppServerSession::connect(cmd, &client_info(), thread_config())
        .await
        .unwrap();

    let outputs = collect_turn(&session, "do a thing").await;

    assert_eq!(
        outputs.len(),
        2,
        "approval-request must NOT be surfaced as chat output: {outputs:?}"
    );
    assert!(matches!(
        &outputs[0],
        OutboundMessage::Text { content, .. } if content == "done"
    ));
    assert!(matches!(&outputs[1], OutboundMessage::Result { .. }));
}

// ── E21-8 (#4870): FAIL-CLOSED approval round-trip over the real transport ────

mod approval_round_trip {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use async_trait::async_trait;
    use maekon_core::codex_approval::CodexApprovalDecider;
    use maekon_core::models::audit::{AuditEntry, AuditLevel, AuditStats, AuditStatus};
    use maekon_core::ports::audit_log::AuditLogPort;
    use maekon_core::ports::codex_approval::{
        ApprovalPolicyPort, DefaultDenyUiHook, PolicyVerdict,
    };
    use maekon_network::codex_app_server_session::CodexAppServerSession;

    /// Records every audited event for assertions (a faithful sink, not theater).
    #[derive(Default)]
    struct RecordingAudit {
        events: Mutex<Vec<(String, String)>>,
    }
    #[async_trait]
    impl AuditLogPort for RecordingAudit {
        async fn pending_count(&self) -> usize {
            self.events.lock().unwrap().len()
        }
        async fn recent_entries(&self, _l: usize) -> Vec<AuditEntry> {
            Vec::new()
        }
        async fn entries_by_status(&self, _s: &AuditStatus, _l: usize) -> Vec<AuditEntry> {
            Vec::new()
        }
        async fn entries_by_action_prefix(&self, _p: &str, _l: usize) -> Vec<AuditEntry> {
            Vec::new()
        }
        async fn stats(&self) -> AuditStats {
            AuditStats {
                total: 0,
                completed: 0,
                failed: 0,
                denied: 0,
                timeout: 0,
            }
        }
        async fn has_pending_batch(&self) -> bool {
            false
        }
        async fn log_event(&self, action_type: &str, _s: &str, details: &str) {
            self.events
                .lock()
                .unwrap()
                .push((action_type.to_string(), details.to_string()));
        }
        async fn log_start_if(&self, _l: AuditLevel, _c: &str, _s: &str, _a: &str) {}
        async fn log_complete_with_time(
            &self,
            _l: AuditLevel,
            _c: &str,
            _s: &str,
            _d: &str,
            _e: u64,
        ) {
        }
        async fn drain_batch(&self) -> Vec<AuditEntry> {
            Vec::new()
        }
        async fn drain_all(&self) -> Vec<AuditEntry> {
            Vec::new()
        }
        async fn entries_by_command_id(&self, _c: &str, _l: usize) -> Vec<AuditEntry> {
            Vec::new()
        }
    }

    /// A policy port returning a fixed verdict (or error).
    struct FixedPolicy(Result<PolicyVerdict, String>);
    #[async_trait]
    impl ApprovalPolicyPort for FixedPolicy {
        async fn verdict_for(&self, _p: &str, _a: &[String]) -> Result<PolicyVerdict, String> {
            self.0.clone()
        }
    }

    fn make_decider(
        verdict: Result<PolicyVerdict, String>,
        audit: Arc<RecordingAudit>,
    ) -> Arc<CodexApprovalDecider> {
        Arc::new(CodexApprovalDecider::new(
            Arc::new(FixedPolicy(verdict)),
            audit,
            Arc::new(DefaultDenyUiHook),
        ))
    }

    /// Poll `$MAEKON_REQ_LOG` for the client's RESPONSE to `approval_id` and
    /// return the parsed line, or panic after a generous budget.
    async fn await_response(log: &std::path::Path, approval_id: u64) -> serde_json::Value {
        let needle = format!("\"id\":{approval_id}");
        for _ in 0..200 {
            if let Ok(contents) = std::fs::read_to_string(log) {
                if let Some(line) = contents.lines().find(|l| {
                    l.contains(&needle) && (l.contains("\"result\"") || l.contains("\"error\""))
                }) {
                    return serde_json::from_str(line)
                        .unwrap_or_else(|e| panic!("response line is valid JSON: {e}: {line}"));
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!(
            "no client response to approval id {approval_id} in {log:?}:\n{}",
            std::fs::read_to_string(log).unwrap_or_default()
        );
    }

    fn approval_turn(approval_id: u64, command: &'static str, with_network: bool) -> Vec<TurnStep> {
        vec![
            TurnStep::ApprovalRequest {
                method: "item/commandExecution/requestApproval",
                approval_id,
                command,
                with_network,
            },
            TurnStep::Answer("done"),
            TurnStep::Completed { usage: None },
        ]
    }

    #[tokio::test]
    async fn round_trip_auto_accept() {
        // An Auto policy → the client writes {"id":N,"result":{"decision":"accept"}}.
        // Proves the reverse-request is no longer swallowed AND the response
        // correlates by id.
        let req_log = unique_request_log("approval_auto_accept");
        let audit = Arc::new(RecordingAudit::default());
        let decider = make_decider(
            Ok(PolicyVerdict::Auto {
                policy_id: "p1".to_string(),
                allow_network: false,
            }),
            audit.clone(),
        );
        let cmd = FakeCodexAppServer::new()
            .with_request_log(req_log.clone())
            .turn(approval_turn(9101, "git status", false))
            .into_command();

        let session = CodexAppServerSession::connect_with_approval(
            cmd,
            &client_info(),
            thread_config(),
            Some(decider),
        )
        .await
        .unwrap();

        let _ = collect_turn(&session, "do a thing").await;
        let resp = await_response(&req_log, 9101).await;
        assert_eq!(resp["id"], 9101);
        assert_eq!(resp["result"]["decision"], "accept");

        // Audit recorded the approval with the process name retained.
        let events = audit.events.lock().unwrap();
        assert!(
            events
                .iter()
                .any(|(a, d)| a.contains("approved") && d.contains("process_name=git")),
            "decision audited with process name: {events:?}"
        );
        let _ = std::fs::remove_file(&req_log);
    }

    #[tokio::test]
    async fn round_trip_fail_closed_decline_on_no_match() {
        // No policy match + DefaultDenyUiHook → denied (NOT silently approved).
        let req_log = unique_request_log("approval_no_match");
        let audit = Arc::new(RecordingAudit::default());
        let decider = make_decider(Ok(PolicyVerdict::NoMatch), audit);
        let cmd = FakeCodexAppServer::new()
            .with_request_log(req_log.clone())
            .turn(approval_turn(9102, "git status", false))
            .into_command();

        let session = CodexAppServerSession::connect_with_approval(
            cmd,
            &client_info(),
            thread_config(),
            Some(decider),
        )
        .await
        .unwrap();

        let _ = collect_turn(&session, "do a thing").await;
        let resp = await_response(&req_log, 9102).await;
        assert_eq!(resp["result"]["decision"], "decline");
        let _ = std::fs::remove_file(&req_log);
    }

    #[tokio::test]
    async fn round_trip_fail_closed_on_policy_error() {
        // A PolicyClient error must NEVER become approved → denied.
        let req_log = unique_request_log("approval_policy_error");
        let audit = Arc::new(RecordingAudit::default());
        let decider = make_decider(Err("policy backend down".to_string()), audit);
        let cmd = FakeCodexAppServer::new()
            .with_request_log(req_log.clone())
            .turn(approval_turn(9103, "git status", false))
            .into_command();

        let session = CodexAppServerSession::connect_with_approval(
            cmd,
            &client_info(),
            thread_config(),
            Some(decider),
        )
        .await
        .unwrap();

        let _ = collect_turn(&session, "do a thing").await;
        let resp = await_response(&req_log, 9103).await;
        assert_eq!(resp["result"]["decision"], "decline");
        let _ = std::fs::remove_file(&req_log);
    }

    #[tokio::test]
    async fn round_trip_dangerous_command_blocked_even_under_auto() {
        // `rm -rf /` with an Auto policy → still denied (dangerous screen precedes
        // policy). Uses the harness's configurable dangerous payload.
        let req_log = unique_request_log("approval_dangerous");
        let audit = Arc::new(RecordingAudit::default());
        let decider = make_decider(
            Ok(PolicyVerdict::Auto {
                policy_id: "p1".to_string(),
                allow_network: true,
            }),
            audit,
        );
        let cmd = FakeCodexAppServer::new()
            .with_request_log(req_log.clone())
            .turn(approval_turn(9104, "rm -rf /", false))
            .into_command();

        let session = CodexAppServerSession::connect_with_approval(
            cmd,
            &client_info(),
            thread_config(),
            Some(decider),
        )
        .await
        .unwrap();

        let _ = collect_turn(&session, "do a thing").await;
        let resp = await_response(&req_log, 9104).await;
        assert_eq!(
            resp["result"]["decision"], "decline",
            "dangerous command must be denied even under an Auto policy"
        );
        let _ = std::fs::remove_file(&req_log);
    }

    #[tokio::test]
    async fn round_trip_network_default_deny_without_opt_in() {
        // networkApprovalContext + policy.allow_network=false → denied.
        let req_log = unique_request_log("approval_network_deny");
        let audit = Arc::new(RecordingAudit::default());
        let decider = make_decider(
            Ok(PolicyVerdict::Auto {
                policy_id: "p1".to_string(),
                allow_network: false,
            }),
            audit,
        );
        let cmd = FakeCodexAppServer::new()
            .with_request_log(req_log.clone())
            .turn(approval_turn(9105, "curl example.com", true))
            .into_command();

        let session = CodexAppServerSession::connect_with_approval(
            cmd,
            &client_info(),
            thread_config(),
            Some(decider),
        )
        .await
        .unwrap();

        let _ = collect_turn(&session, "do a thing").await;
        let resp = await_response(&req_log, 9105).await;
        assert_eq!(resp["result"]["decision"], "decline");
        let _ = std::fs::remove_file(&req_log);
    }

    #[tokio::test]
    async fn round_trip_network_allowed_with_opt_in() {
        // networkApprovalContext + policy.allow_network=true + Auto → approved.
        let req_log = unique_request_log("approval_network_allow");
        let audit = Arc::new(RecordingAudit::default());
        let decider = make_decider(
            Ok(PolicyVerdict::Auto {
                policy_id: "p1".to_string(),
                allow_network: true,
            }),
            audit,
        );
        let cmd = FakeCodexAppServer::new()
            .with_request_log(req_log.clone())
            .turn(approval_turn(9106, "curl example.com", true))
            .into_command();

        let session = CodexAppServerSession::connect_with_approval(
            cmd,
            &client_info(),
            thread_config(),
            Some(decider),
        )
        .await
        .unwrap();

        let _ = collect_turn(&session, "do a thing").await;
        let resp = await_response(&req_log, 9106).await;
        assert_eq!(resp["result"]["decision"], "accept");
        let _ = std::fs::remove_file(&req_log);
    }
}

// ── E21-4: multi-turn reuses thread without resending history ─────────────────

#[tokio::test]
async fn multi_turn_reuses_thread_without_resending_history() {
    // Two send_message calls on one connected session must both target the SAME
    // persistent threadId, and turn 2's turn/start input must contain ONLY the
    // new message (no turn-1 history replay — the server owns context).
    let log = unique_turn_log("multi_turn_no_resend");
    let cmd = FakeCodexAppServer::new()
        .thread_id("thr_42")
        .with_turn_log(log.clone())
        .turn(vec![
            TurnStep::Answer("ack"),
            TurnStep::Completed { usage: None },
        ])
        .into_command();

    let session = CodexAppServerSession::connect(cmd, &client_info(), thread_config())
        .await
        .expect("connect");

    collect_turn(&session, "first question").await;
    collect_turn(&session, "second question").await;

    let logged = read_log(&log);
    let lines: Vec<&str> = logged.lines().collect();
    assert_eq!(lines.len(), 2, "exactly two turn/start requests: {logged}");
    assert!(
        lines[0].contains(r#""threadId":"thr_42""#),
        "turn 1 targets thr_42: {}",
        lines[0]
    );
    assert!(
        lines[1].contains(r#""threadId":"thr_42""#),
        "turn 2 targets thr_42: {}",
        lines[1]
    );
    assert!(
        lines[1].contains("second question"),
        "turn 2 sends the new message: {}",
        lines[1]
    );
    assert!(
        !lines[1].contains("first question"),
        "turn 2 must NOT re-send turn-1 history: {}",
        lines[1]
    );
    let _ = std::fs::remove_file(&log);
}

// ── R2 (#24347): transport close -> whole child-tree reap ────────────────────

#[tokio::test]
async fn transport_close_reaps_whole_child_tree() {
    // Dropping the session reaps the ENTIRE child group: a long-lived grandchild
    // spawned by the mock is killed, not orphaned (process-GROUP reap, not a
    // direct-child kill). Integration-level R2 regression through the public
    // AppServerProcess surface.
    fn alive(pid: i32) -> bool {
        // signal 0 = existence check.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    // Hand-built script (the grandchild-PID side channel does not fit the
    // builder's request/reply shape): production `connect` routes the child's
    // stderr to null and uses stdout for the JSONL channel, so the mock cannot
    // print the grandchild PID over either. Instead it writes the grandchild PID
    // into a temp file the test polls. The mock completes the initialize
    // handshake, spawns a long-lived grandchild, publishes its PID, then blocks.
    let pid_file = unique_turn_log("reap_grandchild_pid");
    let mut cmd = tokio::process::Command::new("sh");
    cmd.env("MAEKON_PID_FILE", &pid_file).arg("-c").arg(
        r#"IFS= read -r line
id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
printf '{"id":%s,"result":{"userAgent":"fake/1"}}\n' "$id"
sleep 120 &
echo "$!" > "$MAEKON_PID_FILE"
IFS= read -r _ignored
wait"#,
    );

    let (process, _notifications, _inbound) = AppServerProcess::connect(cmd, &client_info())
        .await
        .expect("connect + handshake against the reap mock");

    // Wait for the grandchild PID file to appear (generous budget so a
    // CPU-saturated runner does not starve the poll before the mock publishes).
    let mut grandchild_pid = 0i32;
    for _ in 0..100 {
        if let Ok(s) = std::fs::read_to_string(&pid_file) {
            if let Ok(pid) = s.trim().parse::<i32>() {
                grandchild_pid = pid;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(grandchild_pid > 0, "mock must publish its grandchild PID");
    assert!(
        alive(grandchild_pid),
        "grandchild should be alive before reap"
    );

    drop(process);

    // Poll up to ~5s for the grandchild to fully disappear (SIGKILL + reparent).
    // The generous budget keeps the reap assertion deterministic even when a
    // busy runner delays signal delivery / process reaping under CPU pressure.
    let mut gone = false;
    for _ in 0..100 {
        if !alive(grandchild_pid) {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        gone,
        "transport close must reap the whole child group, not orphan the grandchild (#24347)"
    );
    let _ = std::fs::remove_file(&pid_file);
}

// ── R4 (AC3): initialize request CONTRACT SNAPSHOT (outbound drift detector) ──

#[tokio::test]
async fn initialize_request_contract_snapshot() {
    // Freeze the OUTBOUND initialize wire contract: method=="initialize",
    // params=={clientInfo:{name,title,version}}, followed by an "initialized"
    // notification (no id, empty params). Any rename/add/remove of a wire key
    // breaks this test — the AC3 drift signal. (wire_contract_snapshot.rs style.)
    let req_log = unique_request_log("initialize_contract");
    let cmd = FakeCodexAppServer::new()
        .with_request_log(req_log.clone())
        .into_command();

    let _session = CodexAppServerSession::connect(cmd, &client_info(), thread_config())
        .await
        .expect("connect");

    let logged = read_log(&req_log);
    let lines: Vec<&str> = logged.lines().collect();

    // First inbound line is the initialize request.
    let init: serde_json::Value =
        serde_json::from_str(lines[0]).expect("initialize line is valid JSON");
    assert_eq!(
        init["method"], "initialize",
        "contract drift — initialize method renamed; renames require an RFC PR. got: {}",
        lines[0]
    );
    let client_info_keys: std::collections::BTreeSet<String> = init["params"]["clientInfo"]
        .as_object()
        .expect("clientInfo object")
        .keys()
        .cloned()
        .collect();
    let expected_keys: std::collections::BTreeSet<String> = ["name", "title", "version"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        client_info_keys, expected_keys,
        "contract drift — clientInfo key set changed (expected name/title/version); \
         renames/additions require an RFC PR. got: {}",
        lines[0]
    );
    assert!(
        init.get("id").is_some(),
        "initialize is a request and must carry an id: {}",
        lines[0]
    );

    // The `initialized` notification must follow: a method, no id, empty params.
    let initialized_line = lines
        .iter()
        .find(|l| l.contains(r#""initialized""#))
        .expect("an `initialized` notification was sent");
    let initialized: serde_json::Value =
        serde_json::from_str(initialized_line).expect("initialized line is valid JSON");
    assert_eq!(initialized["method"], "initialized");
    assert!(
        initialized.get("id").is_none(),
        "contract drift — the initialized notification must NOT carry an id: {initialized_line}"
    );
    assert_eq!(
        initialized["params"],
        serde_json::json!({}),
        "contract drift — initialized params must be empty: {initialized_line}"
    );
    let _ = std::fs::remove_file(&req_log);
}

// ── R4 (AC3): accepted userAgent parse + forward-compat (inbound drift) ───────

#[tokio::test]
async fn accepted_useragent_parses_to_some_exact_string() {
    // Freeze the INBOUND parse contract: result.userAgent parses to Some(exact)
    // and the raw round-trips. A future response-key rename (userAgent ->
    // user_agent / nesting) would silently make user_agent None and fail here.
    let (process, _notes, _inbound) = AppServerProcess::connect(
        FakeCodexAppServer::new()
            .user_agent(Some("codex-app-server/7.7"))
            .into_command(),
        &client_info(),
    )
    .await
    .expect("connect");

    assert_eq!(
        process.server_info().user_agent.as_deref(),
        Some("codex-app-server/7.7"),
        "contract drift — userAgent no longer parses; response-key renames require an RFC PR"
    );
    assert_eq!(
        process.server_info().raw["userAgent"],
        "codex-app-server/7.7",
        "raw must retain the userAgent verbatim"
    );
}

#[tokio::test]
async fn unknown_initialize_result_fields_are_retained_forward_compat() {
    // Forward-compat promise: extra unknown sibling fields in the initialize
    // result do NOT break connect; userAgent still parses and raw retains them.
    let (process, _notes, _inbound) = AppServerProcess::connect(
        FakeCodexAppServer::new()
            .user_agent(Some("codex-app-server/7.7"))
            .handshake(Handshake::OkWithExtraFields)
            .into_command(),
        &client_info(),
    )
    .await
    .expect("connect tolerates unknown result fields");

    assert_eq!(
        process.server_info().user_agent.as_deref(),
        Some("codex-app-server/7.7")
    );
    assert_eq!(
        process.server_info().raw["futureField"]["x"],
        1,
        "raw must retain unknown forward-compat fields"
    );
}

// ── R4 (AC3): unsupported/garbage handling -> #4871 fallback input (pure test) ─

#[tokio::test]
async fn broken_handshake_surfaces_error_for_exec_fallback_without_hang() {
    // PURE-TEST R4 (no production version-gate): a broken handshake (JSON-RPC
    // error to initialize) makes connect return Err WITHOUT panic/hang — exactly
    // the input #4871's `create_codex_session_with_fallback` degrades to
    // `codex exec` on. A timeout wraps it as a hang guard.
    let cmd = FakeCodexAppServer::new()
        .handshake(Handshake::RpcError {
            code: -32000,
            msg: "unsupported".to_string(),
        })
        .into_command();

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        CodexAppServerSession::connect(cmd, &client_info(), thread_config()),
    )
    .await
    .expect("connect must resolve (not hang) on a broken handshake");

    let err = result.err().expect("a broken handshake must Err, not Ok");
    assert!(
        matches!(
            err,
            CoreError::Network { .. } | CoreError::RequestTimeout { .. }
        ),
        "expected a transport-level error feeding the #4871 exec fallback, got {err:?}"
    );
}

#[tokio::test]
async fn garbage_useragent_is_accepted_today_not_rejected() {
    // CURRENT-TRUTH baseline (no version gate exists): a well-formed initialize
    // result with a garbage/unknown userAgent is ACCEPTED — connect returns Ok
    // and user_agent reflects the garbage. This documents that this path would
    // NOT trigger the #4871 exec fallback today, giving a future gate a clear
    // before/after baseline. (It asserts current truth, not a desired reject.)
    let (process, _notes, _inbound) = AppServerProcess::connect(
        FakeCodexAppServer::new()
            .user_agent(Some("garbage-not-a-version/x"))
            .into_command(),
        &client_info(),
    )
    .await
    .expect("a well-formed garbage userAgent is currently accepted, not rejected");

    assert_eq!(
        process.server_info().user_agent.as_deref(),
        Some("garbage-not-a-version/x")
    );
}

#[tokio::test]
async fn absent_useragent_is_accepted_as_none() {
    // Omitting userAgent entirely is accepted (parses to None) without panic —
    // the infallible-parse boundary the snapshot freezes.
    let (process, _notes, _inbound) = AppServerProcess::connect(
        FakeCodexAppServer::new().user_agent(None).into_command(),
        &client_info(),
    )
    .await
    .expect("a missing userAgent is accepted as None");

    assert_eq!(process.server_info().user_agent, None);
}

// ── E21 #4868 Part 1: structured account/read auth probe (wire round-trip) ───
//
// These drive the REAL AppServerProcess::connect + request("account/read")
// against the extended fake harness, asserting the WIRE response shape per
// scenario. The production response→status MAPPING (parse_account_read_status)
// is unit-tested in src-tauri's subprocess_provider::auth_probe; here we prove
// the end-to-end spawn → initialize → account/read round-trip and the exact JSON
// the probe will parse. The probe sends params {"refreshToken":false} (read-only)
// — never the ADR-025-legal-gated refreshToken:true.

async fn account_read_response(behavior: AccountReadBehavior) -> serde_json::Value {
    let cmd = FakeCodexAppServer::new()
        .account_read(behavior)
        .into_command();
    let (process, _notifications, _inbound) = AppServerProcess::connect(cmd, &client_info())
        .await
        .expect("connect + handshake before account/read");
    process
        .request("account/read", serde_json::json!({"refreshToken": false}))
        .await
        .expect("account/read round-trip")
}

#[tokio::test]
async fn account_read_chatgpt_wire_shape() {
    let value = account_read_response(AccountReadBehavior::Chatgpt {
        email: "alice@example.com",
        plan_type: "pro",
    })
    .await;
    assert_eq!(value["account"]["type"], "chatgpt");
    assert_eq!(value["requiresOpenaiAuth"], true);
    // The email is on the wire (the CLI owns it) — the production probe must NOT
    // surface it. The probe-side PII guard is unit-tested in auth_probe.
    assert_eq!(value["account"]["email"], "alice@example.com");
}

#[tokio::test]
async fn account_read_apikey_wire_shape() {
    let value = account_read_response(AccountReadBehavior::ApiKey).await;
    assert_eq!(value["account"]["type"], "apiKey");
    assert_eq!(value["requiresOpenaiAuth"], true);
}

#[tokio::test]
async fn account_read_not_logged_in_wire_shape() {
    let value = account_read_response(AccountReadBehavior::NotLoggedIn).await;
    assert!(value["account"].is_null());
    assert_eq!(value["requiresOpenaiAuth"], true);
}

#[tokio::test]
async fn account_read_no_auth_needed_wire_shape() {
    let value = account_read_response(AccountReadBehavior::NoAuthNeeded).await;
    assert!(value["account"].is_null());
    assert_eq!(value["requiresOpenaiAuth"], false);
}

#[tokio::test]
async fn account_read_amazon_bedrock_wire_shape() {
    let value = account_read_response(AccountReadBehavior::AmazonBedrock).await;
    assert_eq!(value["account"]["type"], "amazonBedrock");
}

#[tokio::test]
async fn account_read_unknown_shape_round_trips() {
    let value = account_read_response(AccountReadBehavior::UnknownShape).await;
    assert_eq!(value["account"]["type"], "futuretype");
    // requiresOpenaiAuth absent → the probe degrades to Unknown (unit-tested).
    assert!(value.get("requiresOpenaiAuth").is_none());
}

#[tokio::test]
async fn account_read_rpc_error_surfaces_transport_error() {
    // A JSON-RPC error reply to account/read → request() returns Err. The probe
    // maps this to Unknown (unit-tested); here we prove the round-trip errors
    // rather than panics, and the child is reaped on drop.
    let cmd = FakeCodexAppServer::new()
        .account_read(AccountReadBehavior::RpcError {
            code: -32000,
            msg: "account read failed",
        })
        .into_command();
    let (process, _notifications, _inbound) = AppServerProcess::connect(cmd, &client_info())
        .await
        .expect("connect + handshake before account/read");
    let rpc_err = process
        .request("account/read", serde_json::json!({"refreshToken": false}))
        .await
        .unwrap_err();
    assert!(
        matches!(
            rpc_err,
            maekon_network::codex_app_server::TransportError::Rpc(_)
        ),
        "account/read RPC error must surface as TransportError::Rpc; got: {rpc_err:?}"
    );
}

/// E21 #4863 R7 harness-seam contract: the EXECUTABLE form of the fake
/// (`write_executable`) must answer BOTH `--version` (the factory's pre-spawn
/// trust probe) AND the `app-server` JSON-RPC dialog with the SAME binary,
/// branching on `argv[1]`. This pins the seam the R7 `--version` probe relies on
/// — a binary that cannot answer `--version` is treated as untrusted — and keeps
/// `version_line`/`write_executable` load-bearing (not dead harness code).
#[tokio::test]
async fn fake_executable_answers_version_probe_and_app_server_dialog() {
    let dir = tempfile::tempdir().expect("tempdir");
    let exe_path = dir.path().join("codex");
    let fake = FakeCodexAppServer::new().version_line(Some("codex-cli 1.2.3"));
    fake.write_executable(&exe_path).expect("write executable");

    // (a) `--version` branch: the binary prints a parseable version + exits 0.
    let version_out = std::process::Command::new(&exe_path)
        .arg("--version")
        .output()
        .expect("run --version");
    assert!(version_out.status.success(), "--version must exit 0");
    let stdout = String::from_utf8_lossy(&version_out.stdout);
    assert_eq!(
        maekon_network::codex_app_server::parse_user_agent_version(&stdout),
        Some("1.2.3".to_string()),
        "the --version output must carry a probe-extractable version token"
    );

    // (b) `app-server` branch: the SAME binary drives the JSON-RPC handshake +
    // a one-turn round-trip via the REAL AppServerProcess/CodexAppServerSession.
    let mut cmd = tokio::process::Command::new(&exe_path);
    cmd.arg("app-server");
    let session = CodexAppServerSession::connect(
        cmd,
        &client_info(),
        ThreadConfig {
            model: "gpt-5.4".to_string(),
            cwd: None,
            sandbox_policy: Some("read-only".to_string()),
            approval_policy: None,
        },
    )
    .await
    .expect("the executable form must also serve the app-server dialog");
    assert_eq!(session.session_id(), "thr_42");
    // The captured userAgent (initialize result) carries the harness default.
    assert_eq!(
        session.server_info().user_agent.as_deref(),
        Some("codex-app-server/1.0")
    );
}

// ── E21-5017: turn/steer + turn/interrupt mid-turn control round-trip ─────────
//
// Drive the REAL CodexAppServerSession steer()/interrupt() against a SPAWNED
// mock whose turn/start surfaces a turn id (turn_99) and which logs the
// turn/steer + turn/interrupt request lines to $MAEKON_REQ_LOG. Proves the wire
// send carries threadId + (turnId|expectedTurnId) + (for steer) the input text,
// and that an interrupt/steer BEFORE any turn returns InvalidArguments.
mod turn_control_round_trip {
    use super::*;

    /// Poll $MAEKON_REQ_LOG for a line containing `method` and return it, or
    /// panic after a generous budget (mirrors the approval await_response).
    async fn await_request_line(log: &std::path::Path, method: &str) -> String {
        let needle = format!("\"{method}\"");
        for _ in 0..200 {
            if let Ok(contents) = std::fs::read_to_string(log) {
                if let Some(line) = contents.lines().find(|l| l.contains(&needle)) {
                    return line.to_string();
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!(
            "no {method} request line in {log:?}:\n{}",
            std::fs::read_to_string(log).unwrap_or_default()
        );
    }

    #[tokio::test]
    async fn interrupt_round_trip_sends_thread_and_turn_id() {
        let req_log = unique_request_log("turn_interrupt_rt");
        let cmd = FakeCodexAppServer::new()
            .turn_id("turn_99")
            .with_request_log(req_log.clone())
            .into_command();
        let session = CodexAppServerSession::connect(cmd, &client_info(), thread_config())
            .await
            .expect("connect");

        // Start a turn but DO NOT drain it — keeps current_turn_id live so the
        // interrupt has a target (turn/completed would clear it).
        let _stream = session
            .send_message(&user_msg("work"))
            .await
            .expect("turn/start");
        session
            .interrupt()
            .await
            .expect("interrupt sends on the wire");

        let line = await_request_line(&req_log, "turn/interrupt").await;
        assert!(
            line.contains(r#""threadId":"thr_42""#),
            "carries threadId: {line}"
        );
        assert!(
            line.contains(r#""turnId":"turn_99""#),
            "carries turnId: {line}"
        );
        let _ = std::fs::remove_file(&req_log);
    }

    #[tokio::test]
    async fn steer_round_trip_sends_expected_turn_id_and_input_text() {
        let req_log = unique_request_log("turn_steer_rt");
        let cmd = FakeCodexAppServer::new()
            .turn_id("turn_99")
            .with_request_log(req_log.clone())
            .into_command();
        let session = CodexAppServerSession::connect(cmd, &client_info(), thread_config())
            .await
            .expect("connect");

        let _stream = session
            .send_message(&user_msg("work"))
            .await
            .expect("turn/start");
        session
            .steer(&user_msg("actually do X"))
            .await
            .expect("steer sends on the wire");

        let line = await_request_line(&req_log, "turn/steer").await;
        assert!(
            line.contains(r#""expectedTurnId":"turn_99""#),
            "carries expectedTurnId: {line}"
        );
        assert!(
            line.contains("actually do X"),
            "carries the input text: {line}"
        );
        // #4861 epic review: the v2 UserInput text item REQUIRES `text_elements`
        // (turn/start has it; turn/steer must too, or a real server rejects the
        // steer with a deserialize error). Guards the epic-review fix.
        assert!(
            line.contains(r#""text_elements":[]"#) || line.contains(r#""text_elements": []"#),
            "turn/steer input item must carry text_elements (v2 UserInput): {line}"
        );
        // R6 / is_external preserved: NO sandbox/permission/escalation field.
        let lower = line.to_ascii_lowercase();
        assert!(!lower.contains("sandbox"), "no sandbox field: {line}");
        assert!(!lower.contains("permission"), "no permission field: {line}");
        assert!(!lower.contains("escalat"), "no escalation field: {line}");
        let _ = std::fs::remove_file(&req_log);
    }

    #[tokio::test]
    async fn steer_and_interrupt_before_turn_are_invalid_arguments() {
        let cmd = FakeCodexAppServer::new().turn_id("turn_99").into_command();
        let session = CodexAppServerSession::connect(cmd, &client_info(), thread_config())
            .await
            .expect("connect");

        let err = session.interrupt().await.expect_err("no turn → Err");
        assert!(matches!(err, CoreError::InvalidArguments { .. }));
        let err = session
            .steer(&user_msg("x"))
            .await
            .expect_err("no turn → Err");
        assert!(matches!(err, CoreError::InvalidArguments { .. }));
    }
}

// ── E21 #4863 AC1: live smoke against the REAL installed `codex` CLI ──────────
//
// IGNORED by default. This is the one acceptance criterion CI cannot satisfy: it
// requires an installed, ChatGPT- (or API-key-) authenticated `codex` binary on
// PATH, spends real subscription quota, and EGRESSES a prompt to OpenAI. It
// drives maekon's REAL client (`CodexAppServerSession` over `AppServerProcess`)
// — NOT the fake harness — through initialize → thread/start → turn/start →
// turn/completed, proving the maekon↔codex app-server integration end-to-end.
//
// Run (model defaults to the repo catalog value; override to match the local
// `~/.codex/config.toml` `model`):
//   MAEKON_SMOKE_MODEL=gpt-5.5 cargo test -p maekon-network \
//     --test codex_app_server_integration \
//     live_smoke_real_codex_app_server_one_turn -- --ignored --nocapture
#[tokio::test]
#[ignore = "live smoke (#4863 AC1): needs a real authenticated `codex` CLI; spends ChatGPT quota + egresses to OpenAI"]
async fn live_smoke_real_codex_app_server_one_turn() {
    let model = std::env::var("MAEKON_SMOKE_MODEL").unwrap_or_else(|_| "gpt-5.4".to_string());
    let mut cmd = tokio::process::Command::new("codex");
    cmd.args(["app-server"]);
    let config = ThreadConfig {
        model: model.clone(),
        cwd: None,
        sandbox_policy: None,
        approval_policy: None,
    };

    let session = CodexAppServerSession::connect(cmd, &client_info(), config)
        .await
        .expect("connect + initialize + thread/start against the REAL codex app-server");
    eprintln!(
        "SMOKE connect ok: provider={} session_id={} model={model}",
        session.provider_name(),
        session.session_id(),
    );

    let msgs = collect_turn(&session, "Reply with exactly: OK").await;
    eprintln!("SMOKE turn emitted {} message(s):", msgs.len());
    for m in &msgs {
        eprintln!("SMOKE   {m:?}");
    }
    assert!(
        !msgs.is_empty(),
        "a real turn must emit at least one OutboundMessage (turn ran to completion)"
    );
    // AC1 proof: the assistant's answer text must round-trip through maekon's
    // client (not just a bare completion) — guards the v2 streaming-shape fixes
    // (agentMessage/delta `delta` field, thread/start `thread.id`, turn `input`
    // array) that the live smoke surfaced against codex 0.130.0.
    let answer = msgs.iter().find_map(|m| match m {
        OutboundMessage::Text { content, .. } if !content.is_empty() => Some(content.clone()),
        OutboundMessage::Result { content, .. } if !content.is_empty() => Some(content.clone()),
        _ => None,
    });
    eprintln!("SMOKE assistant answer captured: {answer:?}");
    assert!(
        answer.is_some(),
        "a real turn must surface the assistant's answer text (got: {msgs:?})"
    );
}

// #5071 live verification: drive turn/steer + turn/interrupt against the REAL
// codex 0.130.0 mid-turn, to confirm the epic-review fixes hold against the real
// binary (turn/steer's `text_elements` was fixed by SCHEMA inspection + a
// shape-guard test, never live-exercised — this closes that gap). IGNORED by
// default; spends ChatGPT quota + egresses. Run:
//   MAEKON_SMOKE_MODEL=<model> cargo test -p maekon-network \
//     --test codex_app_server_integration live_smoke_real_codex_steer_interrupt \
//     -- --ignored --nocapture
#[tokio::test]
#[ignore = "live smoke (#5071): real authenticated codex; spends ChatGPT quota + egress"]
async fn live_smoke_real_codex_steer_interrupt() {
    let model = std::env::var("MAEKON_SMOKE_MODEL").unwrap_or_else(|_| "gpt-5.4".to_string());
    let mut cmd = tokio::process::Command::new("codex");
    cmd.args(["app-server"]);
    let config = ThreadConfig {
        model,
        cwd: None,
        sandbox_policy: None,
        approval_policy: None,
    };
    let session = CodexAppServerSession::connect(cmd, &client_info(), config)
        .await
        .expect("connect against real codex app-server");

    // A long-generation prompt so the turn is reliably in-flight when we steer.
    // send_message captures the turn id synchronously from the turn/start result,
    // so steer/interrupt below find an active turn. We do NOT drain the stream;
    // steer/interrupt are separate request/response correlated by the read loop.
    let _stream = session
        .send_message(&user_msg(
            "Write a long, detailed 400-word explanation of how TCP congestion control works.",
        ))
        .await
        .expect("turn/start against real codex");

    // The KEY check: real codex must PARSE the request (not a deserialize/SHAPE
    // rejection). A SEMANTIC server error like "no active turn to steer" is a
    // PASS — it proves the server accepted the turn/steer wire shape (incl.
    // `text_elements`) and rejected only on turn STATE. Only a deserialize/shape
    // error (the `text_elements`/UserInput bug) fails the test.
    fn is_shape_rejection(e: &CoreError) -> bool {
        let msg = format!("{e}").to_lowercase();
        msg.contains("expected a sequence")
            || msg.contains("text_elements")
            || msg.contains("invalid type")
            || msg.contains("deserialize")
            || msg.contains("missing field")
            || msg.contains("unknown field")
    }
    // A request reached + was parsed by the server iff it's Ok or a server
    // `rpc error` response (vs a transport/connection error).
    fn reached_server(r: &Result<(), CoreError>) -> bool {
        match r {
            Ok(()) => true,
            Err(e) => format!("{e}").to_lowercase().contains("rpc error"),
        }
    }

    let steer_res = session
        .steer(&user_msg(
            "Actually, ignore that — just reply with the single word DONE.",
        ))
        .await;
    eprintln!("SMOKE steer result: {steer_res:?}");
    if let Err(ref e) = steer_res {
        assert!(
            !is_shape_rejection(e),
            "turn/steer rejected on SHAPE — text_elements/UserInput regressed: {e}"
        );
    }

    let interrupt_res = session.interrupt().await;
    eprintln!("SMOKE interrupt result: {interrupt_res:?}");
    if let Err(ref e) = interrupt_res {
        assert!(
            !is_shape_rejection(e),
            "turn/interrupt rejected on SHAPE: {e}"
        );
    }

    // At least one mid-turn control must have round-tripped to the server (proving
    // the request wire shape was accepted by codex's deserializer) — not both
    // failing on transport.
    assert!(
        reached_server(&steer_res) || reached_server(&interrupt_res),
        "expected turn/steer and/or turn/interrupt to reach + be parsed by real codex \
         (steer={steer_res:?}, interrupt={interrupt_res:?})"
    );
}
