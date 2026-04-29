# SSE Reconnection Strategy

How the web dashboard maintains a real-time Server-Sent Events connection
to the local Axum server, including reconnection behavior and debugging tips.

## Architecture Overview

```
┌────────────────────┐       GET /api/stream        ┌──────────────────────┐
│  React frontend    │  ─── EventSource ──────────►  │  Axum SSE handler    │
│  useSSE() hook     │  ◄── metrics / frame / idle   │  stream.rs           │
└────────────────────┘       + keep-alive ping        └──────────────────────┘
```

Two layers are involved:

1. **Server (Rust)** -- `crates/oneshim-web/src/handlers/stream.rs` +
   `crates/oneshim-web/src/services/stream_service.rs`
2. **Client (TypeScript)** -- `crates/oneshim-web/frontend/src/hooks/useSSE.ts`

## Server Side

The Axum handler at `GET /api/stream` creates an SSE response via
`Sse::new(stream).keep_alive(...)`.

Key properties:

| Setting | Value | Purpose |
|---------|-------|---------|
| Keep-alive interval | 15 seconds | Sends `:ping\n\n` comment to prevent proxies / browsers from closing idle connections |
| Keep-alive text | `"ping"` | Identifies the heartbeat in network logs |
| Broadcast channel | 16-slot buffer | `tokio::sync::broadcast::channel(16)` — backpressure drops oldest events when a slow consumer lags behind |

The stream is backed by a `BroadcastStream` that subscribes to the
application-wide `event_tx` broadcast channel. Each SSE client gets its own
subscription, so multiple browser tabs work independently.

### Event Types

| SSE event name | Payload | Source |
|----------------|---------|--------|
| `metrics` | CPU, memory, disk, network | Monitor loop (5s interval) |
| `frame` | Frame metadata (id, app, title, importance) | Capture loop |
| `idle` | `is_idle`, `idle_secs` | Activity tracker |
| `ai_runtime_status` | AI model status | Sent once on connection (initial event) |

## Client Side (`useSSE` Hook)

### Connection Establishment

1. On mount, the hook checks `isStandaloneModeEnabled()`. In standalone
   (demo) mode the connection is skipped entirely.
2. Otherwise, `connectInternal()` resolves the API base URL via
   `resolveApiUrl('/api/stream')` and creates a browser `EventSource`.
3. Named event listeners are attached: `metrics`, `frame`, `idle`, `ping`.
4. A **connect token** (monotonically increasing integer) guards against
   stale callbacks -- if the token has changed by the time `onopen` or
   `onerror` fires, the orphaned `EventSource` is closed immediately.

### Connection States

```
disconnected ──► connecting ──► connected
                     │                │
                     ▼                ▼
                   error ◄────────────┘
                     │
              (retry loop)
                     │
                     ▼
              disconnected (maxRetries exhausted)
```

The `ConnectionStatus` type is one of: `connecting`, `connected`,
`disconnected`, `error`.

### Reconnection Behavior

| Parameter | Default | Description |
|-----------|---------|-------------|
| `autoReconnect` | `true` | Enables automatic reconnection on error |
| `reconnectDelay` | `3000` ms | Fixed delay between retry attempts |
| `maxRetries` | `10` | Maximum consecutive retry attempts before giving up |

The reconnection strategy is **fixed-interval** (not exponential backoff).
On each `onerror`:

1. Status is set to `error`.
2. The current `EventSource` is closed.
3. If `retryCount < maxRetries`, a `setTimeout(reconnectDelay)` schedules
   another `connectInternal()` call.
4. If retries are exhausted, status moves to `disconnected` permanently
   until the user triggers `connect()` manually.
5. On successful reconnection (`onopen`), `retryCount` resets to 0.

### Bounded Data

- `metricsHistory` is capped at `MAX_HISTORY_SIZE = 60` entries (roughly
  5 minutes at one update per 5 seconds). Oldest entries are shifted out.

### Cleanup

The `useEffect` cleanup calls `disconnect()`, which:
- Increments the connect token (invalidating any in-flight async work).
- Clears any pending reconnect timeout.
- Closes the `EventSource`.
- Resets retry count to 0.

## Debugging a "Frozen Dashboard"

When the dashboard stops updating in real time:

### Step 1: Check Connection Status

The `StatusBar` component displays the SSE connection status. Look for
`disconnected` or `error`.

### Step 2: Verify the Server Is Running

```bash
curl -N http://127.0.0.1:10090/api/stream
```

You should see a stream of `event: metrics` lines. If the connection is
refused, the Axum web server has not started (check that `oneshim-app` is
running and port 10090 is not occupied).

### Step 3: Check Browser DevTools

Open the Network tab, filter by `EventSource` or `stream`. Common issues:

| Symptom | Cause | Fix |
|---------|-------|-----|
| Connection refused | Server not running or wrong port | Start the app or check port config |
| Repeated 5xx errors | Panic in stream handler | Check Rust logs (`RUST_LOG=debug`) |
| Stream opens but no events | Broadcast channel has no senders | Ensure the monitor loop is active |
| Max retries exhausted | Server was down > 30 seconds | Click the reconnect button or reload the page |
| Events arrive but UI does not update | React state issue | Check browser console for parse errors |

### Step 4: Check CSP

The `connect-src` directive in `tauri.conf.json` must include the server
origin. The default configuration allows `http://127.0.0.1:10090` through
`http://127.0.0.1:10099`.

### Step 5: Force Reconnect

Call `connect()` from the hook (exposed in the return value) or simply
reload the page. The hook auto-connects on mount.

## Related Files

- `crates/oneshim-web/src/handlers/stream.rs` -- Axum SSE handler
- `crates/oneshim-web/src/services/stream_service.rs` -- Stream query service
- `crates/oneshim-web/src/services/stream_assembler.rs` -- Event serialization
- `crates/oneshim-web/frontend/src/hooks/useSSE.ts` -- React SSE hook
- `crates/oneshim-web/frontend/src/hooks/__tests__/useSSE.test.ts` -- Hook tests
- `crates/oneshim-web/frontend/src/components/shell/StatusBar.tsx` -- Connection indicator
