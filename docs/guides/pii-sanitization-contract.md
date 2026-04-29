# PII Sanitization Contract

**Status**: Accepted 2026-04-20 (D5 PII Filter Audit)
**Scope**: All text-producing adapters + their downstream write/send sites across the `client-rust` workspace
**Related**: D5 design spec and audit matrix are archived as internal implementation records.

## The Rule

**Every text value that crosses a persistence or transmission boundary MUST be sanitized.**

A "boundary" is any point where data leaves the in-memory trust domain of the running agent process:

| Boundary | Examples |
|----------|----------|
| SQLite write | `frames.ocr_text`, `local_suggestions.content`, `ai_sessions.state`, `coaching_events.personalized_message` |
| Server upload | `BatchUploader::enqueue` events, feedback submissions, telemetry reports |
| External API request body | LLM provider chat, OCR provider vision, embedding provider, audio STT cloud |
| Cross-device sync egress | `SyncExtractor` serialized payload (see exemption) |
| Audit log entry | `AuditLogger::record` command outputs |
| Structured `tracing` field value | The `user_input` / `message` fragment logged, NOT the `err.code` (which is PII-free by design) |
| Desktop notification body | Title + body rendered via `DesktopNotifier::show_notification` |
| Export files | CSV / JSON / iCal produced by `/api/export/*` handlers |

## Sanitization Level Resolution

Primary source: `config.privacy.pii_filter_level` — user-configurable 4-tier cascade:

| Level | Masks |
|-------|-------|
| `Off` | Nothing (sanitization bypass; audit-loggable choice) |
| `Basic` | Email, phone |
| `Standard` | All of Basic + credit cards, Korean ID, SSN, IBAN, user paths |
| `Strict` | All of Standard + API keys, IP addresses, passports |

External-path boundaries MAY upgrade the level (e.g., `ExternalDataPolicy::PiiFilterStrict` enforces Strict when sending to external AI providers) but MUST NOT downgrade below the user's configured level.

## How to Apply

### In `src-tauri/` binary crate

```rust
use oneshim_vision::privacy::sanitize_title_with_level;
use oneshim_core::config::PiiFilterLevel;

// At the boundary:
let sanitized = sanitize_title_with_level(&raw_text, pii_filter_level);
storage.save(&sanitized)?;
```

### In adapter crates (`oneshim-network`, `oneshim-audio`, `oneshim-automation`, `oneshim-analysis`, `oneshim-monitor`)

Per the repository's hexagonal architecture guardrails (forbidden: direct dependency between adapter crates), MUST inject the sanitizer via port trait:

```rust
use oneshim_core::ports::pii_sanitizer::PiiSanitizer;
use std::sync::Arc;

pub struct MyAdapter {
    // ... existing fields ...
    pii_sanitizer: Option<Arc<dyn PiiSanitizer>>,
    pii_filter_level: PiiFilterLevel,
}

impl MyAdapter {
    pub fn with_pii_sanitizer(mut self, s: Arc<dyn PiiSanitizer>) -> Self {
        self.pii_sanitizer = Some(s);
        self
    }

    fn sanitize(&self, text: &str) -> String {
        self.pii_sanitizer
            .as_ref()
            .map(|s| s.sanitize_text(text, self.pii_filter_level))
            .unwrap_or_else(|| text.to_string())
    }
}
```

At DI time in `src-tauri/src/main.rs` or `agent_runtime_support.rs`:

```rust
let sanitizer: Arc<dyn PiiSanitizer> =
    Arc::new(oneshim_vision::privacy::VisionPiiSanitizer);
let adapter = MyAdapter::new(...).with_pii_sanitizer(sanitizer.clone());
```

## Exemptions

A path MAY be exempt from sanitization under these conditions:

1. **Intra-process boundaries** — e.g., OCR text flowing between in-memory regex matcher and downstream summarizer within the same agent process. The text must exist somewhere for the pipeline to function; sanitization is applied at the NEXT persistence / transmission boundary.

2. **User-authored content intentionally submitted** — bug reports, chat messages to LLM, manual playbook contents. The user is explicitly sharing text by typing it; sanitizing destroys diagnostic value. If such content flows to a secondary boundary (e.g., chat history → SQLite → sync), sanitization applies at that secondary boundary.

3. **Cross-device sync payload** — receiver is another device owned by the same user; transport is encrypted end-to-end (see `sync/sync_crypto.rs`). Sanitizing here destroys the sync feature's value.

4. **Secret projection paths** — `ProcessEnvSecretProjection` and similar paths INTENTIONALLY carry secrets to their consumers by design. They are the PII-handling infrastructure, not a leak.

### Exemption documentation

Every exempted path MUST include:

- A `// PII-EXEMPT: <reason>` comment at the boundary site
- A row in the internal PII audit matrix stating the exemption rationale
- A regression test confirming the exemption is intentional (e.g., asserting the raw text flows through vs. getting sanitized)

Silent bypasses are not permitted.

## Regression Testing

Each fix site MUST have a contract test in `src-tauri/tests/pii_sanitization_contract.rs` that:

1. Constructs an input containing known PII (e.g., `"user@example.com"`)
2. Routes it through the production code path
3. Asserts the output at the boundary contains the expected marker token (`[EMAIL]`, `[PHONE]`, `[USER]`, etc.)
4. Asserts the raw PII is NOT present

Tests should FAIL on pre-fix `main` and PASS after the fix lands — this pattern proves the fix addresses a real gap rather than a theoretical one.

## Consequences

- Users retain privacy when their OCR, clipboard, accessibility text, or LLM responses are persisted locally or transmitted externally.
- Silent regressions are caught by the contract test suite.
- New text-producing adapters gain a clear protocol for integration: add `PiiSanitizer` injection + a contract test.

## Change process

Adding a new text-producing adapter or a new boundary:

1. Identify the boundary (the persistence/transmission point)
2. Apply sanitization per the patterns above
3. Add a contract test
4. Add a row to the internal PII audit matrix
5. Reference this contract doc in the adapter's module-level docs
