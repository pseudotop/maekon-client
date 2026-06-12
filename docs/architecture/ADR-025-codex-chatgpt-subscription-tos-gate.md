[English](./ADR-025-codex-chatgpt-subscription-tos-gate.md) | [한국어](./ADR-025-codex-chatgpt-subscription-tos-gate.ko.md)

# ADR-025: OpenAI Codex / ChatGPT Subscription ToS Gate (App-Server Path)

**Status**: Accepted
**Date**: 2026-06-04
**Scope**: `specs/providers/provider-surface-catalog.json`, `src-tauri/src/session_manager/factory.rs`, `src-tauri/src/provider_adapters/llm_resolver.rs`
**Related**: ADR-024 (conversation content guard port), ADR-021 (config/consent core placement), ADR-019 (error codes)
**Related issues**: #4861 (Epic E21), #4863 (PoC), #4866 (session resume), #4868 (`chatgptAuthTokens` injection), #4871 (feature-flag rollout + degrade), #4872 (mock harness + R4 attribution)

> **This is an engineering Terms-of-Service *risk* analysis, not legal advice or a legal sign-off.** It documents what the publicly available OpenAI/Anthropic policies and developer docs say, maps them onto maekon's architecture, and decides which E21 work may proceed and under what conditions. Items marked **NEEDS_LEGAL_SIGNOFF** require a human/legal decision before they ship. See §5.

---

## Context

E21 migrates the Codex client integration from per-turn `codex exec` to a persistent `codex app-server` JSON-RPC session (#4861). Issue #4884 (review findings B4/R5) raised a governance gate that *blocks* PoC start (#4863): maekon's `provider-surface-catalog.json` declares the Codex surfaces with `credential_kind: cli_subscription` — maekon borrows the **user's installed Codex CLI ChatGPT subscription** rather than an API key. Combined with the planned `chatgptAuthTokens` host-token injection (#4868), this lets a **non-interactive autonomous agent actively consume a human's ChatGPT subscription quota**. Before #4884, OpenAI ToS / account-attribution / quota / liability boundaries were documented in **0** of the 11 E21 issues and **0** catalog fields.

maekon is materially different from an interactive coding assistant in three ways that sharpen the gate:

1. **Autonomous, non-interactive turns** — maekon can drive turns without a human in the loop, which is exactly the "automation that looks like programmatic extraction" grey zone.
2. **Screen-capture / surveillance product** — turns may carry captured screen content that can implicate *third parties'* private/sensitive data.
3. **Distributed, potentially paid product** — not a single developer's personal CLI use on their own machine.

This ADR resolves the gate with a per-sub-feature decision matrix.

### Evidence confidence tiers

The gate **release** rests only on HIGH-confidence evidence; MEDIUM-confidence evidence is used only to *restrict* (the conservative-safe direction).

- **HIGH (directly fetched / read)**: `developers.openai.com/codex/auth` and `/app-server`; the in-repo catalog and `factory.rs` facts (re-verified at worktree HEAD `533b9c27d` by three independent review agents + the author).
- **MEDIUM (WebSearch-synthesized; `openai.com/policies/*` and `help.openai.com` returned HTTP 403 to automated fetch, `web.archive.org` tool-blocked)**: exact wording of `row-terms-of-use`, the account-sharing policy, and the consumer-vs-API data-training defaults. Exact wording MUST be re-confirmed from an authenticated/unblocked context before any production sign-off (§5, residual item 1).

## Decision

### Decision matrix

| # | Subject | Verdict | Basis |
|---|---------|---------|-------|
| 1 | **#4863 app-server PoC** via ChatGPT-managed OAuth (CLI owns auth) | **CONDITIONAL — cleared for human-attended turns only** | HIGH |
| 2 | **app-server vs already-shipped `exec`** transport (transport axis only) | **ALLOWED** (same ToS footing) | HIGH |
| 3 | **Non-interactive / autonomous quota consumption** on a consumer subscription | **CONDITIONAL — API-key required for unattended turns** | HIGH+MEDIUM |
| 4 | **#4868 `chatgptAuthTokens` host-token injection** (OpenAI "External tokens", experimental) | **BLOCKED as product default — NEEDS_LEGAL_SIGNOFF** | HIGH+precedent |
| 5 | **Commercial / paid distribution** of the wrapper | **NEEDS_LEGAL_SIGNOFF** | open question |
| 6 | **Data-governance**: captured screen content via consumer subscription | **CONDITIONAL — API-key default for captured content** | HIGH+MEDIUM |

### 1. #4863 app-server PoC — cleared, but only for human-attended turns

The app-server PoC and the #4871/#4872 flag-rollout + degrade + handshake work are **cleared to proceed as experimental, flag-gated work** because they add **no new ToS credential surface** beyond the already-shipped `exec` path:

- `provider_surface.openai.codex_app_server` uses the **identical** `credential_kind: cli_subscription` and `auth_probe_mode: codex_login_status_text` as the shipped `provider_surface.openai.subprocess_cli` (exec) surface. It drives the **user's own `codex login`** through the same official binary — OpenAI's "ChatGPT managed" mode, where *Codex* owns token persistence/refresh, not maekon.
- `chatgptAuthTokens` has **0 code references** (grep-confirmed) — host-token injection does not exist today.
- `clientInfo.name = "maekon"` attribution is wired (`factory.rs:316-320`) and contract-tested by a full outbound JSON-RPC wire snapshot (`crates/maekon-network/tests/codex_app_server_integration.rs::initialize_request_contract_snapshot`).

**Conditions (all required):**
1. The **CLI must own the OAuth flow end-to-end** — keep `auth_probe_mode: codex_login_status_text`; maekon MUST NOT read, persist, relay, or inject the underlying ChatGPT token (that is Decision 4).
2. **Per-human single-user binding only** — one human's own subscription; no credential sharing, no multi-user proxy, no multi-tenant relay.
3. Keep `stability: experimental` and `preferred_for_product_auth: false`.
4. Keep `clientInfo.name` attribution wired and contract-tested.
5. Backfill catalog `tos_notes` + `usage_attribution` (Decision 7) before promoting beyond PoC.
6. **The clearance is narrow.** It covers the *transport* and *human-attended* use only; it does **not** bless autonomous consumption (Decision 3) or captured-content routing (Decision 6).

**Rationale**: "already shipped" ≠ "ToS-adjudicated". The exec path shipped without an explicit ToS review, so app-server *inherits* — but does not resolve — the exec path's autonomous-consumption and data-governance exposure. The PoC clearance is justified by transport parity with shipped code plus OpenAI's own endorsement of app-server for "a deep integration inside your own product: authentication, conversation history, approvals, and streamed agent events" (`developers.openai.com/codex/app-server`, HIGH), not by a blanket blessing of how maekon consumes the subscription.

### 2. app-server vs exec transport — allowed, same footing

The transport mechanism (driving the official `codex` binary via `app-server` JSON-RPC vs `exec` one-shot invocation) is **not a ToS-relevant axis** — both consume the user's own subscription through the same binary's own login. Any ToS constraint that applies to one applies equally to the other and must be encoded **once** in shared `tos_notes`. The `exec` surface is already shipped (`preferred_for_product_auth: true`); adding app-server as an experimental, flag-gated, fallback-protected sibling adds no new credential surface.

### 3. Non-interactive / autonomous consumption — API-key required for unattended turns

This is the **load-bearing residual risk** and the literal subject of #4884 ("driving codex app-server to consume a human user's ChatGPT subscription **non-interactively**"). OpenAI's own docs repeatedly steer programmatic/automation use to **API keys**:

- *"We recommend API key authentication for programmatic Codex CLI workflows, such as CI/CD jobs."* (`developers.openai.com/codex/auth`, HIGH)
- *"Don't expose Codex execution in untrusted or public environments."* (same, HIGH)
- *"Access tokens are intended for trusted scripts, schedulers, and private CI runners."* (same, HIGH)

**Conditions:**
1. **Prefer/require API-key (Platform-billed) surfaces for any unattended or scheduled turn.** Reserve consumer-subscription auth for **human-attended / approval-gated** turns.
2. On a consumer subscription, autonomous turns must be **paced like interactive use** — no retry storms, no bursting, nothing that "looks like programmatic extraction or rate-limit circumvention" (consumer ToU "what you cannot do", MEDIUM).
3. Keep `preferred_for_product_auth: false` on `cli_subscription` surfaces.
4. `is_external() = true` privacy guard (ADR-024) + R6 read-only sandbox clamp remain **necessary but not sufficient** — they gate egress, not the consumption/attribution concern.

### 4. #4868 `chatgptAuthTokens` host-token injection — BLOCKED as product default

OpenAI's "External tokens" mode is *"experimental and intended for host apps that already own the user's ChatGPT auth"* (`developers.openai.com/codex/app-server`, HIGH). Host-supplied/persisted tokens driving a subscription is **structurally the same shape Anthropic explicitly banned in Jan 2026** — Anthropic enforced its Consumer ToS against tools (OpenClaw, OpenCode, Roo Code, Goose) that intercepted the OAuth flow, extracted the access token, and made calls *outside* the native client. The pattern Anthropic left *allowed* was invoking the official CLI binary as a subprocess (the Decision 1/2 pattern). A major lab demonstrably *can and did* prohibit exactly the host-token mode.

**Verdict: BLOCKED as a product default; NEEDS_LEGAL_SIGNOFF before any ship.** Do not ship on the optimistic "OpenAI offers it as experimental" reading. If ever pursued, it is permissible only if **all** hold: (a) the end user personally supplies their own token; (b) maekon never stores, relays, or shares it across users/sessions beyond the single owning session; (c) explicit written OpenAI confirmation is obtained. Treat as a **separate gate** from #4863/#4871/#4872 — it is the only genuinely new ToS surface.

### 5. Commercial / paid distribution — NEEDS_LEGAL_SIGNOFF

OpenAI explicitly did **not** answer the "build + sell a paid wrapper, users bring their own subscription" question in `openai/codex` Discussion #8338 (the OpenAI maintainer punted to legal counsel). The OpenClaw/Altman community endorsement covers a **free OSS** tool — a weak precedent for a **paid, distributed, autonomous surveillance** product. The Apache-2.0 license clears reuse of the *code*; it says nothing about *subscription consumption terms*.

**Defensible configuration only**: user brings their own subscription via the official `codex` binary as a subprocess, user owns `codex login` (`auth_probe_mode: codex_login_status_text`), no host-token injection, no multi-user proxy. Do **not** rely on the OpenClaw/Altman analogy or Discussion #8338 as a green light. Avoid any framing/architecture readable as "reselling or leasing access to an Account" or "powering a third-party service" on users' subscriptions (consumer ToU, MEDIUM).

### 6. Data-governance — API-key default for captured content

Distinctive to maekon as a screen-capture product: **consumer ChatGPT plans train on conversations by default (opt-out)**, whereas the API/business path is **no-train by default (opt-in)** (MEDIUM — `help.openai.com` data-usage articles + `openai.com/enterprise-privacy`, 403-blocked, re-confirm before sign-off). Routing captured screen content — which may contain third parties' private/sensitive data — through consumer-subscription auth means that content **may be used to train OpenAI models by default**.

**Conditions:**
1. For any flow carrying **captured screen content**, the **API-key (Platform, no-train-by-default) path is the data-governance-correct default**; consumer-subscription auth should not be the default for captured content.
2. If consumer subscription is used for captured content, surface an explicit runtime/`tos_notes` warning that content may be used for training by default and may implicate third-party data.
3. `is_external()` guard + R6 clamp gate egress, **not** training-data usage — they are necessary but not sufficient here.

### 7. Catalog changes (`provider-surface-catalog.json`)

Encode the above as machine-readable fields on **both** OpenAI `cli_subscription` surfaces (`provider_surface.openai.subprocess_cli` and `provider_surface.openai.codex_app_server`):

- `usage_attribution`: `{ mechanism: "client_info_name", value: "maekon", target: "OpenAI Compliance Logs Platform", evidence: "JSON-RPC initialize clientInfo.name" }`.
- `tos_notes`: array capturing — (a) `cli_subscription` drives the user's own `codex login` (ChatGPT-managed mode, host does not inject tokens); (b) subscription auth = interactive/trusted-private only per OpenAI docs, unattended/scheduled consumption is the disfavored grey zone → prefer API-key; (c) on 429, back off + surface to user, do not re-route to keep consuming (no rate-limit circumvention); (d) consumer plans train-by-default → prefer API-key for captured screen content; (e) `host_injects_token: false`.
- Add `host_injects_token: false` as a machine-checkable boolean, enforced by a **CI lint** (not merely declarative) asserting maekon never sets it `true` outside a future #4868-gated surface.
- Add `https://openai.com/policies/row-terms-of-use/` to the `references` of both surfaces alongside the existing `developers.openai.com/codex/auth/`.
- When #4868 lands, add a **distinct** surface (or `credential_kind` value) for host-injected tokens with its own `tos_notes` flagging highest-risk status + the Anthropic precedent, `preferred_for_product_auth: false`, `stability: experimental`.

### 8. 429 / rate-limit policy (#4871)

**Verified gap**: `factory.rs:288-296` is a blanket `Err(err) => … build_codex_exec_session(…, "app_server_failed")` catch-all; there is **no** 429/`RateLimit`/quota classification anywhere in `codex_app_server.rs` / `codex_app_server_session.rs` (grep-confirmed empty). Degrading a 429 to `codex exec` just re-hits the **same** `cli_subscription` quota wall, and re-routing around a rate limit to keep consuming is exactly what the consumer ToU "what you cannot do" forbids ("circumventing rate limits", MEDIUM).

**Policy**: On 429 / rate-limit / quota-exhausted from the app-server (or exec) path, **back off and surface the limit to the user** — do **not** silently retry and do **not** re-route to an alternate auth/transport to keep consuming. This requires classifying 429 distinctly from transport failures in the adapter so the `factory.rs` fallback does not swallow it into the generic degrade-to-exec path. Tracked as a follow-up (§Known Follow-ups 1).

## Consequences

### Positive

- The PoC (#4863) and the already-merged #4866/#4871/#4872 work are unblocked under explicit, documented conditions — E21 can proceed.
- The two genuinely-new/uncertain risks (#4868 host-token injection, paid distribution) are isolated behind explicit legal gates rather than shipping on optimistic readings.
- A data-governance default (API-key for captured content) is established for a surveillance product before it becomes load-bearing.
- The catalog gains machine-readable `tos_notes`/`usage_attribution`/`host_injects_token` so the policy is enforceable, not just prose.

### Negative

- Two follow-ups are required before broad rollout: 429 back-off classification and the `host_injects_token` CI lint.
- Some genuinely useful flows (unattended autonomous turns on a consumer subscription) are deliberately steered to API-key, which costs the user usage-based billing instead of a flat subscription.
- The MEDIUM-confidence ToU quotes mean the *restrictive* conclusions rest on synthesized wording that must be re-confirmed; the *permissive* clearance does not depend on them.

### Neutral

- The exec path keeps `preferred_for_product_auth: true`; app-server remains experimental/flag-gated behind `CodexAppServerRollout` (Off default).

## Alternatives Considered

**A. Treat the whole `cli_subscription` Codex integration as BLOCKED until full legal sign-off.** Rejected: the transport/PoC adds no new credential surface beyond shipped exec code, and OpenAI explicitly endorses app-server for product integration; blocking the PoC would stall E21 on a risk that is not new and not the PoC's to resolve.

**B. Treat `cli_subscription` as fully ALLOWED on the OpenClaw/Altman precedent.** Rejected: that precedent is community-sourced, for a free OSS tool, and OpenAI never answered the paid-wrapper or autonomous-consumption questions. Reading it as a green light would launder unadjudicated risk into a cleared one.

**C. Ship `chatgptAuthTokens` host-token injection now (it's an offered experimental mode).** Rejected: structurally identical to the pattern Anthropic enforced against in Jan 2026; the "experimental" label is not a ship authorization for a distributed product.

## Known Follow-ups

1. **429 back-off classification (#4871 follow-up)** — classify 429/rate-limit/quota distinctly in `codex_app_server.rs`/`session.rs` so `factory.rs` does not degrade a quota wall into the same-quota exec path; back off + surface to user. Small/medium effort.
2. **`host_injects_token` CI lint** — machine-enforce `host_injects_token: false` on `cli_subscription` surfaces. Small effort.
3. **Authenticated ToU re-fetch** — lock exact wording of `openai.com/policies/row-terms-of-use`, account-sharing, and consumer-vs-API data-training defaults from an unblocked context; promote MEDIUM citations to HIGH. Required before any production sign-off of the restrictive conclusions.
4. **API-key surface preference for captured content / unattended turns** — wire the resolver so captured-content and unattended flows prefer the API-key surface over `cli_subscription`.
5. **Legal sign-off** for Decisions 4 (#4868) and 5 (paid distribution) before either ships.

## Related Docs

- `docs/architecture/ADR-024-conversation-content-guard-port.md` — `is_external()` chat-egress guard (necessary-not-sufficient control referenced above)
- `docs/architecture/ADR-021-config-consent-core-placement.md` — consent boundary
- `specs/providers/provider-surface-catalog.json` — the surfaces this ADR governs
- `developers.openai.com/codex/auth`, `developers.openai.com/codex/app-server` — HIGH-confidence auth-mode + endorsement sources
- `openai/codex` Discussion #8338 — the unanswered paid-wrapper question

---

## Update 2026-06-04 — Decision 4 deep-dive resolution (#5034): host-token injection = WON'T-IMPLEMENT

The original Decision 4 left `chatgptAuthTokens` host-token injection at **BLOCKED / NEEDS_LEGAL_SIGNOFF**. Issue #5034 ran the dedicated engineering ToS-risk research to reach a resolution. **This is an engineering ToS-risk analysis, not binding legal advice.**

### Decision: maekon WILL NOT implement `chatgptAuthTokens` host-token injection.

Resolve #5034 as a documented **wontfix**. Keep `host_injects_token: false` unchanged on both `cli_subscription` surfaces (`provider-surface-catalog.json` L854 + L1041) + the CI-lint follow-up. Three independent research lenses converged and survived adversarial review.

### Rationale

1. **No functional gap (verified in-repo).** There is no maekon scenario that *only* host-injection serves. The only "headless" path in the tree is a notification fallback (`agent_runtime_support.rs` `LogOnlyNotifier`) that suppresses desktop-notification UI for GUI-less builds — NOT a deployment where the user cannot run interactive `codex login`. Interactive subscription use is covered by **ChatGPT-managed OAuth** (the user's own `codex login`; Codex owns the flow + refresh; the token never leaves the user's machine; maekon never holds it). Unattended/automated + captured-content use is covered by the **API-key (Platform)** path (Decisions 3/6). Host-injection adds only "maekon custodies + refreshes the user's ChatGPT access token" — a risk with no capability maekon lacks today.

2. **Highest ToS risk — matches the exact pattern a major lab has enforced against.** OpenAI documents `chatgptAuthTokens` ONLY as experimental: *"experimental and intended for host apps that already own the user's ChatGPT auth lifecycle"* and *"Use this experimental mode only when a host application owns the user's ChatGPT auth lifecycle and supplies tokens directly"* (`developers.openai.com/codex/app-server`, HIGH; gated behind `capabilities.experimentalApi = true`, an opt-in to *unstable* functionality, not a production-ship authorization). OpenAI's auth docs describe only managed-OAuth + API-key for end users and warn to *"treat `~/.codex/auth.json` like a password … Don't … share it"* (`developers.openai.com/codex/auth`, HIGH). **OpenAI nowhere affirmatively permits a distributed third-party product to custody/inject a user's personal ChatGPT subscription token** — the "owns the user's auth lifecycle" qualifier most naturally reads as first-party/enterprise-owned identity contexts, not a consumer-distributed product. Meanwhile **Anthropic** (Jan 9 2026 enforcement; Feb 20 2026 clarification, HIGH) made the structurally-identical pattern an explicit violation: *"Using OAuth tokens obtained through Claude Free, Pro, or Max accounts in any other product, tool, or service — including the Agent SDK — is not permitted and constitutes a violation of the Consumer Terms of Service"*, enforced server-side (origin/fingerprint/user-agent/behavioral checks, per secondary reporting — The Register) and motivated by subscription-vs-API **token arbitrage** that applies identically to OpenAI subscriptions.

3. **No offsetting benefit.** Because (1) shows the alternatives already cover every need, the host-injection path is pure downside: it takes on token-custody + the Anthropic-precedent enforcement risk for zero new capability. (Note: OpenAI's tolerance of third-party tools like OpenCode/Cline is reported as a program-gated open-source-maintainer arrangement — secondary sources cite a GitHub-stars threshold; treat as unverified secondary — NOT a blanket license for any distributed product to host-inject.)

### Future-reconsideration preconditions (ALL must hold; none holds today → a NEW ADR, not this resolution)

(i) a concrete functional gap where interactive `codex login` is impossible AND the API-key path cannot serve the turn (refuted today); (ii) the end user personally supplies their OWN token; (iii) single-session custody, no relay/sharing; (iv) explicit **written OpenAI confirmation** that a distributed third-party product may host-inject a user's personal ChatGPT subscription token in production (not the experimental docs, not silence); (v) authenticated ToU re-fetch (Follow-up #3) + formal legal sign-off (Follow-up #5); (vi) if ever built, a DISTINCT catalog surface / `credential_kind` with `host_injects_token: true`, `stability: experimental`, `preferred_for_product_auth: false` + Anthropic-precedent `tos_notes` — never flipping the invariant on the existing `cli_subscription` surfaces.

### Confidence + residual

HIGH: `developers.openai.com/codex/{app-server,auth}` quotes (directly fetched) + the in-repo no-functional-gap finding. MEDIUM: `openai.com/policies/row-terms-of-use` account-sharing clause (page 403-blocks automated fetch → secondary-sourced; re-confirm per Follow-up #3) and the OpenCode program-gating threshold (secondary). The Anthropic clarification quote is HIGH (primary reporting). Residual legal items (Follow-up #3 authenticated ToU re-fetch; Follow-up #5 legal sign-off for Decisions 4+5) remain formally open should reconsideration ever be opened — but the engineering resolution stands independently: **don't build it.**

### Addendum 2026-06-04 (epic #4861 pre-close review — fact corrections)

Two statements in the original Decision/§7 body went stale or over-claimed after later E21 PRs; corrected here rather than by rewriting the dated body:

- **§1 / Condition 1 "identical `auth_probe_mode: codex_login_status_text`"** — superseded by #4868: the `provider_surface.openai.codex_app_server` surface now probes auth via `auth_probe_mode: codex_account_read_json` (structured read-only `account/read`); only the `exec` surface (`provider_surface.openai.subprocess_cli`) keeps `codex_login_status_text`. This does NOT change Decision 1 — both remain `credential_kind: cli_subscription`, the CLI still owns the OAuth flow, and the app-server surface adds no new ToS credential surface; the "identical" wording referred to the credential model (still true), only the probe mechanism differs.
- **§7 "`host_injects_token: false` … enforced by a CI lint"** — present-tense over-claim: the catalog field is set, but the CI lint does NOT yet exist (it is a Known Follow-up, not a landed control). Read §7 as "to be enforced by a CI lint (follow-up)". Tracked with the §7 `row-terms-of-use` references-array addition, which was also not applied to the catalog.
