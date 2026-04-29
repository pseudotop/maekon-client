[English](./ADR-003-directory-module-pattern.md) | [한국어](./ADR-003-directory-module-pattern.ko.md)

# ADR-003: Directory Module Pattern for Large Source Files

**Status**: Accepted
**Date**: 2026-02-27
**Scope**: All crates in the workspace

---

## Context

Several source files in the workspace exceeded 500 lines, making them difficult to navigate, review, and maintain. The server-side codebase had already adopted a similar pattern (server ADR-013: Domain Service Folder Pattern) for Python modules exceeding 500 lines with positive results across 5 domains.

Files identified at the time of this decision:

| File | Lines | Crate |
|------|-------|-------|
| `handlers/automation.rs` | 1,558 | oneshim-web |
| `controller.rs` | 1,465 | oneshim-automation |
| `updater.rs` | 1,418 | oneshim-app |
| `config.rs` | 1,382 | oneshim-core |
| `app.rs` | 1,227 | oneshim-ui |
| `scheduler.rs` | 1,067 | oneshim-app |
| `focus_analyzer.rs` | 859 | oneshim-app |
| `policy.rs` | 815 | oneshim-automation |
| `gui_interaction.rs` | 750 | oneshim-automation |

`main.rs` (726 lines) was excluded — it is a binary entry point with sequential DI wiring where splitting would scatter the composition logic with no clear responsibility boundary.

---

## Decision

### 1. Convert large files to directory modules

When a Rust source file exceeds **500 lines**, convert it from a single file (`foo.rs`) to a directory module (`foo/mod.rs` + focused sub-files).

### 2. Preserve external API via `pub use` re-exports

`mod.rs` must re-export all public symbols so that **every existing import path continues to compile without changes**. No downstream consumer should need modification after a split.

```rust
// foo/mod.rs
mod helpers;
mod types;

pub use helpers::*;
pub use types::*;
```

### 3. Use `pub(super)` for internal items

Items shared across sub-files within the directory but not intended for external use must use `pub(super)` visibility.

```rust
// foo/helpers.rs
pub(super) fn require_config_manager(state: &AppState) -> Result<&ConfigManager, ApiError> {
    // ...
}
```

### 4. Keep tests in `mod.rs` (with large-test exception)

All `#[cfg(test)] mod tests` blocks remain in `mod.rs` when they are under 1,000 lines. Tests naturally exercise the module's public interface and serve as documentation of expected behavior at the module boundary.

**Exception — tests exceeding 1,000 lines**: When the test block alone exceeds 1,000 lines, extract tests into a `tests/` sub-directory within the module directory:

```
foo/
├── mod.rs           # Production code + #[cfg(test)] mod tests { mod tests; }
├── service.rs
└── tests/
    ├── mod.rs       # Shared mocks, fixtures, helpers
    ├── feature_a.rs # Tests grouped by feature/concern
    └── feature_b.rs
```

The `tests/mod.rs` file contains shared test infrastructure (mocks, fixture builders, constants). Individual test files are organized by functional area. This keeps `mod.rs` focused on production code while maintaining test discoverability through the module hierarchy.

### 5. SOLID principles take priority over line counts

**A file should be split when it violates the Single Responsibility Principle (SRP), not because it exceeds a line count.** A well-structured 1,000-line file with one clear responsibility is better than three 300-line files with tangled concerns.

The 500-line threshold is a **signal to review**, not a trigger to split:
- **>500 lines → review for SRP violations.** If the file has one cohesive responsibility, leave it as-is.
- **SRP violation at any size → split by responsibility.** Even a 200-line file with two unrelated concerns should be split.

Sub-files are organized by functional responsibility:

- **types/models**: Data structures, enums, DTOs
- **helpers**: Private utility functions
- **feature groups**: Logically cohesive handler/method groups (e.g., `scene.rs`, `execution.rs`, `intent.rs`, `preset.rs`)

### 6. Threshold and exclusions

- **Threshold**: 500 lines (soft guideline — a review signal, not a split trigger)
- **Priority**: SOLID compliance > line count. Do not split files mechanically by size alone.
- **Excluded**: `main.rs` and similar binary entry points where sequential composition logic is the primary concern
- **Not retroactive**: Files under 500 lines should not be split preemptively

---

## Applied Splits

| Original File | Target Structure | Crate |
|---------------|-----------------|-------|
| `gui_interaction.rs` | `gui_interaction/{mod, types, crypto, helpers, service}.rs` | oneshim-automation |
| `policy.rs` | `policy/{mod, models, token}.rs` | oneshim-automation |
| `controller.rs` | `controller/{mod, types, intent, preset}.rs` | oneshim-automation |
| `focus_analyzer.rs` | `focus_analyzer/{mod, models, suggestions}.rs` | oneshim-app |
| `scheduler.rs` | `scheduler/{mod, config, loops}.rs` | oneshim-app |
| `updater.rs` | `updater/{mod, github, install, state}.rs` | oneshim-app |
| `config.rs` | `config/{mod, enums, sections}.rs` | oneshim-core |
| `handlers/automation.rs` | `handlers/automation/{mod, helpers, scene, execution}.rs` | oneshim-web |
| `app.rs` | `app/{mod, message, update, view}.rs` | oneshim-ui |
| `gui_interaction/mod.rs` tests | `gui_interaction/tests/{mod, session, highlight, confirm, execute, m5}.rs` | oneshim-automation |
| `scheduler/loops.rs` | `scheduler/loops/{mod, helpers, monitor, system, network, intelligence, events, sync}.rs` | src-tauri |
| `provider_adapters.rs` | `provider_adapters/{mod, types, guarded_ocr, surface, ocr_resolver, llm_resolver, helpers, tests}.rs` | src-tauri |
| `commands.rs` | `commands/{mod}.rs` | src-tauri |
| `gui_detector.rs` | `gui_detector/{mod, correlation, inference, tests}.rs` | oneshim-vision |
| `accessibility/macos.rs` | `accessibility/macos/{mod, extractor, observer, tests}.rs` | oneshim-vision |
| `ai_llm_client.rs` | `ai_llm_client/{mod, request, parsers, tests}.rs` | oneshim-network |
| `ai_ocr_client.rs` | `ai_ocr_client/{mod, ollama, parsers, strategy, tests}.rs` | oneshim-network |
| `sync/lan_server.rs` | `sync/lan_server/{mod, handlers, session, tls, tests}.rs` | oneshim-network |
| `sync/lan_transport.rs` | `sync/lan_transport/{mod, auth, operations, tests}.rs` | oneshim-network |
| `integration/http_transport.rs` | `integration/http_transport/{mod, connect, egress, inbox, tests}.rs` | oneshim-network |
| `integration/auth.rs` | `integration/auth/{mod, oidc_device_flow, proof_factory, static_auth, tests}.rs` | oneshim-network |
| `provider_specs.rs` | `provider_specs/{mod, enums, models, helpers, parsers, queries, resolvers, validation, tests}.rs` | oneshim-api-contracts |
| `coaching_engine.rs` | `coaching_engine/{mod, guards, triggers}.rs` | oneshim-analysis |
| `coaching_template.rs` | `coaching_template/{mod, templates}.rs` | oneshim-analysis |

---

## Consequences

### Positive

- Each sub-file is under 300 lines, improving navigation and code review
- `cargo test/clippy/fmt` continue to pass without any logic changes
- External API paths are fully preserved — zero downstream breakage
- Consistent with the server-side ADR-013 folder pattern, reducing cognitive overhead across the monorepo

### Tradeoffs

- Minor increase in file count (9 files become ~35 files)
- Developers must understand `pub(super)` and re-export patterns
- `mod.rs` files carry re-export boilerplate

### Risks

- `pub use *` re-exports may unintentionally expose items added later. Mitigated by code review and `pub(super)` discipline on internal items.

---

## Related Docs

- Server ADR-013: `server/docs/architecture/ADR-013-domain-service-folder-pattern.md`
- `docs/architecture/ADR-001-rust-client-architecture-patterns.md`
- Repository guardrails — crate summaries document each directory module structure
