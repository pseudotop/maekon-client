# Maekon Contributing Guide

Thanks for your interest in Maekon. This document is the Rust-specific guide for contributing to the Cargo workspace.

Brand and compatibility note: Maekon is the user-facing display name. Repository
URLs, crate/package names, Cargo package names, the `maekon` CLI command,
`MAEKON_*` environment variables, and existing config/data paths remain
`maekon` technical identifiers for compatibility.

## Public Contribution Model

Maekon Client is published from a parent-internal source of truth. Public
contributions are welcome, but accepted changes may be imported into the parent
source tree for full validation before the public repository is regenerated.

Use this model when deciding where a contribution fits.

| Lane | Good public PR candidates | Extra review required |
|------|---------------------------|-----------------------|
| `docs-dx` | Documentation fixes, setup notes, typo fixes, clearer examples | No private paths, maintainer-only validation names, or internal roadmap references |
| `i18n` | Locale parity and copy consistency | UI text must stay resource-driven |
| `examples` | Synthetic examples, local-only playbooks, sample configs | No secrets, private screenshots, raw capture data, or real user content |
| `local-ui` | Dashboard and settings refinements that do not change capture, egress, or consent semantics | UI evidence and accessibility notes are expected |
| `provider-adapter` | Public provider metadata/spec updates | No unmanaged egress or embedded credentials |
| `trust-core` | Consent, PII masking, capture, audio, automation policy, sandbox, updater, release signing | Maintainer security/privacy review and private validation are required |

If you are unsure whether a change is `trust-core`, open an issue or discussion
before writing a large patch. Security vulnerabilities must be reported through
the private channel in `SECURITY.md`, not as public issues or PRs.

Maintainers triage public issues and PRs with the labels, CODEOWNER rules, and
branch protection settings documented in
[`docs/guides/public-contribution-governance.md`](./docs/guides/public-contribution-governance.md).
Use that guide when selecting a lane, deciding whether a hold label is needed,
or checking whether a patch must be imported into the parent source tree before
release.

For a contributor-facing overview of the safe public PR lifecycle, evidence
expectations, and maintainer handoff language, see
[`docs/guides/public-contributor-path.md`](./docs/guides/public-contributor-path.md).

When a public PR is accepted for parent validation, maintainers use
[`docs/guides/hybrid-import-workflow.md`](./docs/guides/hybrid-import-workflow.md)
to preserve public PR links, author attribution, validation status, and the
public export handoff.

## Evidence and Data Safety

Every PR should explain what changed, why it changed, and how it was validated.

- Use synthetic fixtures for examples and tests.
- Do not upload secrets, tokens, private screenshots, raw screen/audio/input
  captures, customer data, private logs, or local absolute paths.
- For UI/runtime changes, include privacy-safe evidence such as redacted
  screenshots, sanitized logs, command output, or before/after behavior notes.
- For tests, prefer assertions that verify the returned value or specific error,
  not just success/failure.

## AI-Assisted Contributions

AI-assisted contributions are allowed when the human author remains responsible
for the patch.

- Disclose AI assistance in the PR description when it materially shaped the
  code, tests, or docs.
- Review, run, and understand the generated changes before submitting.
- Do not paste private project context, private test data, secrets, screenshots,
  or user captures into external AI tools.
- Do not answer maintainer review comments with unverified AI output. Reproduce,
  test, and respond from your own understanding.

## Legal Posture

By contributing, you agree that your contribution is licensed under the Apache
License 2.0, matching this repository's license.

For ordinary community contributions, Maekon uses an inbound-equals-outbound
Apache-2.0 posture. Before a public contribution is imported into the parent
source tree, maintainers require either a `Signed-off-by` line or a
maintainer-approved legal attestation. Public PRs that do not yet have an
automated DCO or CLA required check stay under the `do-not-merge/dco` hold until
that manual verification is recorded. Large corporate, patent-sensitive, or
ownership-sensitive contributions may be asked to use an additional CLA path
before acceptance. We do not require both DCO and CLA by default.

## Development Environment Setup

### Prerequisites

- **Rust** 1.77.1 or later (keep up to date with `rustup update stable`)
- **cargo** — Rust build system and package manager (included with Rust)
- **pnpm** — required to build the frontend web dashboard (`maekon-web/frontend`)

### Setup

```bash
# 1. Clone the repository
git clone https://github.com/pseudotop/maekon-client.git
cd maekon-client

# 2. Check dependencies and build
cargo check --workspace

# 3. Build the frontend (if including the web dashboard)
cd crates/maekon-web/frontend
pnpm install
pnpm build
cd ../../..

# 4. Full build
cargo build --workspace
```

### Optional Features

Some features are controlled via feature flags.

```bash
# Enable OCR (requires Tesseract)
cargo build -p maekon-vision --features ocr

# Enable gRPC client (tonic/prost)
cargo build -p maekon-network --features grpc
```

## Building

### Development Build

```bash
# Quick workspace verification
cargo check --workspace

# Development build
cargo build -p maekon-app

# Run in development mode
cargo run -p maekon-app
```

### Build with Frontend

The web dashboard embeds the React build output into the Rust binary.

```bash
# Step 1: Build the frontend
cd crates/maekon-web/frontend && pnpm install && pnpm build
# Or use the script
./scripts/build-frontend.sh

# Step 2: Build the Rust binary (automatically embeds dist/)
cargo build --release -p maekon-app
```

### Full Workspace Build

```bash
# Release build for all crates
cargo build --release --workspace
```

### Build Specific Crates

```bash
cargo build -p maekon-core
cargo build -p maekon-network
cargo build -p maekon-vision
```

## Code Style

### Formatting

All code follows `cargo fmt` default settings. Run it before submitting a PR.

```bash
# Apply formatting
cargo fmt --all

# Check formatting (same as CI)
cargo fmt --all -- --check
```

> **For external contributors:** like the hedge gate below, `cargo fmt` is a house-style gate — on the public repository it runs as an **advisory** check that annotates your PR but never blocks it (it is enforced on the source-of-truth repo). A maintainer may run `cargo fmt --all` while porting your change. Running it yourself before pushing keeps the diff clean.

### Lint

`cargo clippy` must report 0 warnings. If you need to suppress a warning, add `#[allow(...)]` to the specific item and explain why in a comment.

```bash
# Run clippy on the full workspace
cargo clippy --workspace

# Run with all features enabled
cargo clippy --workspace --all-features
```

### Test Assertions (hedge gate)

We avoid **value-blind assertion hedges** — `assert!(x.is_ok())` / `assert!(x.is_err())` — which prove only that a call succeeded or failed without checking *what* it returned. A test that asserts an error should assert *which* error.

```rust
// ❌ hedge — passes for the wrong error too
assert!(parse(input).is_err());

// ✅ assert the specific failure
let err = parse(input).unwrap_err();
assert!(matches!(err, ParseError::UnexpectedToken { .. }));

// ✅ for is_ok, bind the value and assert something about it
let cfg = load(path).expect("valid config loads");
assert_eq!(cfg.port, 8080);
```

Run the gate locally before pushing:

```bash
cargo test -p maekon-lint --test is_ok_hedge_gate --test is_err_hedge_gate
```

If a site is *genuinely* value-blind by design (e.g. the error type carries no payload, or any error is equally correct), add a one-line justification marker on the line above the assertion instead of strengthening it:

```rust
// lint:allow-is-err-hedge — justified: RecvError is a payload-less unit struct
assert!(rx.try_recv().is_err());
```

> **For external contributors:** this is a project convention, not a correctness requirement. On the public repository the hedge gate runs as an **advisory** check — it annotates your PR but never blocks it. A maintainer may ask you to strengthen a flagged assertion, or do it while porting your change. (Internally it is enforced on the source-of-truth repo.)

### Comments and Documentation

- **Code comments/docstrings should be written in English by default.** This is
  checked by the `language-check` gate, which scans comments (`//`, `///`, `//!`,
  `/* */`) in `.rs`/`.ts`/`.tsx` for non-English (non-Latin-script) letters:

  ```bash
  cargo run -p maekon-lint --bin language-check -- non-english --path crates --path src-tauri
  ```

  Only *comments* are checked — string literals (localized UI text, classifier
  keywords matched against non-English input, test data) are out of scope and may
  be any language. Punctuation, em-dashes, Greek/math symbols (`α`, `O(µs)`) and
  accented Latin are allowed. A file that legitimately needs non-English in comments
  (e.g. one documenting CJK text tokenization, where example tokens are illustrative)
  may opt out with a justified `lint:allow-non-english-comments` marker. Like the
  hedge gate, this is **advisory** on the public repo (annotates, never blocks) and
  enforced on the source-of-truth repo.
- **Public documentation is English-primary with multilingual companion docs (ko, ja, zh-CN, es) for key guides.**
- Add `///` doc comments to all `pub` items.
- Use inline comments (`//`) to explain intent in complex logic.
- For documentation governance, follow [docs/DOCUMENTATION_POLICY.md](./docs/DOCUMENTATION_POLICY.md).
- Korean companion policy: [docs/DOCUMENTATION_POLICY.ko.md](./docs/DOCUMENTATION_POLICY.ko.md)
- For mutable quality metrics, use the current GitHub Actions run page as the
  live source of truth.

```rust
/// Screen capture trigger — decides whether to capture based on event importance.
pub struct SmartCaptureTrigger {
    // Timestamp of last capture — used for throttling
    last_capture: Instant,
}
```

### Internationalization (i18n)

User-facing UI copy in the frontend (`crates/maekon-web/frontend`) must go through
react-i18next, not hardcoded literals:

```tsx
// ❌ hardcoded
<button>Save changes</button>
// ✅ i18n
const { t } = useTranslation()
<button>{t('settings.saveChanges')}</button>
```

Add the new key to **all five** locale files (`src/i18n/locales/{en,ko,ja,zh-CN,es}.json`)
— `en` is the source string; the others must stay key-synced (a missing key is a
build error). The gate checks this:

```bash
cargo run -p maekon-lint --bin language-check -- i18n --strict-i18n
```

`--strict-i18n` fails on hardcoded UI copy and unknown/missing keys. It does **not**
flag dynamic `t(`ns.${expr}`)` template-literal keys (legitimate) or non-prose values
(CSS classes, enum/wire values, format hints like `HH:MM`, code). A component whose
text is genuinely not product copy (e.g. a dev-only debug panel) can opt out with a
justified `lint:allow-hardcoded-ui` marker. Like the other house-style gates, this is
**advisory** on the public repo and enforced on the source-of-truth repo.

### Error Handling

- Library crates: use `thiserror` to define concrete error enums
- Binary crate (`maekon-app`): use `anyhow::Result`
- Wrap external crate errors with `#[from]`

```rust
#[derive(Debug, thiserror::Error)]
pub enum NetworkError {
    /// HTTP request failed
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// No auth token available
    #[error("no auth token")]
    NoToken,
}
```

### Async Traits

Apply `#[async_trait]` to all port traits. This is required for the `Arc<dyn PortTrait>` DI pattern.

```rust
use async_trait::async_trait;

#[async_trait]
pub trait ApiClient: Send + Sync {
    /// Uploads a context payload to the server.
    async fn upload_context(&self, context: &ContextPayload) -> Result<(), CoreError>;
}
```

## Architecture Rules

This project strictly follows **Hexagonal Architecture (Ports & Adapters)**. Please understand these rules before contributing.

### Core Principle

**`maekon-core` defines all port traits and domain models.** Adapter crates implement those ports, and `src-tauri/` (package name `maekon-app`) is the composition root.

```
maekon-core  (port definitions, models)
    <- maekon-monitor   (system monitoring adapter)
    <- maekon-vision    (image processing adapter)
    <- maekon-network   (HTTP/SSE/WebSocket adapter)
    <- maekon-storage   (SQLite adapter)
    <- maekon-suggestion <- maekon-network
    <- src-tauri          <- maekon-suggestion
    <- maekon-automation
    <- src-tauri package maekon-app (full DI wiring)
```

### Prohibited Patterns

Direct dependencies between adapter crates are not allowed. For example, `maekon-monitor` must not directly depend on `maekon-storage`. All cross-crate communication goes through traits defined in `maekon-core`.

Permitted exceptions:
- `maekon-suggestion` -> `maekon-network` (SSE reception)
- `src-tauri` -> `maekon-suggestion` (suggestion display)

### DI Pattern

Use constructor injection with `Arc<dyn T>`. No DI framework is used; all wiring is done manually in `src-tauri/src/main.rs`.

```rust
pub struct Scheduler {
    // Dependencies injected via Arc<dyn T> pattern
    monitor: Arc<dyn SystemMonitor>,
    storage: Arc<dyn StorageService>,
    api_client: Arc<dyn ApiClient>,
}

impl Scheduler {
    pub fn new(
        monitor: Arc<dyn SystemMonitor>,
        storage: Arc<dyn StorageService>,
        api_client: Arc<dyn ApiClient>,
    ) -> Self {
        Self { monitor, storage, api_client }
    }
}
```

## Adding New Features

Follow this order when adding new functionality.

### Step 1: Define a Port in core

Add a new trait under `crates/maekon-core/src/ports/`.

```rust
// crates/maekon-core/src/ports/my_service.rs

use async_trait::async_trait;
use crate::error::CoreError;

/// Port interface for the new feature
#[async_trait]
pub trait MyService: Send + Sync {
    /// Performs the operation.
    async fn do_something(&self, input: &str) -> Result<String, CoreError>;
}
```

### Step 2: Implement the Adapter

Implement the trait in the appropriate adapter crate.

```rust
// crates/maekon-xxx/src/my_impl.rs

use async_trait::async_trait;
use maekon_core::{ports::MyService, error::CoreError};

pub struct MyServiceImpl {
    // Fields needed for the implementation
}

#[async_trait]
impl MyService for MyServiceImpl {
    async fn do_something(&self, input: &str) -> Result<String, CoreError> {
        // Actual implementation
        todo!()
    }
}
```

### Step 3: Wire up DI in app

Connect the implementation to its port in `src-tauri/src/main.rs`.

```rust
// src-tauri/src/main.rs

let my_service: Arc<dyn MyService> = Arc::new(MyServiceImpl::new());
let scheduler = Scheduler::new(my_service, /* other dependencies */);
```

### Step 4: Write Tests

Write both unit tests and integration tests.

```rust
// Unit tests: place at the bottom of the relevant module
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_do_something() {
        let svc = MyServiceImpl::new();
        let result = svc.do_something("input").await;
        assert!(result.is_ok());
    }
}
```

## Writing Tests

### Principles

- **Do not use mockall.** Write mocks manually.
- Place tests in a `#[cfg(test)] mod tests` block at the bottom of each module.
- Implement port traits directly to create test mocks.

### Manual Mock Pattern

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::ports::ApiClient;

    // Test mock — only defined inside the #[cfg(test)] block
    struct MockApiClient {
        should_fail: bool,
    }

    #[async_trait::async_trait]
    impl ApiClient for MockApiClient {
        async fn upload_context(
            &self,
            _context: &ContextPayload,
        ) -> Result<(), CoreError> {
            if self.should_fail {
                Err(CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    message: "test failure".to_string(),
                })
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn upload_success_saves_event() {
        let client = Arc::new(MockApiClient { should_fail: false });
        // ... test logic
    }

    #[tokio::test]
    async fn upload_failure_triggers_retry() {
        let client = Arc::new(MockApiClient { should_fail: true });
        // ... test logic
    }
}
```

### Running Tests

```bash
# Full test suite
cargo test --workspace

# Specific crate
cargo test -p maekon-core
cargo test -p maekon-vision
cargo test -p maekon-network

# Single test
cargo test -p maekon-storage -- sqlite::tests::migration_v7

# Integration tests
cargo test -p maekon-app
```

### E2E Tests (Web Dashboard)

```bash
cd crates/maekon-web/frontend
pnpm test:e2e          # Full E2E test suite
pnpm test:e2e:headed   # With browser visible
pnpm test:e2e:ui       # Playwright UI mode
```

## PR Process

### Branch Strategy

```bash
# New feature branch
git checkout -b feat/vision-pii-filter-improvement

# Bug fix branch
git checkout -b fix/network-sse-reconnect

# Documentation branch
git checkout -b docs/scheduler-architecture
```

### Pre-PR Checklist

Confirm all of the following before opening a PR.

```bash
# 1. Format check
cargo fmt --check

# 2. Clippy warnings: 0
cargo clippy --workspace

# 3. All tests pass
cargo test --workspace

# 4. Build succeeds
cargo build --workspace
```

### Writing the PR Description

Include the following in your PR description:

- Motivation and background for the change
- Summary of the implementation approach
- How to test the change
- Confirmation that architecture rules are followed (especially cross-crate dependencies)
- Contribution lane and risk class, especially if the change touches
  `trust-core` surfaces
- AI-assisted disclosure when applicable
- A privacy-safe evidence summary for UI, runtime, capture, automation, or
  release-impacting changes

### Code Review

Reviewers focus on:

- Hexagonal Architecture compliance (port/adapter separation)
- No direct dependencies between adapter crates
- `cargo clippy` warnings: 0
- Manual mocks only (no mockall)
- English comments
- Public/private data safety: no secrets, private screenshots, raw capture
  content, maintainer-only validation names, or internal-only paths
- Trust-core risk: consent, PII, capture, audio, automation, sandbox, egress,
  updater, and release-signing changes require stronger maintainer review

## Commit Message Convention

Follow [Conventional Commits](https://www.conventionalcommits.org/).

### Format

```
<type>(<scope>): <description>

[optional body]

[optional footer]
```

### Types

| Type | Description |
|------|------|
| `feat` | New feature |
| `fix` | Bug fix |
| `perf` | Performance improvement |
| `refactor` | Refactoring (no behavior change) |
| `test` | Adding or updating tests |
| `docs` | Documentation changes |
| `chore` | Build, CI, or dependency changes |

### Scopes

Use the crate name or feature area as the scope.

`core`, `network`, `suggestion`, `storage`, `monitor`, `vision`, `tauri`, `web`, `automation`, `app`

### Examples

```
feat(vision): add credit card number masking to PII filter

Masks 16-digit number patterns at Standard level and above.
Integrated with the existing CWE-359 compliance logic.
```

```
fix(network): cap SSE reconnect exponential backoff at 30 seconds

Prevents the retry delay from growing unbounded on repeated failures.
```

```
perf(storage): eliminate N+1 query in end_work_session with RETURNING

Merges the SELECT + UPDATE into a single RETURNING clause query.
Benchmark: 50% throughput improvement confirmed.
```

## Reporting Issues

### Bug Reports

Use the **Bug Report** template in GitHub Issues and include:

1. **Bug description**: A clear explanation of what went wrong
2. **Steps to reproduce**: Step-by-step reproduction procedure
3. **Expected behavior**: What should happen
4. **Actual behavior**: What actually happens
5. **Environment**: OS, Rust version (`rustc --version`), relevant dependency versions
6. **Logs**: Relevant output from `RUST_LOG=debug cargo run -p maekon-app`

### Feature Requests

When proposing a feature, explain it from the Hexagonal Architecture perspective:

- Whether a new port is needed or an existing port can be extended
- Which crate the adapter should live in
- Impact on existing cross-crate dependency relationships

### Bounded Collection Policy

All runtime-growing collections MUST have a bounded capacity. Unbounded
collections that grow proportionally to user activity will eventually
exhaust memory in a long-running desktop agent.

**Required patterns:**

| Collection type | Bounded pattern | When to use |
|-----------------|----------------|-------------|
| `LruCache` | Explicit `NonZeroUsize` capacity | Caches with key-based eviction |
| `VecDeque` | `with_capacity(N)` + pop on overflow | FIFO buffers, ring buffers |
| `BTreeSet` | Check `len() >= max_size` before insert | Priority queues with eviction |
| `Map` (frontend) | Check `.size > MAX` and delete oldest | Session caches in React refs |

**Never** use unbounded `Vec`, `HashMap`, or `Map` that grows with user
activity without a capacity check.

**Examples from the codebase:**

| Location | Type | Capacity | Eviction |
|----------|------|----------|----------|
| `LlmWorkTypeRefiner` (maekon-analysis) | `LruCache` | 64 entries | LRU + 5min TTL |
| `SuggestionQueue` (maekon-suggestion) | `BTreeSet` | `max_size` param (default 50) | Lowest-priority evicted |
| `CaptureRingBuffer` (maekon-vision) | `VecDeque` | 6 slots | Circular overwrite |
| `AuditLogger` (maekon-automation) | `VecDeque` | `max_buffer_size` param | `pop_front` on overflow |
| `InputActivityCollector` (maekon-monitor) | `VecDeque` | 16 shortcuts | Capacity-bounded |
| `SegmentBuffer` (maekon-analysis) | `VecDeque` | Constructor `capacity` param | `pop_front` on overflow |
| `ThumbnailCache` (maekon-vision) | `LruCache` | 100 entries | LRU eviction |
| `SuggestionManager` (src-tauri) | `LruCache` | 512 read IDs | LRU eviction |
| `messagesCache` (frontend Chat) | `Map` | 20 sessions | Delete oldest key |
| `metricsHistory` (frontend useSSE) | Array | 60 entries | `slice(1)` shift |
| `BatchUploader` (maekon-network) | `SegQueue` | 10,000 items | Reject on overflow |

**Code review checklist item:** Any new `Vec::new()`, `HashMap::new()`,
`BTreeMap::new()`, or `new Map()` in a path that executes repeatedly (loop,
event handler, periodic task) must have a documented capacity bound or an
explanation of why it is safe (e.g., bounded by a fixed enum variant count).

### Dependency Update Policy

**Rust (Cargo):**
- **Security patches**: Apply immediately via `cargo update`
- **Minor versions**: Review monthly, batch in a single PR
- **Major versions**: Evaluate individually — check breaking changes, migration guide
- **Audit**: Run `cargo audit` before each release

**Frontend (pnpm):**
- **Security patches**: `pnpm audit fix` immediately
- **Minor versions**: Review monthly
- **Major versions**: Evaluate individually (especially React, Vite, Tailwind)
- **Lockfile**: Always commit `pnpm-lock.yaml`

**Toolchain:**
- **Rust**: Track latest stable, minimum version in `Cargo.toml` (`rust-version`)
- **Node.js**: LTS only
- **Dependabot**: Configured for Cargo dependencies (auto-PRs)

## License

By contributing to this project, you agree that your contributions are licensed under the [Apache License 2.0](LICENSE).

---

For questions, use GitHub Issues or Discussions.
