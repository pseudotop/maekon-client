# Maekon Contributing Guide

Thanks for your interest in Maekon. This document is the Rust-specific guide for contributing to the Cargo workspace.

Brand and compatibility note: Maekon is the user-facing display name. Repository
URLs, crate/package names, Cargo package names, the `oneshim` CLI command,
`ONESHIM_*` environment variables, and existing config/data paths remain
`oneshim` technical identifiers for compatibility.

## Development Environment Setup

### Prerequisites

- **Rust** 1.77.1 or later (keep up to date with `rustup update stable`)
- **cargo** — Rust build system and package manager (included with Rust)
- **pnpm** — required to build the frontend web dashboard (`oneshim-web/frontend`)

### Setup

```bash
# 1. Clone the repository
git clone https://github.com/pseudotop/maekon-client.git
cd maekon-client

# 2. Check dependencies and build
cargo check --workspace

# 3. Build the frontend (if including the web dashboard)
cd crates/oneshim-web/frontend
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
cargo build -p oneshim-vision --features ocr

# Enable gRPC client (tonic/prost)
cargo build -p oneshim-network --features grpc
```

## Building

### Development Build

```bash
# Quick workspace verification
cargo check --workspace

# Development build
cargo build -p oneshim-app

# Run in development mode
cargo run -p oneshim-app
```

### Build with Frontend

The web dashboard embeds the React build output into the Rust binary.

```bash
# Step 1: Build the frontend
cd crates/oneshim-web/frontend && pnpm install && pnpm build
# Or use the script
./scripts/build-frontend.sh

# Step 2: Build the Rust binary (automatically embeds dist/)
cargo build --release -p oneshim-app
```

### Full Workspace Build

```bash
# Release build for all crates
cargo build --release --workspace
```

### Build Specific Crates

```bash
cargo build -p oneshim-core
cargo build -p oneshim-network
cargo build -p oneshim-vision
```

## Code Style

### Formatting

All code follows `cargo fmt` default settings. Run it before submitting a PR.

```bash
# Apply formatting
cargo fmt --all

# Check formatting (same as CI)
cargo fmt --check
```

### Lint

`cargo clippy` must report 0 warnings. If you need to suppress a warning, add `#[allow(...)]` to the specific item and explain why in a comment.

```bash
# Run clippy on the full workspace
cargo clippy --workspace

# Run with all features enabled
cargo clippy --workspace --all-features
```

### Comments and Documentation

- **Code comments/docstrings should be written in English by default.**
- **Public documentation is English-primary with multilingual companion docs (ko, ja, zh-CN, es) for key guides.**
- Add `///` doc comments to all `pub` items.
- Use inline comments (`//`) to explain intent in complex logic.
- For documentation governance, follow [docs/DOCUMENTATION_POLICY.md](./docs/DOCUMENTATION_POLICY.md).
- Korean companion policy: [docs/DOCUMENTATION_POLICY.ko.md](./docs/DOCUMENTATION_POLICY.ko.md)
- Keep mutable quality metrics in GitHub Actions run history and follow the status rules in [docs/DOCUMENTATION_POLICY.md](./docs/DOCUMENTATION_POLICY.md).

```rust
/// Screen capture trigger — decides whether to capture based on event importance.
pub struct SmartCaptureTrigger {
    // Timestamp of last capture — used for throttling
    last_capture: Instant,
}
```

### Error Handling

- Library crates: use `thiserror` to define concrete error enums
- Binary crate (`oneshim-app`): use `anyhow::Result`
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

**`oneshim-core` defines all port traits and domain models.** Adapter crates implement those ports, and `src-tauri/` (package name `oneshim-app`) is the composition root.

```
oneshim-core  (port definitions, models)
    <- oneshim-monitor   (system monitoring adapter)
    <- oneshim-vision    (image processing adapter)
    <- oneshim-network   (HTTP/SSE/WebSocket adapter)
    <- oneshim-storage   (SQLite adapter)
    <- oneshim-suggestion <- oneshim-network
    <- src-tauri          <- oneshim-suggestion
    <- oneshim-automation
    <- src-tauri package oneshim-app (full DI wiring)
```

### Prohibited Patterns

Direct dependencies between adapter crates are not allowed. For example, `oneshim-monitor` must not directly depend on `oneshim-storage`. All cross-crate communication goes through traits defined in `oneshim-core`.

Permitted exceptions:
- `oneshim-suggestion` -> `oneshim-network` (SSE reception)
- `src-tauri` -> `oneshim-suggestion` (suggestion display)

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

Add a new trait under `crates/oneshim-core/src/ports/`.

```rust
// crates/oneshim-core/src/ports/my_service.rs

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
// crates/oneshim-xxx/src/my_impl.rs

use async_trait::async_trait;
use oneshim_core::{ports::MyService, error::CoreError};

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
    use oneshim_core::ports::ApiClient;

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
                    code: oneshim_core::error_codes::NetworkCode::Generic,
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
cargo test -p oneshim-core
cargo test -p oneshim-vision
cargo test -p oneshim-network

# Single test
cargo test -p oneshim-storage -- sqlite::tests::migration_v7

# Integration tests
cargo test -p oneshim-app
```

### E2E Tests (Web Dashboard)

```bash
cd crates/oneshim-web/frontend
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

### Code Review

Reviewers focus on:

- Hexagonal Architecture compliance (port/adapter separation)
- No direct dependencies between adapter crates
- `cargo clippy` warnings: 0
- Manual mocks only (no mockall)
- English comments

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
6. **Logs**: Relevant output from `RUST_LOG=debug cargo run -p oneshim-app`

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
| `LlmWorkTypeRefiner` (oneshim-analysis) | `LruCache` | 64 entries | LRU + 5min TTL |
| `SuggestionQueue` (oneshim-suggestion) | `BTreeSet` | `max_size` param (default 50) | Lowest-priority evicted |
| `CaptureRingBuffer` (oneshim-vision) | `VecDeque` | 6 slots | Circular overwrite |
| `AuditLogger` (oneshim-automation) | `VecDeque` | `max_buffer_size` param | `pop_front` on overflow |
| `InputActivityCollector` (oneshim-monitor) | `VecDeque` | 16 shortcuts | Capacity-bounded |
| `SegmentBuffer` (oneshim-analysis) | `VecDeque` | Constructor `capacity` param | `pop_front` on overflow |
| `ThumbnailCache` (oneshim-vision) | `LruCache` | 100 entries | LRU eviction |
| `SuggestionManager` (src-tauri) | `LruCache` | 512 read IDs | LRU eviction |
| `messagesCache` (frontend Chat) | `Map` | 20 sessions | Delete oldest key |
| `metricsHistory` (frontend useSSE) | Array | 60 entries | `slice(1)` shift |
| `BatchUploader` (oneshim-network) | `SegQueue` | 10,000 items | Reject on overflow |

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
