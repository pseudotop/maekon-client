[English](./market-positioning-references.md) | [한국어](./market-positioning-references.ko.md)

# Market Positioning References

> Last updated: 2026-05-14

## Purpose

This document records the **public market category** Maekon operates in, the closest comparable products that entered the same problem space in 2026, and the four axes on which Maekon differentiates. It is the canonical positioning reference for README, landing copy, investor briefs, and any external messaging.

This document is **not** an ADR — it captures market context, not architectural decisions. Architectural axes that follow from this positioning live in the ADR registry (`docs/architecture/`).

## Problem space

**Ambient AI + screen context understanding** — AI that observes a user's screen, focus, and activity stream, then turns natural pointing/typing intent ("summarize this", "what's that", "organize") into structured suggestions or actions.

Two major actors entered this space in 2026:

| Actor | Product | Released | Surface |
|---|---|---|---|
| Google DeepMind | **AI Pointer** (Gemini-powered) | 2026-05 | Chrome (Gemini), Googlebook (Magic Pointer), Google Labs (Disco), Google AI Studio |
| OpenAI | **Codex Chronicle** (Recall-like memory) | 2026-04 | macOS only, ChatGPT Pro subscription (opt-in research preview) |

Source pointers:
- DeepMind AI Pointer: https://deepmind.google/blog/ai-pointer/
- OpenAI Codex Chronicle: https://developers.openai.com/codex/memories/chronicle

## DeepMind AI Pointer — 4 design principles (reference)

DeepMind articulates four design principles for AI Pointer that align closely with the broader category:

1. **Maintain the Flow** — works across all apps; users should not "detour" out of their workflow to use AI
2. **Show and Tell** — capture the visual and semantic context around what the user points at
3. **The power of "this/that"** — natural shorthand reference works without re-typing context
4. **Pixels → Actionable Entities** — convert raw pixels into structured entities the system can act on

Quoted: *"AI capabilities should work across all apps, not force users into 'AI detours' between them."*

Maekon adopts these as the **target experience** for the work-signal layer, while differentiating on four operational axes below.

## Maekon's 4 differentiation axes

| Axis | DeepMind AI Pointer | OpenAI Codex Chronicle | **Maekon** |
|---|---|---|---|
| **Default data path** | Cloud-bound (Gemini) | Cloud-bound (OpenAI servers process screenshots) | **Local-first by default**, on-device. Cloud round-trips are opt-in. |
| **Audit and traceability** | Not publicly documented | Memories stored **unencrypted** on disk | **Source-first audit** — every signal carries origin, retention, PII-filter trace |
| **Automation boundary** | Natural intent → **direct action** | Memory-only (Codex still acts) | Natural intent → **next-action candidates** with explicit review/approval gate (policy-gated) |
| **Platform reach** | Chrome / Gemini / Googlebook (Google ecosystem) | macOS only / ChatGPT Pro subscription / EU/UK/CH excluded | **3 OS** (macOS, Windows, Linux), Apache-2.0, ecosystem-neutral |

## Vocabulary alignment

The vocabulary used in Maekon's user-facing surface and the broader market frame:

| Maekon surface | DeepMind frame (reference) | Equivalent meaning |
|---|---|---|
| "next-action candidates" | "Pixels → Actionable Entities" (principle #4) | Convert observed context into discrete, actionable suggestions |
| "policy-gated action paths" | "Maintain the Flow" + audit constraints | Suggestions stay inside review boundaries |
| "edge processing" | "Show and Tell" + on-device | Pre-process locally before any cloud round-trip |
| "delta encoding" | (Maekon-specific) | Send only changes between frames to keep bandwidth low |

> An equivalent canonical vocabulary in the wider ONESHIM SSOT is **"pointed context → actionable entity"** (used in submission materials and investor decks). Both vocabularies map to the same underlying mechanism — local work signals + focus timeline + screen/OCR edge → reviewable candidate flow. Surface vocabulary differs by audience (developer/user frame vs. evaluator/investor frame).

## Why not direct competition

Maekon does not position as a head-to-head replacement for DeepMind AI Pointer or Codex Chronicle. Each addresses the same problem space from a different ecosystem assumption:

- DeepMind binds the experience to Google's cloud + browser stack.
- OpenAI Codex Chronicle binds to ChatGPT Pro + macOS, with memory stored unencrypted.
- Maekon's bet is that **a meaningful share of users and organizations require local-first defaults, source-first audit trails, and policy gates before they can adopt any of this**, especially in regulated sectors (finance, manufacturing, healthcare, public sector).

This is a **category-adjacent differentiation**, not direct competition.

## Cross-references

- ONESHIM SSOT competitor scan: [K27] DeepMind AI Pointer, [K28] OpenAI Codex Chronicle (full entries in the parent submission package at `plan/_shared/references/competitor.md`)
- ONESHIM product positioning: `plan/_shared/제품현황.md` v2.22+ "Maekon 어휘 통합 정책" section
- Maekon README: see `## Why Maekon → Market positioning (2026)`

## Update policy

Refresh this document when:
- A new comparable product enters the ambient AI + screen context space
- Maekon's 4 axes change (e.g., dropping local-first default, adding cloud-only mode)
- DeepMind or OpenAI public stance shifts (link breaks, principles updated)

Companion: [market-positioning-references.ko.md](./market-positioning-references.ko.md)
