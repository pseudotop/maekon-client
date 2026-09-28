use super::*;

use chrono::Utc;
use maekon_core::config::{AiAccessMode, AiProviderType, ExternalApiEndpoint};
use maekon_core::models::ai_session::{ConversationSessionInfo, SessionState, TokenUsage};

use crate::ai_invocation_evidence::{read_recorded_fingerprint, write_evidence};

const OLLAMA_SURFACE: &str = "provider_surface.ollama.local_http";
const SESSION_ID: &str = "verification-session";

type Log = Arc<parking_lot::Mutex<Vec<String>>>;

enum Reply {
    Frames(Vec<Result<OutboundMessage, CoreError>>),
    /// The provider never answers.
    Silent,
    /// `send_message` itself fails.
    Refused,
}

fn provider_failure() -> CoreError {
    CoreError::ServiceUnavailable {
        code: maekon_core::error_codes::ServiceCode::Unavailable,
        message: "provider unavailable".to_owned(),
    }
}

struct ScriptedSession {
    reply: parking_lot::Mutex<Option<Reply>>,
    sent: parking_lot::Mutex<Vec<SessionMessage>>,
    log: Log,
}

#[async_trait]
impl ConversationSession for ScriptedSession {
    async fn send_message(&self, message: &SessionMessage) -> Result<ResponseStream, CoreError> {
        self.sent.lock().push(message.clone());
        self.log.lock().push("send".to_owned());
        let reply = self.reply.lock().take();
        match reply {
            Some(Reply::Frames(frames)) => Ok(Box::pin(futures::stream::iter(frames))),
            Some(Reply::Silent) => Ok(Box::pin(futures::stream::pending())),
            Some(Reply::Refused) | None => Err(provider_failure()),
        }
    }

    fn info(&self) -> ConversationSessionInfo {
        ConversationSessionInfo {
            session_id: SESSION_ID.to_owned(),
            provider_name: "scripted".to_owned(),
            model: "llama3".to_owned(),
            state: SessionState::Active,
            transport: SessionTransport::HttpApi,
            created_at: Utc::now(),
            last_active: Utc::now(),
            turn_count: 0,
            title: None,
        }
    }

    fn session_id(&self) -> &str {
        SESSION_ID
    }

    fn provider_name(&self) -> &str {
        "scripted"
    }
}

struct FakeHost {
    session: Option<Arc<ScriptedSession>>,
    budget_allows: bool,
    log: Log,
    opened_with: parking_lot::Mutex<Option<SessionConfig>>,
    /// When set, `close` waits for a notification after logging.
    close_gate: Option<Arc<tokio::sync::Notify>>,
}

#[async_trait]
impl VerificationSessionHost for FakeHost {
    async fn open(&self, config: SessionConfig) -> Result<Arc<dyn ConversationSession>, CoreError> {
        self.log.lock().push("open".to_owned());
        *self.opened_with.lock() = Some(config);
        match &self.session {
            Some(session) => Ok(Arc::clone(session) as Arc<dyn ConversationSession>),
            None => Err(provider_failure()),
        }
    }

    async fn token_budget_allows(&self, session_id: &str) -> bool {
        self.log.lock().push(format!("budget {session_id}"));
        self.budget_allows
    }

    async fn record_usage(&self, session_id: &str, input_tokens: u64, output_tokens: u64) {
        self.log
            .lock()
            .push(format!("usage {session_id} {input_tokens}/{output_tokens}"));
    }

    async fn close(&self, session_id: &str) {
        self.log.lock().push(format!("close {session_id}"));
        if let Some(gate) = &self.close_gate {
            gate.notified().await;
        }
    }
}

struct Fixture {
    host: Arc<FakeHost>,
    session: Arc<ScriptedSession>,
    log: Log,
}

fn fixture(reply: Reply, budget_allows: bool, session_opens: bool) -> Fixture {
    let log: Log = Arc::default();
    let session = Arc::new(ScriptedSession {
        reply: parking_lot::Mutex::new(Some(reply)),
        sent: parking_lot::Mutex::default(),
        log: Arc::clone(&log),
    });
    let host = Arc::new(FakeHost {
        session: session_opens.then(|| Arc::clone(&session)),
        budget_allows,
        log: Arc::clone(&log),
        opened_with: parking_lot::Mutex::default(),
        close_gate: None,
    });
    Fixture { host, session, log }
}

fn logged(log: &Log) -> Vec<String> {
    log.lock().clone()
}

fn ollama_config(model: Option<&str>) -> AiProviderConfig {
    AiProviderConfig {
        access_mode: AiAccessMode::ProviderApiKey,
        llm_api: Some(ExternalApiEndpoint {
            endpoint: "http://localhost:11434/v1/responses".to_owned(),
            api_key: String::new(),
            model: model.map(str::to_owned),
            timeout_secs: 30,
            provider_type: AiProviderType::Ollama,
            surface_id: Some(OLLAMA_SURFACE.to_owned()),
            credential: None,
        }),
        ..Default::default()
    }
}

/// The session factory's answer for a no-auth surface.
fn no_auth(_surface_id: &str, _url: &str) -> Option<CredentialSource> {
    Some(CredentialSource::NoAuth)
}

/// The session factory's answer for a surface it will not open.
fn no_credential(_surface_id: &str, _url: &str) -> Option<CredentialSource> {
    None
}

async fn ready_plan() -> VerificationPlan {
    match plan_verification(&ollama_config(Some("llama3")), true, false, no_auth).await {
        Ok(plan) => plan,
        Err(status) => panic!("the no-auth surface must plan, got {status:?}"),
    }
}

fn text(content: &str) -> Result<OutboundMessage, CoreError> {
    Ok(OutboundMessage::Text {
        content: content.to_owned(),
        done: false,
    })
}

fn result(content: &str, usage: Option<(u64, u64)>) -> Result<OutboundMessage, CoreError> {
    Ok(OutboundMessage::Result {
        content: content.to_owned(),
        done: true,
        usage: usage.map(|(input_tokens, output_tokens)| TokenUsage {
            input_tokens,
            output_tokens,
        }),
    })
}

fn previous_record(path: &Path) -> Vec<u8> {
    let fingerprint = format!("sha256:{}", "a".repeat(64));
    write_evidence(path, &fingerprint, Utc::now()).expect("seed evidence");
    std::fs::read(path).expect("read seeded evidence")
}

#[tokio::test]
async fn consent_is_checked_before_anything_else() {
    let unconfigured = AiProviderConfig::default();
    for config in [&unconfigured, &ollama_config(Some("llama3"))] {
        for restart_pending in [false, true] {
            assert!(matches!(
                plan_verification(config, false, restart_pending, no_auth).await,
                Err(ChatHttpVerificationStatus::ConsentRequired)
            ));
        }
    }
}

/// Until a restart applies a saved provider change, the factory answers for
/// the boot-time provider. The plan refuses before it asks.
#[tokio::test]
async fn a_pending_restart_is_refused_before_the_credential_lookup() {
    let asked = parking_lot::Mutex::new(0_u32);
    let counting = |_: &str, _: &str| {
        *asked.lock() += 1;
        Some(CredentialSource::NoAuth)
    };

    assert!(matches!(
        plan_verification(&ollama_config(Some("llama3")), true, true, counting).await,
        Err(ChatHttpVerificationStatus::RestartRequired)
    ));
    assert_eq!(*asked.lock(), 0);
    let plan = plan_verification(&ollama_config(Some("llama3")), true, false, counting)
        .await
        .expect("with the restart applied the plan resolves");
    assert_eq!(
        plan.session_config.surface_id.as_deref(),
        Some(OLLAMA_SURFACE)
    );
    assert_eq!(*asked.lock(), 1);
}

#[tokio::test]
async fn only_a_resolvable_http_path_is_configured() {
    let mut cli = ollama_config(Some("llama3"));
    cli.access_mode = AiAccessMode::ProviderSubscriptionCli;
    let mut local = ollama_config(Some("llama3"));
    local.access_mode = AiAccessMode::LocalModel;
    let mut unnamed = ollama_config(Some("llama3"));
    if let Some(endpoint) = unnamed.llm_api.as_mut() {
        endpoint.surface_id = None;
    }
    let no_endpoint = AiProviderConfig {
        access_mode: AiAccessMode::ProviderApiKey,
        ..Default::default()
    };

    for (config, label) in [
        (&cli, "subscription CLI"),
        (&local, "local model"),
        (&unnamed, "no surface"),
        (&no_endpoint, "no endpoint"),
    ] {
        assert!(
            matches!(
                plan_verification(config, true, false, no_auth).await,
                Err(ChatHttpVerificationStatus::NotConfigured)
            ),
            "{label}"
        );
    }
    assert!(
        matches!(
            plan_verification(&ollama_config(Some("llama3")), true, false, no_credential).await,
            Err(ChatHttpVerificationStatus::NotConfigured)
        ),
        "the session factory has no credential for the surface"
    );
}

#[tokio::test]
async fn the_plan_opens_a_tool_free_session_on_the_fingerprinted_target() {
    let config = ollama_config(Some("llama3"));
    let plan = ready_plan().await;
    let expected = HttpChatTarget::resolve(&config)
        .expect("target")
        .fingerprint(&CredentialSource::NoAuth)
        .await
        .expect("fingerprint");

    assert_eq!(plan.fingerprint, expected);
    let session = &plan.session_config;
    assert_eq!(session.transport, SessionTransport::HttpApi);
    assert_eq!(session.surface_id.as_deref(), Some(OLLAMA_SURFACE));
    assert_eq!(session.model.as_deref(), Some("llama3"));
    assert_eq!(
        session.system_prompt.as_deref(),
        Some(VERIFICATION_SYSTEM_PROMPT)
    );
    assert!(!session.tools_enabled);
    assert!(session.cwd.is_none() && session.sandbox_policy.is_none());
    assert!(session.approval_policy.is_none());
}

#[tokio::test]
async fn a_completed_reply_records_evidence_after_the_budget_check() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("evidence.json");
    previous_record(&path);
    let plan = ready_plan().await;
    let fingerprint = plan.fingerprint.clone();
    let fixture = fixture(
        Reply::Frames(vec![text("O"), text("K"), result("", Some((12, 3)))]),
        true,
        true,
    );

    let status = run_verification(fixture.host.clone(), plan, &path, REPLY_TIMEOUT)
        .await
        .expect("evidence write");

    assert_eq!(status, ChatHttpVerificationStatus::Verified);
    assert_eq!(read_recorded_fingerprint(&path), Some(fingerprint));
    assert_eq!(
        logged(&fixture.log),
        [
            "open".to_owned(),
            format!("budget {SESSION_ID}"),
            "send".to_owned(),
            format!("usage {SESSION_ID} 12/3"),
            format!("close {SESSION_ID}"),
        ]
    );

    let sent = fixture.session.sent.lock().clone();
    let [message] = sent.as_slice() else {
        panic!("exactly one message must be sent, got {}", sent.len());
    };
    assert_eq!(message.role, MessageRole::User);
    assert_eq!(message.content, VERIFICATION_MESSAGE);
    assert!(message.attachments.is_empty());
    assert!(message.tools.is_none() && message.context.is_none());
    assert!(message.response_format.is_none());
    assert!(!message.screen_derived);
    let opened = fixture.host.opened_with.lock().clone().expect("opened");
    assert_eq!(
        opened.system_prompt.as_deref(),
        Some(VERIFICATION_SYSTEM_PROMPT)
    );
}

#[tokio::test]
async fn a_reply_carried_only_by_the_terminal_result_counts() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("evidence.json");
    let fixture = fixture(Reply::Frames(vec![result("OK", None)]), true, true);

    let status = run_verification(
        fixture.host.clone(),
        ready_plan().await,
        &path,
        REPLY_TIMEOUT,
    )
    .await
    .expect("evidence write");

    assert_eq!(status, ChatHttpVerificationStatus::Verified);
    assert!(read_recorded_fingerprint(&path).is_some());
}

#[tokio::test]
async fn failures_are_classified_and_leave_the_record_untouched() {
    let stream_error = || Err(provider_failure());
    let error_frame = || {
        Ok(OutboundMessage::Error {
            code: "rate_limited".to_owned(),
            message: "slow down".to_owned(),
            retryable: true,
        })
    };
    let cases = vec![
        (
            "budget exhausted",
            fixture(Reply::Frames(vec![text("OK")]), false, true),
            ChatHttpVerificationStatus::BudgetExhausted,
        ),
        (
            "session did not open",
            fixture(Reply::Frames(vec![text("OK")]), true, false),
            ChatHttpVerificationStatus::ProviderError,
        ),
        (
            "request refused",
            fixture(Reply::Refused, true, true),
            ChatHttpVerificationStatus::ProviderError,
        ),
        (
            "stream failed after text",
            fixture(Reply::Frames(vec![text("OK"), stream_error()]), true, true),
            ChatHttpVerificationStatus::ProviderError,
        ),
        (
            "error frame",
            fixture(Reply::Frames(vec![text("OK"), error_frame()]), true, true),
            ChatHttpVerificationStatus::ProviderError,
        ),
        (
            "blank text",
            fixture(
                Reply::Frames(vec![text("  "), result("", None)]),
                true,
                true,
            ),
            ChatHttpVerificationStatus::ProviderError,
        ),
        (
            "text frames own the body",
            fixture(
                Reply::Frames(vec![text(" "), result("OK", None)]),
                true,
                true,
            ),
            ChatHttpVerificationStatus::ProviderError,
        ),
        (
            "empty stream",
            fixture(Reply::Frames(Vec::new()), true, true),
            ChatHttpVerificationStatus::ProviderError,
        ),
    ];

    for (label, fixture, expected) in cases {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("evidence.json");
        let before = previous_record(&path);
        let session_opens = fixture.host.session.is_some();

        let status = run_verification(
            fixture.host.clone(),
            ready_plan().await,
            &path,
            REPLY_TIMEOUT,
        )
        .await
        .expect("no evidence write");

        assert_eq!(status, expected, "{label}");
        assert_eq!(std::fs::read(&path).expect("read"), before, "{label}");
        let log = logged(&fixture.log);
        let closes = log
            .iter()
            .filter(|entry| entry.starts_with("close"))
            .count();
        assert_eq!(closes, usize::from(session_opens), "{label}: {log:?}");
        if expected == ChatHttpVerificationStatus::BudgetExhausted {
            assert!(!log.contains(&"send".to_owned()), "{label}: {log:?}");
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_silent_provider_times_out_and_the_session_still_closes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("evidence.json");
    let before = previous_record(&path);
    let fixture = fixture(Reply::Silent, true, true);

    let status = run_verification(
        fixture.host.clone(),
        ready_plan().await,
        &path,
        REPLY_TIMEOUT,
    )
    .await
    .expect("no evidence write");

    assert_eq!(status, ChatHttpVerificationStatus::Timeout);
    assert_eq!(std::fs::read(&path).expect("read"), before);
    assert_eq!(
        logged(&fixture.log).last(),
        Some(&format!("close {SESSION_ID}"))
    );
}

/// The result waits for the session to close, so a caller never sees one while
/// the internal session still holds a session slot.
#[tokio::test]
async fn the_result_waits_for_the_session_to_close() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("evidence.json");
    let gate = Arc::new(tokio::sync::Notify::new());
    let mut fixture = fixture(Reply::Frames(vec![text("OK")]), true, true);
    Arc::get_mut(&mut fixture.host)
        .expect("the fixture owns its host")
        .close_gate = Some(Arc::clone(&gate));
    let host = fixture.host.clone();
    let plan = ready_plan().await;
    let verification =
        tokio::spawn(async move { run_verification(host, plan, &path, REPLY_TIMEOUT).await });

    let closed = format!("close {SESSION_ID}");
    for _ in 0..500 {
        if logged(&fixture.log).contains(&closed) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        logged(&fixture.log).contains(&closed),
        "close was never called"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !verification.is_finished(),
        "the verification returned before its session closed"
    );

    gate.notify_one();
    let status = verification
        .await
        .expect("verification task")
        .expect("evidence write");
    assert_eq!(status, ChatHttpVerificationStatus::Verified);
}

/// The production host is the session manager: the budget it checks is the
/// manager's daily allowance, reported usage counts against it, and closing
/// ends the session.
#[cfg(feature = "analysis")]
#[tokio::test]
async fn the_session_manager_hosts_the_verification() {
    use maekon_core::config::AiSessionConfig;
    use maekon_core::ports::conversation_session::SessionManager;

    let manager = Arc::new(SessionManagerImpl::new(
        Arc::new(AiSessionConfig {
            daily_token_budget: 20,
            ..AiSessionConfig::default()
        }),
        Arc::new(crate::auditing_session::tests::MockAudit::default()),
        None,
    ));
    let host: Arc<dyn VerificationSessionHost> = manager.clone();
    let session = host
        .open(ready_plan().await.session_config)
        .await
        .expect("the no-auth surface opens");
    let session_id = session.session_id().to_owned();

    assert!(
        host.token_budget_allows(&session_id).await,
        "nothing spent yet"
    );
    host.record_usage(&session_id, 15, 5).await;
    assert!(
        !host.token_budget_allows(&session_id).await,
        "reported usage spends the daily budget"
    );

    host.close(&session_id).await;
    let Err(error) = manager.get_session(&session_id).await else {
        panic!("the session must be closed");
    };
    assert!(
        matches!(error, CoreError::NotFound { .. }),
        "a closed session is gone, got {error:?}"
    );
}

/// A verification future dropped mid-call (for example by an outer timeout)
/// still closes its internal session.
#[tokio::test]
async fn a_dropped_verification_still_closes_its_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("evidence.json");
    let fixture = fixture(Reply::Silent, true, true);
    let plan = ready_plan().await;

    tokio::time::timeout(
        Duration::from_millis(50),
        run_verification(fixture.host.clone(), plan, &path, REPLY_TIMEOUT),
    )
    .await
    .expect_err("the call must still be pending when it is dropped");

    let closed = format!("close {SESSION_ID}");
    for _ in 0..500 {
        if logged(&fixture.log).contains(&closed) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the session was not closed: {:?}", logged(&fixture.log));
}

#[test]
fn every_status_has_its_wire_name() {
    let cases = [
        (ChatHttpVerificationStatus::Verified, "verified"),
        (ChatHttpVerificationStatus::NotConfigured, "not_configured"),
        (
            ChatHttpVerificationStatus::ConsentRequired,
            "consent_required",
        ),
        (
            ChatHttpVerificationStatus::RestartRequired,
            "restart_required",
        ),
        (
            ChatHttpVerificationStatus::BudgetExhausted,
            "budget_exhausted",
        ),
        (ChatHttpVerificationStatus::ProviderError, "provider_error"),
        (ChatHttpVerificationStatus::Timeout, "timeout"),
    ];
    for (status, wire) in cases {
        assert_eq!(
            serde_json::to_value(ChatHttpVerificationResult { status }).expect("serialize"),
            serde_json::json!({ "status": wire })
        );
    }
}
