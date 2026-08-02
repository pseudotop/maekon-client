[English](./market-positioning-references.md) | [한국어](./market-positioning-references.ko.md)

# Market Positioning References

> Last updated: 2026-07-30

## Purpose

This document records the **public market category** Maekon operates in, the closest comparable products that entered the same problem space in 2026, and the four axes on which Maekon differentiates. It is the canonical positioning reference for README, landing copy, investor briefs, and any external messaging.

This document is **not** an ADR — it captures market context, not architectural decisions. Architectural axes that follow from this positioning live in the ADR registry (`docs/architecture/`).

## Problem Space

**Ambient AI + screen context understanding** — AI that observes a user's screen, focus, and activity stream, then turns natural pointing/typing intent ("summarize this", "what's that", "organize") into structured suggestions or actions.

Two major actors entered this space in 2026:

| Actor | Product | Released | Surface |
|---|---|---|---|
| Google DeepMind | **AI Pointer** (Gemini-powered) | 2026-05 | Chrome (Gemini), Googlebook (Magic Pointer), Google Labs (Disco), Google AI Studio |
| OpenAI | **Codex Chronicle** (Recall-like memory) | 2026-04 | macOS only, ChatGPT Pro subscription (opt-in research preview) |

Source pointers:
- DeepMind AI Pointer: https://deepmind.google/blog/ai-pointer/
- OpenAI Codex Chronicle: https://developers.openai.com/codex/memories/chronicle

## DeepMind AI Pointer — 4 Design Principles (Reference)

DeepMind articulates four design principles for AI Pointer that align closely with the broader category:

1. **Maintain the Flow** — works across all apps; users should not "detour" out of their workflow to use AI
2. **Show and Tell** — capture the visual and semantic context around what the user points at
3. **The power of "this/that"** — natural shorthand reference works without re-typing context
4. **Pixels → Actionable Entities** — convert raw pixels into structured entities the system can act on

Quoted: *"AI capabilities should work across all apps, not force users into 'AI detours' between them."*

Maekon adopts these as the **target experience** for the work-signal layer, while differentiating on four operational axes below.

## OpenHuman (tinyhumansai) — Closest Local-First Comparable (2026-07-29 scan)

**Product**: OpenHuman — a Tauri+Rust+SQLite local-first "personal AI super intelligence" (launched 2026-05, GPL-3.0). Closest stack-and-category comparable to Maekon; its context-acquisition axis is the inverse of ours (it READS ACCOUNTS via 100+ OAuth connectors on a 20-minute pull loop; Maekon OBSERVES ACTIVITY via passive local capture).

**Signal-quality verdict (mandatory reading before citing its metrics)**:

- Its star/trending metrics are NOT community validation. Full-archive verification (2026-07-29): every Hacker News submission about it (5 total, including the maintainer's own Show HN and a launch-day third-party post) died at 2–4 points with ~1 comment total; zero indexed Reddit threads; zero organic X reactions beyond the maintainer. Most English "coverage" is SEO content farms echoing the trending rank. Growth is attributable to non-anglophone channels plus a trending→SEO→stars amplification loop. **Do not cite its stars, trending rank, or Product Hunt position as demand or product evidence.**
- Valid evidence = two hands-on reviews only: 요즘IT (direct use; yozm.wishket.com/magazine/detail/3870) and OpenAIToolsHub (14-day self-host). Both converge on the same points: the single most-praised property is the **inspectable memory vault** ("you can open the vault and see exactly what the agent knows — nobody else does"); the consistent shortfalls are stability and speed.
- Its issue tracker's real user signal (~83% of recent traffic is core-team work tickets; top-reacted community issues): the #1 grievance is **forced online accounts in a "local-first" product**; two user-conducted code audits found unconsented phone-home to its own backend and OAuth handshakes routed through a hosted aggregator (Composio). Quote from that thread: *"'local-first' and 'connected to your tools' pull in opposite directions and the marketing copy almost always papers over the seam."*

**What this validates for Maekon** (external evidence for decisions already made):

1. **The vault bet** — ADR-033's user-owned Markdown mirror lands on the property hands-on reviewers praise most, grounded in real usage signal, not star counts.
2. **No account required** — the community's loudest OpenHuman complaint is exactly the axis Maekon refuses: Maekon's standalone success path requires no online account, no OAuth breadth, no aggregator middleman (MK-EXT-01: first-party read-only connectors only).
3. **Egress honesty** — OpenHuman's trust collapse came from silent phone-home discovered by user audits; Maekon's egress ledger + receipt-only/non-PII telemetry + ADR-033 §3.4 cloud-path ledger records are the preemptive public answer. State this in public surfaces rather than waiting to be audited.

Source pointers: github.com/tinyhumansai/openhuman (issues #1977, #2020, #2422 for the trust findings).

## Maekon's 4 Differentiation Axes

| Axis | DeepMind AI Pointer | OpenAI Codex Chronicle | **Maekon** |
|---|---|---|---|
| **Default data path** | Cloud-bound (Gemini) | Cloud-bound (OpenAI servers process screenshots) | **Local-first by default**, on-device. Cloud round-trips are opt-in. |
| **Audit and traceability** | Not publicly documented | Memories stored **unencrypted** on disk | **Source-first audit** — every signal carries origin, retention, PII-filter trace |
| **Automation boundary** | Natural intent → **direct action** | Memory-only (Codex still acts) | Natural intent → **next-action candidates** with explicit review/approval gate (policy-gated) |
| **Platform reach** | Chrome / Gemini / Googlebook (Google ecosystem) | macOS only / ChatGPT Pro subscription / EU/UK/CH excluded | **3 OS** (macOS, Windows, Linux), Apache-2.0, ecosystem-neutral |

## Trust-First Differentiator Dimensions

Beyond the four competitive axes above, Maekon's trust-first wedge is expressed through five user-visible dimensions. Each is a design commitment the desktop client should prove to the user directly — visible provenance, consent, audit, and lightweight evidence — rather than a background claim:

| Dimension | What the user can verify | Why it matters |
|---|---|---|
| **Retrieval trust** | With AI features enabled (opt-in; off by default and applied after restart), past work context returns with visible provenance — frame, time range, app/window, and source snippet — with sensitive content redacted in index, results, and exports; low-confidence recall asks for clarification instead of fabricating | Searchable past context is only trustworthy when it is auditable and redacted, not opaque |
| **Agent-safety confirmation** | Captured screen/web/app content is treated as untrusted context and cannot override intent; sensitive actions (payment, credentials, file/email mutation, destructive automation) require explicit confirmation with a clear action summary; allowlist denials stay visible | Screen-reading agents are exposed to prompt injection, so meaningful actions need human confirmation |
| **Data-control visibility** | Retention, export, deletion, external egress, and provider-training policy are shown in plain language; export sanitizes app/window/OCR fields and fails closed; sharing defaults stay private | Users should be able to control their data without reading architecture docs |
| **Audio & bystander consent** | Audio/STT stays off until explicit consent, recording scope and external STT egress are explained, a recording notice or bystander guidance appears before always-on capture, and revoke purges buffers | Ambient audio capture with on-device transcription (no meeting detection or summarization) is judged on consent, notice, retention, and deletion |
| **Evidence readability** | Capture border, pointer halo, and click ripple stay readable without blocking the app; Computer Use and Maekon cursors stay distinct with no duplicate trace; reduced-motion keeps static pointer evidence | Readable, honest capture evidence builds trust in what was observed |

These dimensions describe the **target experience** for the trust layer and are the public-safe framing of Maekon's differentiation. Detailed release-gate execution evidence is maintained in the parent-internal QA process and is intentionally not part of this public document.

## Vocabulary Alignment

The vocabulary used in Maekon's user-facing surface and the broader market frame:

| Maekon surface | DeepMind frame (reference) | Equivalent meaning |
|---|---|---|
| "next-action candidates" | "Pixels → Actionable Entities" (principle #4) | Convert observed context into discrete, actionable suggestions |
| "policy-gated action paths" | "Maintain the Flow" + audit constraints | Suggestions stay inside review boundaries |
| "edge processing" | "Show and Tell" + on-device | Pre-process locally before any cloud round-trip |
| "delta encoding" | (Maekon-specific) | Send only changes between frames to keep bandwidth low |

Maekon can also be described with the enterprise vocabulary **"pointed context → actionable entity"**. Both phrases map to the same public mechanism — local work signals + focus timeline + screen/OCR edge → reviewable candidate flow. Surface vocabulary differs by audience, but the product boundary remains the same Apache-2.0 desktop client.

## Why Not Direct Competition

Maekon does not position as a head-to-head replacement for DeepMind AI Pointer or Codex Chronicle. Each addresses the same problem space from a different ecosystem assumption:

- DeepMind binds the experience to Google's cloud + browser stack.
- OpenAI Codex Chronicle binds to ChatGPT Pro + macOS, with memory stored unencrypted.
- Maekon's bet is that **a meaningful share of users and organizations require local-first defaults, source-first audit trails, and policy gates before they can adopt any of this**, especially in regulated sectors (finance, manufacturing, healthcare, public sector).

This is a **category-adjacent differentiation**, not direct competition.

## Cross-References

- Maekon README: see `## Why Maekon → Market positioning (2026)`
- Public source pointers above remain the canonical public references for this OSS document.

## Update Policy

Refresh this document when:
- A new comparable product enters the ambient AI + screen context space
- Maekon's 4 axes change (e.g., dropping local-first default, adding cloud-only mode)
- DeepMind or OpenAI public stance shifts (link breaks, principles updated)

Companion: [market-positioning-references.ko.md](./market-positioning-references.ko.md)
