[English](./first-5-minutes.md) | [한국어](./first-5-minutes.ko.md)

# First 5 Minutes Guide

Use this checklist to get the first usable insight from MAEKON quickly in standalone mode.

## 1. Launch standalone mode

```bash
cargo run -p maekon-app -- --offline
```

Expected: app starts without server/auth dependency.

## 2. Open local dashboard

- URL: `http://localhost:10090`
- Verify dashboard panels load (metrics, timeline, focus).

## 3. Keep privacy baseline

In Settings:
- keep sandbox enabled (`Standard` or `Strict`)
- set `external_data_policy` to `PiiFilterStandard` or stricter
- keep `allow_unredacted_external_ocr=false`

## 4. Run one workflow preset

Start with one preset:
- `daily-priority-sync`
- `deep-work-start`

Expected: automation audit entries appear with success/blocked signals.

## 5. Capture first support bundle

Query:
- `GET /api/onboarding/quickstart`
- `GET /api/support/diagnostics`
- `GET /api/automation/policy-events?limit=50`

Expected: a reproducible snapshot of settings, health, and policy actions for tuning.

## 6. Turn on AI features (optional)

AI features ship **off by default** (privacy-first). To unlock local embedding, AI semantic search, and the AI daily-digest narrative, open **Settings → Advanced → Enable AI features** — a single master toggle (individual toggles stay available for power users).

Everything runs **on-device**: local embedding builds the vector index, and the daily digest's narrative is written by the local LLM.

Note: these flags are read at startup, so **restart Maekon** for the pipeline to take effect. After restart, the Search page's **Semantic** mode becomes selectable and the daily digest gains an AI-written narrative.
