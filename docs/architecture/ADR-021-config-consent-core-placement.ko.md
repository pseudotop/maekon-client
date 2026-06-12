[English](./ADR-021-config-consent-core-placement.md) | [한국어](./ADR-021-config-consent-core-placement.ko.md)

# ADR-021: Config 및 Consent Core 배치

**Status**: Accepted
**Date**: 2026-05-28
**Scope**: `crates/maekon-core/src/config_manager`, `crates/maekon-core/src/consent.rs`, runtime wiring
**Related**: ADR-001, ADR-014, ADR-016, ADR-019
**Implementation**: `crates/maekon-core/src/config_manager/`, `crates/maekon-core/src/consent.rs`, `src-tauri/src/app_runtime_launch/capture_wiring.rs`

---

## Context

Privacy gate 및 runtime hardening 통합 작업 중 파일 기반 `ConfigManager` 와 `ConsentManager` 를 `maekon-core` 에 유지할지, 아니면 adapter port 뒤로 이동할지 결정이 필요했다.

Hexagonal 원칙상 domain contract 는 infrastructure 에 의존하면 안 된다. 동시에 Maekon client 에는 여러 adapter/runtime loop 가 공유하는 두 가지 local authority 가 있다.

- `vision.capture_enabled`, active hours, tracking schedule 같은 설정 gate
- screen capture, full-text extraction 같은 privacy consent gate

이 둘을 adapter crate 로 이동하면 core-facing runtime code 가 concrete adapter 에 의존하거나 동일한 policy state 를 별도 DTO/trait 로 중복해야 한다.

## Decision

### 1. ConfigManager 와 ConsentManager 는 maekon-core 에 유지한다

`ConfigManager` 와 `ConsentManager` 는 workspace 전역에서 사용하는 product policy state, validation, migration, default, consent semantics 를 정의하므로 `maekon-core` 에 남긴다.

이는 승인된 boundary 예외다. 이들의 file-backed persistence 는 remote/platform infrastructure 가 아니라 local product state persistence 로 취급한다.

### 2. 두 manager 에 외부 side effect 를 넣지 않는다

두 manager 는 자신이 소유한 local JSON state file 을 읽고 쓸 수 있다. 하지만 provider 호출, network egress, native automation, screen capture, notification delivery, OS permission mutation 을 수행하면 안 된다.

그런 효과는 provider catalog, frame storage, notification, capture, automation port 또는 runtime adapter 뒤에 둔다.

### 3. runtime composition 이 manager 를 아래로 전달한다

`src-tauri` 가 composition root 이다. 여기서 manager 를 만들고 web service, scheduler loop, capture wiring, provider guard 로 clone 을 전달할 수 있다. 소비자는 독립적인 file-backed manager 를 새로 만들기보다 기존 snapshot 또는 change bus 를 사용해야 한다.

### 4. persistence backend 가 실제로 교체 가능해질 때 port 를 추가한다

configuration 또는 consent state 에 encrypted cloud sync, OS keychain-backed consent record 같은 두 번째 production backend 가 생기면 그 시점에 `maekon-core` port 를 추가한다. 현재는 adapter port 가 실제 coupling 을 줄이지 못하고 불필요한 indirection 만 만든다.

## Consequences

### Positive

- capture, consent, schedule, privacy gate 의 single source of truth 가 유지된다.
- runtime loop 가 capture, AX extraction, GUI analysis, provider call 전에 fail-closed gate 를 적용할 수 있다.
- 기존 web, Tauri, scheduler consumer 의 public API 안정성이 유지된다.

### Negative

- `maekon-core` 에 소량의 local file I/O 가 계속 남는다.
- 두 manager 가 외부 side effect 를 갖지 않는지 테스트로 계속 확인해야 한다.

### Neutral

- `ConfigManager` 는 core local-state service 로 남고, network/provider discovery 는 계속 core/network port 뒤로 이동한다.

## Alternatives Considered

**A. 두 manager 를 storage adapter crate 로 이동.** core-facing runtime code 가 adapter dependency 를 갖거나 policy state trait/DTO 를 중복해야 하므로 거절했다.

**B. 현재 JSON file 에도 즉시 port 도입.** production backend 가 하나뿐이라 실제 privacy/testability coupling 을 줄이지 못하므로 거절했다.

**C. file I/O 만 storage 로 분리하고 DTO 는 core 유지.** startup/migration semantics 가 더 복잡해지면서 persistence coupling 은 거의 그대로 남으므로 현재는 거절했다.

## Known Follow-ups

1. `ConfigManager` 와 `ConsentManager` 테스트는 local persistence, migration, snapshot, consent semantics 에 집중한다.
2. 두 번째 backend 가 생기면 `src-tauri` 에 연결하기 전에 core port 를 먼저 추가한다.
3. external egress, native GUI mutation, OS permission 변경은 두 manager 밖에 유지한다.

## Related Docs

- `docs/architecture/ADR-001-rust-client-architecture-patterns.md`
- `docs/architecture/ADR-014-tauri-managed-state-boundary.md`
- `docs/architecture/ADR-016-config-change-bus.md`
- `docs/architecture/ADR-019-error-code-infrastructure.md`
