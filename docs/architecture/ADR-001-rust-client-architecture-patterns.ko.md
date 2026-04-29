[English](./ADR-001-rust-client-architecture-patterns.md) | [한국어](./ADR-001-rust-client-architecture-patterns.ko.md)

# ADR-001: Rust Client 아키텍처 패턴

**상태**: 채택됨 (Accepted)
**날짜**: 2026-01-28
**범위**: client-rust/ 전체

---

## 컨텍스트

Parent server 는 DDD + Hexagonal Architecture를 ADR로 엄격히 통제한다.
Rust 클라이언트도 동일 수준의 아키텍처 일관성이 필요하지만, Rust 컴파일러가 이미 강제하는 부분(crate 경계, trait 구현 필수)이 있으므로 **컴파일러가 잡지 못하는 설계 결정만 명시적으로 규정**한다.

## 결정 사항

### 1. 에러 타입 전략

**규칙**: 라이브러리 crate는 `thiserror`, 바이너리 crate는 `anyhow`

```
oneshim-core / audio / monitor / vision / network
storage / suggestion / automation / analysis
embedding / web    → crate-local thiserror enum
oneshim-api-contracts → contract crate (DTO 중심, 공유 top-level facade 불필요)
oneshim-lint       → tooling binary (로컬 CLI 스타일 실패 처리)
oneshim-app        → anyhow::Result            ← 최상위(`src-tauri`)에서만 사용
```

**패턴** (설명용; 현재 `NetworkError`는 13개 variant — 실제 목록은 `crates/oneshim-network/src/error.rs` 참조):
```rust
// 라이브러리 crate — 구체적 에러
#[derive(Debug, thiserror::Error)]
pub enum NetworkError {
    #[error("HTTP 에러: {0}")]
    Http(String),                                  // timeout/rate-limit 분류는
                                                   // http_client::map_reqwest_error 담당.
    #[error("요청 타임아웃 {timeout_ms}ms 초과")]
    Timeout { timeout_ms: u64 },
    #[error(transparent)]
    Core(#[from] oneshim_core::error::CoreError),
    // ... 10개 추가 semantic variant
}

// 바이너리 crate — anyhow로 통합
fn main() -> anyhow::Result<()> { ... }
```

**근거**: `thiserror`는 호출자가 에러를 패턴 매칭할 수 있어 라이브러리에 적합. `anyhow`는 "그냥 실패했다"를 표현하기 좋아 최종 바이너리에 적합. Wire error code 는 [ADR-019](./ADR-019-error-code-infrastructure.ko.md) 에서 다룸 — 각 `CoreError` struct-variant 가 typed `code: XxxCode` 필드를 carries 하고 adapter error 들은 `impl From<AdapterError> for CoreError` 로 매핑.

### 2. 비동기 Trait 패턴 (Port 인터페이스)

**규칙**: `async_trait` 매크로 사용 (object safety 보장)

```rust
use async_trait::async_trait;

#[async_trait]
pub trait ApiClient: Send + Sync {
    async fn post(&self, path: &str, body: &[u8]) -> Result<Vec<u8>, CoreError>;
}
```

**근거**: Rust 1.75에서 `async fn in trait`이 안정화되었지만, `dyn Trait`으로 사용 시 object safety가 보장되지 않는다. DI 패턴(`Arc<dyn T>`)에 필수적인 `async_trait`을 일관 적용한다.

**적용 범위**: `oneshim-core/src/ports/` 내 모든 trait에 `#[async_trait]` 적용.

### 3. 의존성 주입 (DI) 패턴

**규칙**: 생성자 주입 + `Arc<dyn PortTrait>`

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

**와이어링 위치**: `oneshim-app` composition root(`src-tauri/src/main.rs`, `src-tauri/src/setup.rs`, 그리고 app-layer builder/coordinator`)에서 수동 와이어링. DI 프레임워크는 사용하지 않는다.

**근거**: Rust 생태계에는 Spring/Guice 같은 DI 프레임워크가 필요 없다. 생성자 주입은 컴파일 타임에 검증되며, 테스트 시 mock 주입이 용이하다. composition root를 얇게 유지하는 현재 기준은 ADR-009가 추가로 규정한다.

### 4. 모듈 가시성 규칙

| 가시성 | 사용 위치 | 예시 |
|--------|----------|------|
| `pub` | crate 외부에 노출하는 타입/trait | 모든 모델, 포트 trait, 에러 타입 |
| `pub(crate)` | crate 내부에서만 사용하는 헬퍼 | 유틸리티 함수, 내부 상수 |
| private | 모듈 내부 구현 | 파서, 변환 로직 |

**규칙**:
- `oneshim-core`의 `models/`, `ports/`, `error.rs`, `config.rs`는 모두 `pub`
- 어댑터 crate의 구현체는 `pub struct`이지만 내부 필드는 private
- `pub(crate)` 사용 시 반드시 이유를 주석으로 명시

### 5. 테스트 + Mock 전략

**규칙**: trait 기반 수동 mock (mockall 미사용)

```rust
// 테스트용 mock — 각 crate의 tests/ 또는 #[cfg(test)] mod에 정의
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

**근거**: `mockall`은 proc macro 오버헤드가 크고, 단순 trait mock은 수동 구현이 더 명확하다. trait 수가 적으므로(<10개) 수동 관리 가능.

**테스트 범위**:
- `oneshim-core`: 모델 serde 직렬화/역직렬화
- 어댑터 crate: port trait mock 주입 후 로직 테스트
- `oneshim-app`: 통합 테스트 (`tests/` 디렉토리)

### 6. Crate 의존성 방향 (불변)

```
oneshim-core           ← audio / monitor / vision / storage / suggestion
                      ← automation / analysis / embedding
oneshim-api-contracts ← web / network
src-tauri (oneshim-app) ← 모든 runtime crate (composition root 전용)
oneshim-lint          ← standalone tooling package
```

**금지**: 위 승인된 baseline 밖의 runtime adapter 간 직접 의존 (예: `oneshim-monitor -> oneshim-storage`). cross-crate 동작은 `oneshim-core` port를 통해 흐르거나, transport DTO 공유에 한해 `oneshim-api-contracts`를 거쳐야 한다.

**현재 runtime baseline**:
- `oneshim-network`는 `oneshim-api-contracts`에 의존 가능
- `oneshim-web`은 `oneshim-api-contracts`에 의존 가능
- `oneshim-audio`는 `oneshim-core`에만 의존 가능
- `oneshim-app`(`src-tauri` 패키지)만 여러 adapter를 직접 집계할 수 있음
- `oneshim-lint`는 tooling 전용이므로 runtime graph 밖으로 본다

**가드레일**: CI는 `scripts/check-architecture-deps.sh`로 normal workspace dependency를 검증한다. dev/build dependency는 이 runtime 체크에서 의도적으로 제외한다.

### 7. Port 위치 규칙

**규칙**: 둘 이상의 crate가 소비하는 모든 port trait(인터페이스)는 반드시 `oneshim-core/src/ports/`에 정의한다.

어댑터 crate 내부에 trait를 두는 것은 아래 경우에만 허용된다.
- 그 trait가 해당 단일 어댑터 crate 내부에서만 사용될 때
- 그 trait가 cross-crate contract가 아니라 내부 추상화일 때

**현재 상태**:
- `WebStorage`의 canonical 정의는 `oneshim-core/src/ports/web_storage.rs`에 있다.
- `oneshim-web/src/storage_port.rs`는 crate 내부 편의를 위한 re-export shim일 뿐, port 정의 위치가 아니다.

**구체 타입 누수 금지**: 기본 원칙으로, cross-crate 경계 역할을 하는 adapter crate의
state struct(`AppState` 등)는 다른 crate의 concrete adapter 타입 대신
`Arc<dyn PortTrait>` 형태의 port trait만 참조해야 한다. `oneshim-app`의
Tauri-managed entry-point state에는 더 좁은 framework-specific 규칙이 있으며,
이는 [ADR-014](./ADR-014-tauri-managed-state-boundary.ko.md)에서 다룬다.

```rust
// ❌ 잘못된 예 — 다른 adapter의 concrete type 누수
pub struct AppState {
    automation: Arc<AutomationController>,  // concrete from oneshim-automation
}

// ✅ 올바른 예 — oneshim-core/ports/ 의 trait 참조
pub struct AppState {
    automation: Arc<dyn AutomationPort>,
}
```

**근거**: Hexagonal Architecture에서는 모든 계약(contract)이 domain core에 있어야 한다. concrete adapter를 통한 adapter-to-adapter 의존은 port layer를 우회하는 숨은 결합을 만든다.

### 8. Port Contract Testing

**규칙**: `oneshim-core/src/ports/`의 각 port trait는 가능하면 어떤 adapter 구현도 호출할 수 있는 contract test macro를 제공해야 한다.

**패턴**:
```rust
// oneshim-core/src/ports/storage.rs 또는 별도 test-utils module
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
    };
}

// oneshim-storage tests:
test_storage_service_contract!(SqliteStorage::open_in_memory(30));
```

**근거**: 기존 port에 새 adapter 구현(예: 다른 storage backend)을 추가할 때, contract test는 새 구현이 기존과 같은 동작 보장을 만족하는지 검증한다. 수동 mock(§5)은 호출자를 검증하고, contract test는 구현체를 검증한다.

---

## 서버 ADR과의 대응 관계

| 서버 ADR | Rust 클라이언트 대응 | 비고 |
|---------|-------------------|------|
| ADR-004 Hexagonal Architecture | Crate 경계 = Layer 경계 | 컴파일러가 강제 |
| ADR-010 Application Layer Structure | `oneshim-app` = orchestration | 수동 와이어링 |
| ADR-034 Selective DI | `Arc<dyn T>` 생성자 주입 | 본 ADR §3 |
| ADR-037 Event Sourcing + Hexagonal | 해당 없음 (클라이언트는 이벤트 소싱 미사용) | — |
| Port Patterns | `oneshim-core/src/ports/` | 본 ADR §2 |

---

## 결과

- Phase 1부터 모든 코드가 이 패턴을 따름
- `oneshim-core`에 구현된 trait/model이 계약(contract) 역할
- 새 crate 추가 시 이 ADR 참조 필수
- delivery layer, composition root, integration plane, AI/provider baseline은 ADR-009를 함께 기준으로 본다
