# Internal `/api` per-session local-auth gate (E20-41 / #4833)

> Transport-level security note **beside** the frozen contract
> (`maekon-web.v1.openapi.yaml`, `http-interface-manifest.v1.json`). This gate is
> **additive** — it changes no response schema and does not touch the JWT/token
> subsystem, so the frozen contract is untouched.

## What changed

Every request to the internal `/api` surface now requires a **per-session
local-auth token** in addition to the existing loopback-only restriction. Without
a valid token the server responds **`401 Unauthorized`**.

This closes a local privilege-escalation gap: on a multi-user host (RDP / Citrix)
any local user could previously reach `127.0.0.1:<port>/api/settings` or
`/api/audit/export` — the loopback check alone does not distinguish users. The
naive `SO_PEERCRED` approach is **not** usable here: the dashboard is served over
TCP loopback (AF_INET), where OS peer-cred always fails (it is AF_UNIX-only) and
would fail **open**.

## Token channels (accepted by the gate)

| Channel | Header / field | Used by |
|---------|----------------|---------|
| HTTP header (primary) | `X-Local-Auth: <token>` | `fetch` (the dashboard SPA, both browser + Tauri — works cross-origin via CORS) |
| HTTP header (alias) | `Authorization: Bearer <token>` | programmatic clients |
| Query param | `?local_auth=<token>` | EventSource / SSE (`/api/stream`, `/api/update/stream`) — cross-origin Tauri (`tauri://localhost` → `127.0.0.1`) where EventSource cannot set a header and a Tauri-document cookie is never sent to the loopback origin |
| Cookie | `maekon_local_auth=<token>` (`Path=/api; SameSite=Strict`) | same-origin (plain-browser) EventSource / SSE |

The query-param channel is **safe** because the access-log span
(`TraceLayer::make_span_with`) records the request **path only, never the query
string** — the token never reaches Loki/Grafana/OTel logs. `Referer` does not carry
it either (the token is in the request URI, not the page URL).

`OPTIONS` (CORS preflight) is exempt — it carries no auth header and is answered
by the CORS layer.

## Token lifecycle

- **Ephemeral**, minted from the OS CSPRNG (256-bit) once per app launch. **Never**
  persisted to `config.json`, **never** logged, **never** passed via env/argv.
- Delivered to the **legit** client only: injected into the Tauri WebView as
  `window.__MAEKON_LOCAL_AUTH__` (with a `get_local_auth_token` IPC fallback). A
  different local user's browser never transits Tauri, so it never receives the
  token and is rejected.
- **Never exposed over HTTP** — there is no endpoint that returns the token.
- Plain-browser access (`http://localhost:10090` in a real browser, not the
  WebView) uses a one-time **`#local_auth=<token>` URL-fragment** handshake. A
  fragment is never sent to the server, so it cannot leak into access logs or the
  `Referer`; the page reads it, sets the same-origin cookie, and scrubs the
  fragment from history.

## Compatibility

- `server`-feature builds keep `web.integration_auth_token` (config-persisted) for
  the **external** `/integration/v1` surface — that is unchanged and orthogonal.
- The token comparison is constant-time (`subtle::ConstantTimeEq`) to avoid a
  timing oracle from a co-resident attacker.
