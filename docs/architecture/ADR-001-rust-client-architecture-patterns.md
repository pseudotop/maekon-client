[English](./ADR-001-rust-client-architecture-patterns.md) | [한국어](./ADR-001-rust-client-architecture-patterns.ko.md)

# ADR-001: Rust Client Architecture Patterns

**Status**: Accepted
**Date**: 2026-01-28
**Scope**: Entire client-rust/

---

## Context

The parent server strictly governs DDD + Hexagonal Architecture through ADRs. The Rust client also requires the same level of architectural consistency, but since the Rust compiler already enforces certain aspects (crate boundaries, mandatory trait implementations), **only design decisions that the compiler cannot catch are explicitly specified**.

## Decisions

### 1. Error Type Strategy

**Rule**: Library crates use `thiserror`, binary crate uses `anyhow`

```
oneshim-core / audio / monitor / vision / network
storage / suggestion / automation / analysis
embedding / web    → crate-local thiserror enums
oneshim-api-contracts → contract crate (DTO-focused, no shared top-level facade required)
oneshim-lint       → tooling binary (local CLI-style failure handling)
oneshim-app        → anyhow::Result            ← Used only at top level (`src-tauri`)
```

**Pattern** (illustrative; current `NetworkError` carries 13 variants — see `crates/oneshim-network/src/error.rs` for the canonical list):
```rust
// Library crate — specific errors
#[derive(Debug, thiserror::Error)]
pub enum NetworkError {
    #[error("HTTP error: {0}")]
    Http(String),                                  // See http_client::map_reqwest_error
                                                   // for timeout/rate-limit classification.
    #[error("request timeout after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },
    #[error(transparent)]
    Core(#[from] oneshim_core::error::CoreError),
    // ... 10 more semantic variants
}

// Binary crate — unified with anyhow
fn main() -> anyhow::Result<()> { ... }
```

**Rationale**: `thiserror` allows callers to pattern match on errors, making it suitable for libraries. `anyhow` is good for expressing "something failed" and is suitable for the final binary. Wire error codes are handled by [ADR-019](./ADR-019-error-code-infrastructure.md) — each `CoreError` struct-variant carries a typed `code: XxxCode` field that adapter errors map into via `impl From<AdapterError> for CoreError`.

### 2. Async Trait Pattern (Port Interfaces)

**Rule**: Use `async_trait` macro (ensures object safety)

```rust
use async_trait::async_trait;

#[async_trait]
pub trait ApiClient: Send + Sync {
    async fn post(&self, path: &str, body: &[u8]) -> Result<Vec<u8>, CoreError>;
}
```

**Rationale**: While `async fn in trait` was stabilized in Rust 1.75, object safety is not guaranteed when used as `dyn Trait`. `async_trait` is consistently applied as it is essential for the DI pattern (`Arc<dyn T>`).

**Scope**: All traits in `oneshim-core/src/ports/` have `#[async_trait]` applied.

### 3. Dependency Injection (DI) Pattern

**Rule**: Constructor injection + `Arc<dyn PortTrait>`

```rust
pub struct SuggestionReceiver {
    api_client: Arc<dyn ApiClient>,
    notifier: Arc<dyn DesktopNotifier>,
    storage: Arc<dyn StorageService>,
}

impl SuggestionReceiver {
    pub fn new(
        api_client: Arc<dyn ApiClient>,
        notifier: Arc<dyn DesktopNotifier>,
        storage: Arc<dyn StorageService>,
    ) -> Self {
        Self { api_client, notifier, storage }
    }
}
```

**Wiring location**: Manual wiring in the `oneshim-app` composition root (`src-tauri/src/main.rs`, `src-tauri/src/setup.rs`, and dedicated app-layer builders/coordinators). No DI framework used.

**Rationale**: The Rust ecosystem doesn't need a DI framework like Spring/Guice. Constructor injection is validated at compile time and makes mock injection easy during testing. ADR-009 further constrains how the composition root is kept thin over time.

### 4. Module Visibility Rules

| Visibility | Usage | Example |
|------------|-------|---------|
| `pub` | Types/traits exposed outside the crate | All models, port traits, error types |
| `pub(crate)` | Helpers used only within the crate | Utility functions, internal constants |
| private | Internal module implementation | Parsers, conversion logic |

**Rules**:
- `oneshim-core`'s `models/`, `ports/`, `error.rs`, `config.rs` are all `pub`
- Adapter crate implementations are `pub struct` but internal fields are private
- When using `pub(crate)`, always include a comment explaining why

### 5. Testing + Mock Strategy

**Rule**: Trait-based manual mocks (mockall not used)

```rust
// Test mock — defined in each crate's tests/ or #[cfg(test)] module
#[cfg(test)]
pub(crate) struct MockStorageService {
    pub events: std::sync::Mutex<Vec<Event>>,
}

#[cfg(test)]
#[async_trait]
impl StorageService for MockStorageService {
    async fn save_event(&self, event: &Event) -> Result<(), CoreError> {
        self.events.lock().unwrap().push(event.clone());
        Ok(())
    }
}
```

**Rationale**: `mockall` has significant proc macro overhead, and simple trait mocks are clearer when implemented manually. With fewer than 10 traits, manual management is feasible.

**Test scope**:
- `oneshim-core`: Model serde serialization/deserialization
- Adapter crates: Logic testing with port trait mocks injected
- `oneshim-app`: Integration tests (`tests/` directory)

### 6. Crate Dependency Direction (Immutable)

```
oneshim-core           ← audio / monitor / vision / storage / suggestion
                      ← automation / analysis / embedding
oneshim-api-contracts ← web / network
src-tauri (oneshim-app) ← all runtime crates (composition root only)
oneshim-lint          ← standalone tooling package
```

**Forbidden**: Direct runtime dependencies between adapter crates outside the approved baseline above (for example `oneshim-monitor -> oneshim-storage`). Cross-crate behavior should flow through `oneshim-core` ports, or through `oneshim-api-contracts` when the dependency is strictly transport-DTO sharing.

**Runtime baseline**:
- `oneshim-network` may depend on `oneshim-api-contracts`
- `oneshim-web` may depend on `oneshim-api-contracts`
- `oneshim-audio` may depend only on `oneshim-core`
- `oneshim-app` (package in `src-tauri/`) is the only package allowed to aggregate multiple adapters directly
- `oneshim-lint` is tooling-only and is outside the runtime graph

**Guardrail**: CI verifies normal workspace dependencies with `scripts/check-architecture-deps.sh`. Dev/build-only dependencies are intentionally excluded from that runtime check.

### 7. Port Location Rules

**Rule**: All port traits (interfaces) that are consumed by more than one crate MUST be defined in `oneshim-core/src/ports/`.

Port traits defined inside adapter crates are only allowed when:
- The trait is used exclusively within that single adapter crate
- The trait represents an internal abstraction, not a cross-crate contract

**Current status**:
- `WebStorage` is canonical in `oneshim-core/src/ports/web_storage.rs`.
- `oneshim-web/src/storage_port.rs` remains only as a crate-local re-export shim and is not a canonical port definition.

**Concrete type leaks**: As the default rule, adapter crate state structs that act as cross-crate
boundaries MUST reference port traits via `Arc<dyn PortTrait>`, never concrete adapter types from
other crates. Tauri-managed entry-point state in `oneshim-app` has a narrower framework-specific
rule; see [ADR-014](./ADR-014-tauri-managed-state-boundary.md).

```rust
// ❌ Wrong — leaks concrete type from another adapter
pub struct AppState {
    automation: Arc<AutomationController>,  // concrete from oneshim-automation
}

// ✅ Correct — references port trait from oneshim-core
pub struct AppState {
    automation: Arc<dyn AutomationPort>,  // trait from oneshim-core/ports/
}
```

**Rationale**: Hexagonal Architecture requires all contracts to live in the domain core. Adapter-to-adapter dependencies through concrete types create hidden coupling that bypasses the port layer.

### 8. Port Contract Testing

**Rule**: Each port trait in `oneshim-core/src/ports/` SHOULD have a contract test macro that any adapter implementation can invoke.

**Pattern**:
```rust
// In oneshim-core/src/ports/storage.rs or a dedicated test-utils module
#[cfg(test)]
#[macro_export]
macro_rules! test_storage_service_contract {
    ($create_impl:expr) => {
        #[tokio::test]
        async fn contract_save_and_retrieve() {
            let storage = $create_impl;
            let event = Event::test_fixture();
            storage.save_event(&event).await.unwrap();
            let retrieved = storage.get_events(None, None, 10).await.unwrap();
            assert_eq!(retrieved.len(), 1);
        }

        #[tokio::test]
        async fn contract_pending_mark_sent() {
            let storage = $create_impl;
            // ... verify pending → mark_as_sent → no longer pending
        }

        #[tokio::test]
        async fn contract_empty_state() {
            let storage = $create_impl;
            let events = storage.get_pending_events(10).await.unwrap();
            assert!(events.is_empty());
        }
    };
}

// In oneshim-storage tests:
test_storage_service_contract!(SqliteStorage::open_in_memory(30));
```

**Rationale**: When adding a new adapter for an existing port (e.g., a new storage backend), contract tests ensure the new implementation satisfies the same behavioral guarantees as the existing one. Manual mocks (§5) verify callers; contract tests verify implementors.

**Scope**: Start with `StorageService` (most adapters). Extend to `ApiClient`, `SystemMonitor` as needed.

---

## Correspondence with Server ADRs

| Server ADR | Rust Client Correspondence | Notes |
|------------|---------------------------|-------|
| ADR-004 Hexagonal Architecture | Crate boundary = Layer boundary | Enforced by compiler |
| ADR-010 Application Layer Structure | `oneshim-app` = orchestration | Manual wiring |
| ADR-034 Selective DI | `Arc<dyn T>` constructor injection | This ADR §3 |
| ADR-037 Event Sourcing + Hexagonal | Not applicable (client doesn't use event sourcing) | — |
| Port Patterns | `oneshim-core/src/ports/` | This ADR §2 |

---

## Consequences

- All code follows these patterns from Phase 1
- Traits/models implemented in `oneshim-core` serve as contracts
- This ADR must be referenced when adding new crates
- ADR-009 defines the accepted client baseline for delivery-layer, composition-root, integration-plane, and AI/provider structure on top of these core rules
