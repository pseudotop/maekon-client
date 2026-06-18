//! Codex `app-server` JSON-RPC transport (E21 #4865).
//!
//! The app-server speaks JSON-RPC 2.0 over newline-delimited JSON (JSONL) on
//! stdio, with the `"jsonrpc":"2.0"` envelope field **omitted on the wire**.
//!
//! Layers (all in this module):
//! - framing codec — [`encode_request`] / [`encode_notification`] / [`parse_incoming`]
//! - correlation transport — [`JsonRpcClient`] (responses by `id` vs notifications)
//! - process lifecycle — [`spawn_in_process_group`] / [`reap_process_group`] and
//!   [`AppServerProcess`] (spawn + `initialize` handshake + drop-reap, unix)
//!
//! Still open in #4865: request timeouts/backpressure, idle-timeout graceful
//! shutdown, and a Windows Job Object reap path. Production wiring of an
//! `AppServerProcess` as a `ConversationSession` is #4866.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex};

use crate::mutex_ext::lock_or_recover;

/// A JSON-RPC error object as returned by the app-server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

/// An inbound app-server message: a correlated response to a prior request
/// (carries the request `id`, no `method`), an inbound server→client REQUEST
/// (carries BOTH an `id` AND a `method`, e.g. `requestApproval` — E21 #4870),
/// or an unsolicited notification (a `method`, no `id`).
#[derive(Debug)]
pub enum IncomingMessage {
    Response {
        id: u64,
        outcome: Result<serde_json::Value, RpcError>,
    },
    /// A server-initiated REQUEST that expects a correlated response back from
    /// the client (the reverse direction of [`encode_request`]). The Codex
    /// app-server uses this for `requestApproval` (#4870). Before #4870 this was
    /// MISCLASSIFIED as a `Response{Ok(null)}` and silently dropped.
    Request {
        id: u64,
        method: String,
        params: serde_json::Value,
    },
    Notification {
        method: String,
        params: serde_json::Value,
    },
}

/// A server→client request awaiting a client response (correlated by `id`).
/// Demultiplexed off the read loop onto its own channel so the approval layer
/// can answer it WITHOUT blocking the in-flight turn's response/notification
/// streams (E21 #4870).
#[derive(Debug, Clone)]
pub struct InboundRequest {
    pub id: u64,
    pub method: String,
    pub params: serde_json::Value,
}

/// Encode an outbound request as a single JSONL line. The `jsonrpc` envelope
/// field is intentionally omitted — the app-server wire format does not include
/// it (E21 review I1 / app-server docs).
pub fn encode_request(id: u64, method: &str, params: &serde_json::Value) -> String {
    let value = serde_json::json!({
        "id": id,
        "method": method,
        "params": params,
    });
    // serde_json on an in-memory Value cannot fail to serialize.
    let mut line = serde_json::to_string(&value).expect("serialize request");
    line.push('\n');
    line
}

/// Shape used only to classify an inbound line; fields are all optional so a
/// single deserialize distinguishes response (has `id`) from notification.
#[derive(Deserialize)]
struct RawIncoming {
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<serde_json::Value>,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<RawRpcError>,
}

#[derive(Deserialize)]
struct RawRpcError {
    code: i64,
    message: String,
}

/// Parse one inbound JSONL line into a classified [`IncomingMessage`].
///
/// # Errors
/// Returns a descriptive error string if the line is not valid JSON or does not
/// match either the response (`id` + `result`/`error`) or notification
/// (`method`, no `id`) shape.
pub fn parse_incoming(line: &str) -> Result<IncomingMessage, String> {
    let raw: RawIncoming = serde_json::from_str(line.trim())
        .map_err(|err| format!("invalid app-server JSON line: {err}"))?;

    if let Some(id) = raw.id {
        // KEYSTONE (#4870): an inbound line carrying BOTH an `id` AND a `method`
        // (and NEITHER `result` nor `error`) is a server→client REQUEST, not a
        // response. This branch MUST precede the id→Response branch below — a
        // request also has an `id`, so checking `id`-first without the
        // method/result discriminator would misclassify it as `Response{Ok}` and
        // the read loop would drop it (no pending sender). The discriminator is
        // `method.is_some() && result.is_none() && error.is_none()`.
        if raw.error.is_none() && raw.result.is_none() {
            if let Some(method) = raw.method {
                return Ok(IncomingMessage::Request {
                    id,
                    method,
                    params: raw.params.unwrap_or(serde_json::Value::Null),
                });
            }
        }

        if let Some(error) = raw.error {
            return Ok(IncomingMessage::Response {
                id,
                outcome: Err(RpcError {
                    code: error.code,
                    message: error.message,
                }),
            });
        }
        // A response without an explicit `error` is a success; default to null
        // when `result` is omitted.
        return Ok(IncomingMessage::Response {
            id,
            outcome: Ok(raw.result.unwrap_or(serde_json::Value::Null)),
        });
    }

    match raw.method {
        Some(method) => Ok(IncomingMessage::Notification {
            method,
            params: raw.params.unwrap_or(serde_json::Value::Null),
        }),
        None => Err(
            "app-server line has neither an id (response) nor a method (notification)".to_string(),
        ),
    }
}

// ── Correlation transport (E21 #4865 increment 2) ────────────────────────────

/// An unsolicited app-server notification (streaming event).
#[derive(Debug, Clone)]
pub struct Notification {
    pub method: String,
    pub params: serde_json::Value,
}

/// Failure modes of a JSON-RPC request over the app-server transport.
#[derive(Debug)]
pub enum TransportError {
    /// The server returned a JSON-RPC error object.
    Rpc(RpcError),
    /// The transport closed (process exited / stdout EOF) before responding.
    Closed(String),
    /// No response arrived within the configured per-request timeout.
    Timeout { timeout_ms: u64 },
    /// I/O failure writing the request.
    Io(String),
}

type PendingMap = Arc<StdMutex<HashMap<u64, oneshot::Sender<Result<serde_json::Value, RpcError>>>>>;

/// Long-lived JSON-RPC client over an app-server's stdio. Owns a background read
/// loop that demultiplexes correlated responses (by `id`) from notifications.
/// Process spawning / handshake / PID-group reap land in increment 3 of #4865.
pub struct JsonRpcClient {
    next_id: AtomicU64,
    pending: PendingMap,
    closed: Arc<AtomicBool>,
    writer: AsyncMutex<Box<dyn AsyncWrite + Send + Unpin>>,
    request_timeout: Duration,
}

/// Default per-request timeout when none is configured.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

impl JsonRpcClient {
    /// Wrap an app-server's `reader` (its stdout) and `writer` (its stdin),
    /// spawning the read loop. Returns the client, a receiver of inbound
    /// notifications, and a receiver of inbound server→client REQUESTS (the
    /// reverse-request channel for `requestApproval` — E21 #4870).
    ///
    /// The two receiver channels are independent and unbounded: the read loop
    /// is non-blocking (it `send`s onto whichever channel matches and continues),
    /// so an unanswered approval request can never stall the in-flight turn's
    /// own response/notification delivery.
    pub fn new<R, W>(
        reader: R,
        writer: W,
    ) -> (
        Self,
        mpsc::UnboundedReceiver<Notification>,
        mpsc::UnboundedReceiver<InboundRequest>,
    )
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let pending: PendingMap = Arc::new(StdMutex::new(HashMap::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let (notif_tx, notif_rx) = mpsc::unbounded_channel();
        let (request_tx, request_rx) = mpsc::unbounded_channel();

        let read_pending = pending.clone();
        let read_closed = closed.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) if line.trim().is_empty() => continue,
                    Ok(Some(line)) => match parse_incoming(&line) {
                        Ok(IncomingMessage::Response { id, outcome }) => {
                            if let Some(tx) =
                                lock_or_recover(&read_pending, "codex_app_server.pending")
                                    .remove(&id)
                            {
                                let _ = tx.send(outcome);
                            }
                        }
                        Ok(IncomingMessage::Request { id, method, params }) => {
                            // Non-blocking demux onto the reverse-request channel
                            // (mirrors notif_tx): the approval layer answers it on
                            // a separate task so the turn keeps streaming (#4870).
                            let _ = request_tx.send(InboundRequest { id, method, params });
                        }
                        Ok(IncomingMessage::Notification { method, params }) => {
                            let _ = notif_tx.send(Notification { method, params });
                        }
                        Err(err) => tracing::warn!("app-server parse error: {err}"),
                    },
                    // EOF or read error → transport is gone.
                    Ok(None) | Err(_) => break,
                }
            }
            // Mark closed first so any request that registers after this point
            // is rejected, then drop all pending senders so in-flight requests
            // resolve to `Closed` rather than hanging forever.
            read_closed.store(true, Ordering::SeqCst);
            lock_or_recover(&read_pending, "codex_app_server.pending").clear();
        });

        (
            Self {
                next_id: AtomicU64::new(1),
                pending,
                closed,
                writer: AsyncMutex::new(Box::new(writer)),
                request_timeout: DEFAULT_REQUEST_TIMEOUT,
            },
            notif_rx,
            request_rx,
        )
    }

    /// Override the per-request timeout (default 120s).
    #[must_use]
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Send a JSON-RPC request and await its correlated response.
    pub async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, TransportError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(TransportError::Closed(
                "app-server transport already closed".to_string(),
            ));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        lock_or_recover(&self.pending, "codex_app_server.pending").insert(id, tx);

        // Race guard: if the read loop closed between the check and the insert,
        // remove our sender and fail closed instead of leaking it.
        if self.closed.load(Ordering::SeqCst) {
            lock_or_recover(&self.pending, "codex_app_server.pending").remove(&id);
            return Err(TransportError::Closed(
                "app-server transport closed while sending".to_string(),
            ));
        }

        let line = encode_request(id, method, &params);
        {
            let mut writer = self.writer.lock().await;
            if let Err(err) = writer.write_all(line.as_bytes()).await {
                lock_or_recover(&self.pending, "codex_app_server.pending").remove(&id);
                return Err(TransportError::Io(err.to_string()));
            }
            if let Err(err) = writer.flush().await {
                lock_or_recover(&self.pending, "codex_app_server.pending").remove(&id);
                return Err(TransportError::Io(err.to_string()));
            }
        }

        match tokio::time::timeout(self.request_timeout, rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(rpc))) => Err(TransportError::Rpc(rpc)),
            Ok(Err(_)) => Err(TransportError::Closed(
                "app-server transport closed before responding".to_string(),
            )),
            Err(_elapsed) => {
                // Timed out: drop our pending slot so a late response is ignored.
                lock_or_recover(&self.pending, "codex_app_server.pending").remove(&id);
                Err(TransportError::Timeout {
                    timeout_ms: self.request_timeout.as_millis() as u64,
                })
            }
        }
    }
}

// ── Process-group lifecycle (E21 #4865 increment 3a — R2 / openai/codex#24347) ─
//
// `codex app-server` can spawn its own children (e.g. an MCP server tree). On
// teardown, killing only the direct child orphans that tree (#24347). We spawn
// the app-server as the leader of a new process group and kill the whole group
// on reap, so grandchildren are collected too. Unix-only; Windows would use a
// Job Object (as the automation sandbox already does) — tracked for #4865.

/// Spawn `command` as the leader of a fresh process group so the entire child
/// tree can later be reaped together via [`reap_process_group`].
#[cfg(unix)]
pub fn spawn_in_process_group(
    mut command: tokio::process::Command,
) -> std::io::Result<tokio::process::Child> {
    // process_group(0) → setpgid(0, 0): the child becomes leader of a new group
    // whose pgid equals the child pid.
    command.process_group(0);
    command.kill_on_drop(true);
    command.spawn()
}

/// Best-effort SIGKILL of the child's entire process group (reaps grandchildren
/// that a direct-child kill would orphan — #24347). No-op once the child has
/// been awaited (its pid is gone).
#[cfg(unix)]
pub fn reap_process_group(child: &tokio::process::Child) {
    if let Some(pid) = child.id() {
        // Negative pid → signal the whole process group (pgid == leader pid).
        // SAFETY: kill(2) with a group target and SIGKILL has no memory effects;
        // a stale/exited pid simply yields ESRCH which we ignore.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
}

// ── initialize handshake (E21 #4865 increment 3b) ────────────────────────────

/// Encode an outbound JSON-RPC notification (no `id`, no response expected).
pub fn encode_notification(method: &str, params: &serde_json::Value) -> String {
    let value = serde_json::json!({ "method": method, "params": params });
    let mut line = serde_json::to_string(&value).expect("serialize notification");
    line.push('\n');
    line
}

/// Encode a RESPONSE to a server→client request (E21 #4870), correlated by the
/// originating request `id`. Symmetric with [`encode_request`] /
/// [`encode_notification`]: a success carries `result`, an error carries an
/// `error` object. This codec is DELIBERATELY decision-agnostic — there is no
/// "default to approved" convenience; the approval layer owns the decision and
/// only ever passes a fully-formed payload here.
pub fn encode_response(id: u64, outcome: Result<serde_json::Value, RpcError>) -> String {
    let value = match outcome {
        Ok(result) => serde_json::json!({ "id": id, "result": result }),
        Err(err) => serde_json::json!({
            "id": id,
            "error": { "code": err.code, "message": err.message },
        }),
    };
    let mut line = serde_json::to_string(&value).expect("serialize response");
    line.push('\n');
    line
}

/// Identifies this client to the app-server during `initialize`.
#[derive(Debug, Clone)]
pub struct ClientInfo {
    pub name: String,
    pub title: String,
    pub version: String,
}

/// Subset of the `initialize` response we surface to callers. `raw` retains the
/// full result for forward-compatibility (the app-server may add fields).
#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub user_agent: Option<String>,
    pub raw: serde_json::Value,
}

/// Lenient version-token extraction from an app-server `userAgent` string
/// (E21 #4863 version negotiation). The `initialize` result carries NO
/// `protocolVersion` — only a `userAgent` (e.g. `"codex-app-server/1.2.3 (...)"`)
/// — so the only version signal is whatever token is embedded in that string.
///
/// This is intentionally permissive and NEVER errors: it scans for the first
/// `MAJOR.MINOR.PATCH` token and, failing that, the first `MAJOR.MINOR` token
/// (the in-repo fake harness emits the 2-part `"codex-app-server/1.0"`, so a
/// strict 3-part-only matcher would silently return `None` for the harness and
/// every inform-only check would no-op — a theater check). Returns `None` only
/// when no numeric version token is present at all.
///
/// The caller treats the result as INFORM-ONLY: it is logged and cross-checked
/// against the catalog denylist, but it NEVER gates the session (graceful
/// tolerate-and-fallback, ADR-025 / #4871).
pub fn parse_user_agent_version(user_agent: &str) -> Option<String> {
    // First pass: a full MAJOR.MINOR.PATCH token. Second pass: MAJOR.MINOR.
    // Both are hand-rolled (no regex dep in this crate) digit-run scanners that
    // walk the bytes once and stop at the first qualifying run.
    extract_dotted_number(user_agent, 3).or_else(|| extract_dotted_number(user_agent, 2))
}

/// Scan `text` for the first run of exactly `segments` dot-separated numeric
/// groups (e.g. `segments == 3` → `1.2.3`). The run must be bounded by a
/// non-`[0-9.]` boundary on each side so `1.2.3` is not clipped out of `1.2.34`
/// or matched as `2.3` inside `1.2.3`.
fn extract_dotted_number(text: &str, segments: usize) -> Option<String> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // A candidate can only start at a digit that is not preceded by a digit
        // or a dot (so we anchor on the leading segment boundary).
        let prev_is_boundary = i == 0 || !(bytes[i - 1].is_ascii_digit() || bytes[i - 1] == b'.');
        if prev_is_boundary && bytes[i].is_ascii_digit() {
            if let Some((token, _end)) = match_dotted_run(bytes, i, segments) {
                return Some(token);
            }
            // Advance past this digit run + trailing dots to avoid re-scanning a
            // sub-run of the same number.
            i = skip_number_run(bytes, i);
            continue;
        }
        i += 1;
    }
    None
}

/// Try to match exactly `segments` digit groups separated by single dots,
/// starting at `start`. On success returns the matched token and the end index;
/// the match must be followed by a non-`[0-9.]` boundary (or end of input).
fn match_dotted_run(bytes: &[u8], start: usize, segments: usize) -> Option<(String, usize)> {
    let mut i = start;
    for seg in 0..segments {
        // One or more digits.
        let seg_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == seg_start {
            return None; // empty group
        }
        if seg + 1 < segments {
            // Require a single separating dot before the next group.
            if i >= bytes.len() || bytes[i] != b'.' {
                return None;
            }
            i += 1;
        }
    }
    // Trailing boundary: not another digit or dot (which would make this a prefix
    // of a longer/different version run).
    if i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
        return None;
    }
    Some((String::from_utf8_lossy(&bytes[start..i]).into_owned(), i))
}

/// Advance past a contiguous run of digits and dots starting at `start`.
fn skip_number_run(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
        i += 1;
    }
    i.max(start + 1)
}

impl JsonRpcClient {
    /// Send a fire-and-forget notification (no `id`, no correlated response).
    pub async fn notify(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<(), TransportError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(TransportError::Closed(
                "app-server transport already closed".to_string(),
            ));
        }
        let line = encode_notification(method, &params);
        let mut writer = self.writer.lock().await;
        writer
            .write_all(line.as_bytes())
            .await
            .map_err(|err| TransportError::Io(err.to_string()))?;
        writer
            .flush()
            .await
            .map_err(|err| TransportError::Io(err.to_string()))
    }

    /// Send a RESPONSE to a server→client request (E21 #4870), correlated by the
    /// originating `id`. Reuses [`notify`](Self::notify)'s exact writer path
    /// (closed-check → lock writer → write_all → flush): a terminal write with
    /// NO pending-map insert and NO await-for-reply. A closed transport yields
    /// [`TransportError::Closed`]; the approval layer treats an unwritable
    /// response as fail-closed (the server simply never sees the decision and the
    /// turn proceeds without the gated action — safe).
    pub async fn respond(
        &self,
        id: u64,
        outcome: Result<serde_json::Value, RpcError>,
    ) -> Result<(), TransportError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(TransportError::Closed(
                "app-server transport already closed".to_string(),
            ));
        }
        let line = encode_response(id, outcome);
        let mut writer = self.writer.lock().await;
        writer
            .write_all(line.as_bytes())
            .await
            .map_err(|err| TransportError::Io(err.to_string()))?;
        writer
            .flush()
            .await
            .map_err(|err| TransportError::Io(err.to_string()))
    }
}

/// Run the app-server `initialize` handshake: send `initialize` with our
/// `clientInfo`, await the response, then send the required `initialized`
/// notification. Returns the parsed [`ServerInfo`].
pub async fn initialize(
    client: &JsonRpcClient,
    info: &ClientInfo,
) -> Result<ServerInfo, TransportError> {
    let result = client
        .request(
            "initialize",
            serde_json::json!({
                "clientInfo": {
                    "name": info.name,
                    "title": info.title,
                    "version": info.version,
                }
            }),
        )
        .await?;
    client.notify("initialized", serde_json::json!({})).await?;
    Ok(ServerInfo {
        user_agent: result
            .get("userAgent")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        raw: result,
    })
}

// ── AppServerProcess: spawn + wire + handshake + reap-on-drop (3b) ────────────

/// Owns a spawned `codex app-server` process, its JSON-RPC transport, and the
/// `initialize` handshake result. Dropping it reaps the whole process group
/// (#24347 on Unix). Windows currently falls back to direct-child kill-on-drop;
/// a full Job Object tree reap path is tracked in #4865.
pub struct AppServerProcess {
    child: tokio::process::Child,
    client: JsonRpcClient,
    server_info: ServerInfo,
    last_activity: StdMutex<tokio::time::Instant>,
}

impl AppServerProcess {
    /// Spawn `command` as `codex app-server`, wire its stdio to a
    /// [`JsonRpcClient`], and run the `initialize` handshake. Returns the
    /// process handle and the inbound notification stream.
    pub async fn connect(
        mut command: tokio::process::Command,
        info: &ClientInfo,
    ) -> Result<
        (
            Self,
            mpsc::UnboundedReceiver<Notification>,
            mpsc::UnboundedReceiver<InboundRequest>,
        ),
        TransportError,
    > {
        use std::process::Stdio;
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = {
            #[cfg(unix)]
            {
                spawn_in_process_group(command)
                    .map_err(|err| TransportError::Io(err.to_string()))?
            }
            #[cfg(not(unix))]
            {
                command.kill_on_drop(true);
                command
                    .spawn()
                    .map_err(|err| TransportError::Io(err.to_string()))?
            }
        };
        let stdout = child.stdout.take().ok_or_else(|| {
            TransportError::Io("app-server child stdout not captured".to_string())
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| TransportError::Io("app-server child stdin not captured".to_string()))?;

        let (client, notifications, inbound_requests) = JsonRpcClient::new(stdout, stdin);
        let server_info = initialize(&client, info).await?;
        Ok((
            Self {
                child,
                client,
                server_info,
                last_activity: StdMutex::new(tokio::time::Instant::now()),
            },
            notifications,
            inbound_requests,
        ))
    }

    /// The parsed `initialize` response.
    pub fn server_info(&self) -> &ServerInfo {
        &self.server_info
    }

    /// Time since the last request was issued. The scheduler uses this to
    /// drop (→ group-reap) idle app-server processes — the idle-timeout
    /// graceful-shutdown policy for #4865.
    pub fn idle_for(&self) -> Duration {
        lock_or_recover(&self.last_activity, "codex_app_server.last_activity").elapsed()
    }

    /// The OS pid of the app-server process (group leader), if still running.
    pub fn child_id(&self) -> Option<u32> {
        self.child.id()
    }

    /// Send a request to the app-server (resets the idle timer).
    pub async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, TransportError> {
        *lock_or_recover(&self.last_activity, "codex_app_server.last_activity") =
            tokio::time::Instant::now();
        self.client.request(method, params).await
    }

    /// Respond to a server→client request (E21 #4870), correlated by `id`
    /// (resets the idle timer). Delegates to [`JsonRpcClient::respond`].
    pub async fn respond(
        &self,
        id: u64,
        outcome: Result<serde_json::Value, RpcError>,
    ) -> Result<(), TransportError> {
        *lock_or_recover(&self.last_activity, "codex_app_server.last_activity") =
            tokio::time::Instant::now();
        self.client.respond(id, outcome).await
    }
}

impl Drop for AppServerProcess {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            // Reap the entire process group so the app-server's child tree
            // (e.g. an MCP server) is collected, not orphaned (#24347).
            reap_process_group(&self.child);
        }
        #[cfg(not(unix))]
        {
            let _ = self.child.start_kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        encode_request, encode_response, parse_incoming, parse_user_agent_version, IncomingMessage,
        RpcError,
    };
    use crate::mutex_ext::lock_or_recover;
    use std::sync::Mutex as StdMutex;

    #[test]
    fn parse_user_agent_version_extracts_three_part_token() {
        // The expected real-codex shape: a slash-delimited 3-part version inside
        // a longer userAgent string with trailing parenthetical metadata.
        assert_eq!(
            parse_user_agent_version("codex-app-server/1.2.3 (rust; macos)"),
            Some("1.2.3".to_string())
        );
    }

    #[test]
    fn lock_or_recover_returns_guard_after_poison() {
        let mutex = StdMutex::new(41_u32);
        let _ = std::panic::catch_unwind(|| {
            let mut guard = mutex.lock().expect("initial lock");
            *guard = 42;
            panic!("poison test mutex");
        });

        let mut guard = lock_or_recover(&mutex, "codex-test-mutex");
        *guard += 1;

        assert_eq!(*guard, 43);
    }

    #[test]
    fn parse_user_agent_version_accepts_two_part_fallback() {
        // CRITICAL anti-theater guard: the in-repo fake harness emits the 2-part
        // "codex-app-server/1.0". If the extractor only matched 3-part tokens it
        // would return None here and every inform-only version check would
        // silently no-op forever. The 2-part fallback MUST fire.
        assert_eq!(
            parse_user_agent_version("codex-app-server/1.0"),
            Some("1.0".to_string())
        );
    }

    #[test]
    fn parse_user_agent_version_prefers_three_part_over_two_part() {
        // When a full 3-part token exists, it wins over any 2-part prefix.
        assert_eq!(
            parse_user_agent_version("codex/10.20.30 build 4.5"),
            Some("10.20.30".to_string())
        );
    }

    #[test]
    fn parse_user_agent_version_returns_none_without_numeric_token() {
        assert_eq!(parse_user_agent_version("codex-app-server"), None);
        assert_eq!(parse_user_agent_version(""), None);
    }

    #[test]
    fn parse_user_agent_version_does_not_clip_longer_runs() {
        // A 4-segment run is not a valid 3-part version; the extractor must not
        // emit a clipped "1.2.3" out of "1.2.3.4" (boundary check). It also is
        // not a valid 2-part token, so the result is None.
        assert_eq!(parse_user_agent_version("v1.2.3.4"), None);
    }

    #[test]
    fn encode_request_omits_jsonrpc_header_and_is_jsonl() {
        let line = encode_request(7, "thread/start", &serde_json::json!({"cwd": "/tmp"}));
        assert!(line.ends_with('\n'), "framing must be newline-delimited");
        let value: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(value["id"], 7);
        assert_eq!(value["method"], "thread/start");
        assert_eq!(value["params"]["cwd"], "/tmp");
        assert!(
            value.get("jsonrpc").is_none(),
            "app-server omits the jsonrpc envelope field on the wire"
        );
    }

    #[test]
    fn parse_incoming_classifies_successful_response() {
        let msg = parse_incoming(r#"{"id":7,"result":{"threadId":"t_1"}}"#).unwrap();
        match msg {
            IncomingMessage::Response { id, outcome } => {
                assert_eq!(id, 7);
                assert_eq!(outcome.unwrap()["threadId"], "t_1");
            }
            other => panic!("expected response, got {other:?}"),
        }
    }

    #[test]
    fn parse_incoming_classifies_error_response() {
        let msg =
            parse_incoming(r#"{"id":8,"error":{"code":-32601,"message":"method not found"}}"#)
                .unwrap();
        match msg {
            IncomingMessage::Response {
                id,
                outcome: Err(err),
            } => {
                assert_eq!(id, 8);
                assert_eq!(
                    err,
                    RpcError {
                        code: -32601,
                        message: "method not found".to_string()
                    }
                );
            }
            other => panic!("expected error response, got {other:?}"),
        }
    }

    #[test]
    fn parse_incoming_classifies_notification_without_id() {
        let msg = parse_incoming(r#"{"method":"item/agentMessage/delta","params":{"text":"hi"}}"#)
            .unwrap();
        match msg {
            IncomingMessage::Notification { method, params } => {
                assert_eq!(method, "item/agentMessage/delta");
                assert_eq!(params["text"], "hi");
            }
            other => panic!("expected notification, got {other:?}"),
        }
    }

    #[test]
    fn parse_incoming_rejects_malformed_line() {
        let err = parse_incoming("not json").unwrap_err();
        assert!(
            err.contains("expected") || err.contains("invalid") || err.contains("JSON"),
            "malformed JSON line must produce a parse error message, got: {err:?}"
        );
    }

    // ── #4870: server→client REQUEST classification (KEYSTONE) ──

    #[test]
    fn parse_incoming_classifies_server_request_with_id_and_method() {
        // An inbound line with BOTH an id AND a method (no result/error) is a
        // server→client REQUEST (requestApproval), NOT a Response. Before #4870
        // this was misclassified as Response{Ok(null)} and dropped.
        let msg = parse_incoming(
            r#"{"id":9,"method":"item/commandExecution/requestApproval","params":{"command":"ls"}}"#,
        )
        .unwrap();
        match msg {
            IncomingMessage::Request { id, method, params } => {
                assert_eq!(id, 9);
                assert_eq!(method, "item/commandExecution/requestApproval");
                assert_eq!(params["command"], "ls");
            }
            other => panic!("expected Request, got {other:?}"),
        }
    }

    #[test]
    fn parse_incoming_response_regression_id_plus_result_no_method() {
        // REGRESSION LOCK: a line with id + result and NO method must still be a
        // Response, not a Request. (If the discriminator were broken to require
        // only `id`, this would flip and break correlation.)
        let msg = parse_incoming(r#"{"id":7,"result":{"threadId":"t_1"}}"#).unwrap();
        assert!(
            matches!(msg, IncomingMessage::Response { id: 7, .. }),
            "id+result (no method) must classify as Response, got {msg:?}"
        );
    }

    #[test]
    fn parse_incoming_id_plus_error_is_response_even_though_no_method() {
        // An error reply (id + error, no method) is a Response, not a Request:
        // the Request branch requires error.is_none().
        let msg = parse_incoming(r#"{"id":8,"error":{"code":-32601,"message":"x"}}"#).unwrap();
        assert!(
            matches!(
                msg,
                IncomingMessage::Response {
                    id: 8,
                    outcome: Err(_)
                }
            ),
            "id+error must classify as Response(Err), got {msg:?}"
        );
    }

    #[test]
    fn encode_response_success_shape() {
        let line = encode_response(11, Ok(serde_json::json!({"decision": "approved"})));
        assert!(line.ends_with('\n'), "framing must be newline-delimited");
        let value: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(value["id"], 11);
        assert_eq!(value["result"]["decision"], "approved");
        assert!(value.get("error").is_none());
        assert!(
            value.get("jsonrpc").is_none(),
            "app-server omits the jsonrpc envelope field on the wire"
        );
    }

    #[test]
    fn encode_response_error_shape() {
        let line = encode_response(
            12,
            Err(RpcError {
                code: -32000,
                message: "boom".to_string(),
            }),
        );
        let value: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(value["id"], 12);
        assert_eq!(value["error"]["code"], -32000);
        assert_eq!(value["error"]["message"], "boom");
        assert!(value.get("result").is_none());
    }

    // ── Increment 2: request/response correlation + notification stream ──

    use super::{JsonRpcClient, TransportError};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn request_correlates_response_and_delivers_notification_concurrently() {
        let (client_io, server_io) = tokio::io::duplex(8192);
        let (cr, cw) = tokio::io::split(client_io);
        let (client, mut notifications, _inbound) = JsonRpcClient::new(cr, cw);

        // Mock app-server: read the request, emit a notification BEFORE the
        // correlated response to prove the two streams are demultiplexed.
        tokio::spawn(async move {
            let (sr, mut sw) = tokio::io::split(server_io);
            let mut lines = tokio::io::BufReader::new(sr).lines();
            let line = lines.next_line().await.unwrap().unwrap();
            let req: serde_json::Value = serde_json::from_str(&line).unwrap();
            let id = req["id"].as_u64().unwrap();
            assert_eq!(req["method"], "thread/start");
            sw.write_all(b"{\"method\":\"turn/started\",\"params\":{\"n\":1}}\n")
                .await
                .unwrap();
            sw.write_all(
                format!("{{\"id\":{id},\"result\":{{\"threadId\":\"t_1\"}}}}\n").as_bytes(),
            )
            .await
            .unwrap();
        });

        let result = client
            .request("thread/start", serde_json::json!({"cwd": "/tmp"}))
            .await
            .expect("request should correlate to its response");
        assert_eq!(result["threadId"], "t_1");

        let note = notifications.recv().await.expect("notification delivered");
        assert_eq!(note.method, "turn/started");
        assert_eq!(note.params["n"], 1);
    }

    #[tokio::test]
    async fn pending_request_fails_closed_when_transport_drops() {
        let (client_io, server_io) = tokio::io::duplex(8192);
        let (cr, cw) = tokio::io::split(client_io);
        let (client, _notifications, _inbound) = JsonRpcClient::new(cr, cw);

        // Server reads the request then drops without responding (process death).
        tokio::spawn(async move {
            let (sr, _sw) = tokio::io::split(server_io);
            let mut lines = tokio::io::BufReader::new(sr).lines();
            let _ = lines.next_line().await;
            // drop server_io halves → client read loop sees EOF
        });

        let err = client
            .request("turn/start", serde_json::json!({}))
            .await
            .expect_err("dropped transport must fail the in-flight request");
        assert!(
            matches!(err, TransportError::Closed(_)),
            "expected Closed, got {err:?}"
        );
    }

    // ── #4870: respond() writes a correlated response line ──

    #[tokio::test]
    async fn respond_writes_correlated_response_line() {
        // respond(id, Ok(..)) must emit a single JSONL line carrying that id and
        // the result payload, through the same writer path as request()/notify().
        let (client_io, server_io) = tokio::io::duplex(8192);
        let (cr, cw) = tokio::io::split(client_io);
        let (client, _notifications, _inbound) = JsonRpcClient::new(cr, cw);

        let server = tokio::spawn(async move {
            let (sr, _sw) = tokio::io::split(server_io);
            let mut lines = tokio::io::BufReader::new(sr).lines();
            lines.next_line().await.unwrap().unwrap()
        });

        client
            .respond(42, Ok(serde_json::json!({"decision": "approved"})))
            .await
            .expect("respond writes the correlated line");

        let line = server.await.unwrap();
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["id"], 42);
        assert_eq!(value["result"]["decision"], "approved");
    }

    #[tokio::test]
    async fn respond_on_closed_transport_errors() {
        // After the read loop closes (server stdout EOF), respond() must fail
        // Closed rather than silently succeeding — the approval layer treats this
        // as fail-closed (the decision never reaches the server).
        let (client_io, server_io) = tokio::io::duplex(8192);
        let (cr, cw) = tokio::io::split(client_io);
        let (client, _notifications, _inbound) = JsonRpcClient::new(cr, cw);

        // Drop the server side so the client read loop sees EOF and marks closed.
        drop(server_io);
        // Give the spawned read loop a moment to observe EOF + set closed.
        for _ in 0..50 {
            if client.closed.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let err = client
            .respond(1, Ok(serde_json::json!({"decision": "denied"})))
            .await
            .expect_err("respond on a closed transport must error");
        assert!(matches!(err, TransportError::Closed(_)), "got {err:?}");
    }

    // ── Increment 3a: process-group reap (R2 / openai/codex#24347) ──

    #[cfg(unix)]
    #[tokio::test]
    async fn reap_process_group_kills_the_whole_child_tree() {
        use super::{reap_process_group, spawn_in_process_group};
        use std::process::Stdio;
        use tokio::io::AsyncReadExt;

        fn alive(pid: i32) -> bool {
            // signal 0 = existence check; an unreaped zombie also returns 0,
            // so callers poll for the process to fully disappear.
            unsafe { libc::kill(pid, 0) == 0 }
        }

        // Parent `sh` spawns a long-lived grandchild and prints its PID, then
        // blocks. `kill_on_drop`/direct-child kill would orphan the grandchild
        // (the #24347 leak); a process-GROUP kill must reap it.
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg("sleep 120 & echo $! ; wait")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = spawn_in_process_group(cmd).expect("spawn fake app-server tree");

        // Read the grandchild PID line from stdout.
        let mut stdout = child.stdout.take().unwrap();
        let mut buf = [0u8; 32];
        let n = stdout.read(&mut buf).await.unwrap();
        let grandchild_pid: i32 = String::from_utf8_lossy(&buf[..n])
            .trim()
            .parse()
            .expect("grandchild pid");
        assert!(alive(grandchild_pid), "grandchild should be alive pre-reap");

        reap_process_group(&child);

        // Poll up to ~2s for the grandchild to be fully gone (SIGKILL + reparent).
        let mut gone = false;
        for _ in 0..40 {
            if !alive(grandchild_pid) {
                gone = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        // Best-effort cleanup of the direct child regardless of assertion.
        let _ = child.start_kill();
        assert!(
            gone,
            "process-group reap must kill the grandchild tree (#24347)"
        );
    }

    // ── Increment 3b: initialize handshake + AppServerProcess integration ──

    #[tokio::test]
    async fn initialize_handshake_sends_clientinfo_then_initialized_and_parses_server_info() {
        use super::{initialize, ClientInfo, JsonRpcClient};
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

        let (client_io, server_io) = tokio::io::duplex(8192);
        let (cr, cw) = tokio::io::split(client_io);
        let (client, _notifications, _inbound) = JsonRpcClient::new(cr, cw);

        let server = tokio::spawn(async move {
            let (sr, mut sw) = tokio::io::split(server_io);
            let mut lines = tokio::io::BufReader::new(sr).lines();
            // 1. initialize request
            let req: serde_json::Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(req["method"], "initialize");
            assert_eq!(req["params"]["clientInfo"]["name"], "maekon");
            let id = req["id"].as_u64().unwrap();
            sw.write_all(
                format!("{{\"id\":{id},\"result\":{{\"userAgent\":\"codex-app-server/1.2\"}}}}\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
            // 2. the `initialized` notification (no id) must follow
            let note: serde_json::Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(note["method"], "initialized");
            assert!(
                note.get("id").is_none(),
                "notification must not carry an id"
            );
        });

        let info = ClientInfo {
            name: "maekon".to_string(),
            title: "Maekon".to_string(),
            version: "0.0.1".to_string(),
        };
        let server_info = initialize(&client, &info).await.expect("handshake");
        assert_eq!(
            server_info.user_agent.as_deref(),
            Some("codex-app-server/1.2")
        );
        server.await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn app_server_process_connects_handshakes_and_reaps_on_drop() {
        use super::{AppServerProcess, ClientInfo};

        fn alive(pid: i32) -> bool {
            unsafe { libc::kill(pid, 0) == 0 }
        }

        // Fake app-server: reply to initialize with a matching id, consume the
        // `initialized` notification, then stay alive (so drop-reap is exercised).
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(
            r#"IFS= read -r line; id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p'); printf '{"id":%s,"result":{"userAgent":"fake-app-server/9.9"}}\n' "$id"; IFS= read -r _ignored; sleep 120 & wait"#,
        );

        let info = ClientInfo {
            name: "maekon".to_string(),
            title: "Maekon".to_string(),
            version: "0.0.1".to_string(),
        };
        let (process, _notifications, _inbound) = AppServerProcess::connect(cmd, &info)
            .await
            .expect("connect + handshake against fake app-server");
        assert_eq!(
            process.server_info().user_agent.as_deref(),
            Some("fake-app-server/9.9")
        );

        let pid = process.child_id().expect("running child pid") as i32;
        assert!(alive(pid), "app-server should be alive after connect");

        drop(process);

        let mut gone = false;
        for _ in 0..40 {
            if !alive(pid) {
                gone = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            gone,
            "dropping AppServerProcess must reap the app-server group"
        );
    }

    // ── Increment 3c: request timeout + idle tracking (policies) ──

    #[tokio::test(start_paused = true)]
    async fn request_times_out_when_server_never_responds() {
        use super::{JsonRpcClient, TransportError};

        let (client_io, server_io) = tokio::io::duplex(8192);
        let (cr, cw) = tokio::io::split(client_io);
        let (client, _notifications, _inbound) = JsonRpcClient::new(cr, cw);
        let client = client.with_request_timeout(std::time::Duration::from_secs(5));

        // Server reads the request but never replies; it holds the io open (a
        // long sleep) so the client does NOT see EOF — only the timeout fires.
        tokio::spawn(async move {
            let (sr, _sw) = tokio::io::split(server_io);
            let mut lines = tokio::io::BufReader::new(sr).lines();
            let _ = lines.next_line().await;
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        });

        let err = client
            .request("hang", serde_json::json!({}))
            .await
            .expect_err("a request with no response must time out");
        assert!(
            matches!(err, TransportError::Timeout { .. }),
            "expected Timeout, got {err:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn app_server_process_tracks_idle_duration() {
        use super::{AppServerProcess, ClientInfo};

        // NOTE: real (not start_paused) time — connect performs a real subprocess
        // handshake, and start_paused would auto-advance past the request timeout
        // before the fake `sh` could respond.
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(
            r#"IFS= read -r line; id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p'); printf '{"id":%s,"result":{"userAgent":"fake/1"}}\n' "$id"; IFS= read -r _ignored; sleep 3600 & wait"#,
        );
        let info = ClientInfo {
            name: "maekon".to_string(),
            title: "Maekon".to_string(),
            version: "0.0.1".to_string(),
        };
        let (process, _notifications, _inbound) =
            AppServerProcess::connect(cmd, &info).await.unwrap();

        // A fresh process is near-zero idle; idle grows after a quiet period.
        assert!(process.idle_for() < std::time::Duration::from_secs(1));
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert!(
            process.idle_for() >= std::time::Duration::from_millis(150),
            "idle_for must reflect time since last activity (idle-shutdown policy input)"
        );
    }
}
