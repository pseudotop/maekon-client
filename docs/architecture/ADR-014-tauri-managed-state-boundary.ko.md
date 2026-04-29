[English](./ADR-014-tauri-managed-state-boundary.md) | [한국어](./ADR-014-tauri-managed-state-boundary.ko.md)

# ADR-014: Tauri Managed State 경계

**상태**: Accepted
**날짜**: 2026-04-02
**범위**: `src-tauri/`, Tauri managed state, IPC command 경계, composition-root wiring

---

## 컨텍스트

ADR-001은 cross-crate 동작이 concrete adapter 타입이 아니라 `oneshim-core` port를 통해
흘러야 한다는 기본 원칙을 올바르게 정한다.

하지만 `oneshim-app`은 데스크톱 엔트리포인트이자 Tauri 프레임워크 경계도 함께 소유한다.
여기에는 ADR-001만으로는 충분히 정리되지 않는 세 가지 압력이 있다.

1. Tauri managed state는 exact type으로 조회된다. 이것은 domain port 경계가 아니라
   framework-level storage 메커니즘이다.
2. 일부 desktop command 경로는 lifecycle 제어, retry, token 집계, persistence
   coordination처럼 `src-tauri` 안에서는 의미가 있지만 workspace 전반의 재사용 business
   contract는 아닌 운영 기능을 필요로 한다.
3. raw implementation 타입을 `AppState`에 그대로 두면 command 경계가 내부 서비스 shape에
   묶인다. 반대로 Tauri 때문에 필요한 helper 메서드를 모두 `oneshim-core`로 밀어 넣으면
   core가 특정 delivery/runtime framework에 과적합된다.

그 결과 두 방향의 drift가 생긴다.

- raw implementation object가 framework-managed state로 누수되거나
- Tauri를 만족시키기 위해 `oneshim-core` port가 delivery/runtime 전용 메서드로 커진다

이 ADR은 그 중간 지점의 베스트 프랙티스를 정의한다.

---

## 결정 사항

### 1. 구체 조립은 엔트리포인트에서 유지한다

`oneshim-app`은 데스크톱 바이너리의 단일 composition root로 남는다.

Concrete adapter 생성은 아래 위치에서 허용된다.

- `src-tauri/src/main.rs`
- `src-tauri/src/setup*.rs`
- `app_runtime_launch.rs` 같은 app-layer builder / launch coordinator

Concrete composition 자체는 위반이 아니다. 그것이 composition root의 역할이다.

### 2. Managed state는 port 또는 binary-local boundary type을 사용한다

Tauri에 등록되어 command, event handler, background callback에서 소비되는 state 타입은
반드시 다음 셋 중 하나여야 한다.

1. 실제 cross-crate contract를 표현하는 경우 `oneshim-core`의 `Arc<dyn PortTrait>`
2. desktop/framework/orchestration 전용 capability인 경우 `src-tauri`에 정의한
   purpose-built binary-local facade / handle 타입
3. `AppHandle`, window handle, background runtime coordinator, channel sender/receiver 같은
   framework-native runtime handle

허용되는 boundary 타입 예시:

```rust
pub struct AiSessionRuntimeHandle {
    pub session_manager: Arc<dyn SessionManager>,
    pub session_storage: Arc<dyn SessionStoragePort>,
    pub token_budget: Arc<TokenBudgetTracker>,
}

pub struct AutomationCommandHandle {
    pub tx: tokio::sync::mpsc::Sender<AutomationCommand>,
}
```

### 3. Raw implementation object를 command boundary의 기본값으로 두지 않는다

새 managed-state 필드는 raw implementation 타입을 command boundary로 직접 노출해서는 안 된다.

이 금지는 구현 타입이:

- 다른 workspace crate에 있든
- `src-tauri` 내부에 있든

동일하게 적용된다.

금지되는 새 boundary shape 예시:

```rust
pub struct AppState {
    pub session_manager: Arc<SessionManagerImpl>;
    pub storage: Arc<SqliteStorage>;
}
```

문제는 composition 시점에 concrete라는 사실이 아니다. 문제는 command와 framework callback이
raw implementation detail에 직접 결합된다는 점이다.

### 4. 책임에 따라 boundary 형태를 고른다

새 capability를 Tauri managed state에 추가할 때는 다음 순서로 결정한다.

1. stable한 business/application contract이고 둘 이상의 crate가 구현하거나 소비할 수 있다면
   `oneshim-core` port를 사용한다.
2. desktop delivery, framework lifecycle, command orchestration 전용이면 `src-tauri`
   facade / handle을 사용한다.
3. async resource가 serialized access, explicit backpressure, exclusive ownership을 요구하면
   actor-style handle + message passing을 사용한다.

즉,

- domain/application contract는 `oneshim-core`
- desktop command orchestration은 `src-tauri`
- serialized async resource ownership은 actor handle

로 정리한다.

### 5. 거대한 `AppState`보다 좁은 managed state를 선호한다

새 desktop 기능은 clean isolation이 가능하다면 하나의 거대한 `AppState`를 계속 키우기보다
좁은 managed state 타입을 선호해야 한다.

권장 패턴:

```rust
app.manage(AiSessionRuntimeHandle::new(...));
app.manage(AudioRuntimeHandle::new(...));
```

이렇게 하면 Tauri의 exact-type state retrieval이 명확해지고 서로 무관한 capability가 하나의
global struct에 계속 쌓이는 것을 막을 수 있다.

### 6. Framework를 만족시키기 위해 `oneshim-core`를 오염시키지 않는다

Tauri command가 convenience API를 원한다는 이유만으로 `oneshim-core` port에 메서드를
추가해서는 안 된다.

다음 같은 연산은 core port로 억지 승격하기보다 binary-local facade에 두는 편이 낫다.

- framework shutdown coordination
- desktop-only token display aggregation
- UI retry/recovery helper
- command-specific event emission coordination

이 연산들이 나중에 실제 multi-crate contract가 되면 그때 의도적으로 `oneshim-core`로 올린다.
처음부터 거기에 둘 필요는 없다.

### 7. 기존 raw field는 legacy로 보고 기회가 될 때 점진적으로 이관한다

현재 `src-tauri` state 일부에는 raw implementation object가 남아 있다. 이것은 target pattern이
아니라 transitional debt로 본다.

기존 legacy field에 대한 규칙:

1. 현재 delivery work를 위해 일시적으로 남아 있을 수 있다.
2. 새 기능은 이 패턴을 복제하면 안 된다.
3. legacy field를 meaningful feature work나 refactor로 건드릴 때는 facade / handle 또는
   port-backed boundary로 교체하는 것을 우선한다.

이 ADR은 repo 전체를 한 번에 뒤엎지 않으면서도 앞으로의 기본 규칙을 명확히 한다.

---

## 고려한 대안

### A. 필요한 메서드를 전부 `oneshim-core` port에 넣는다

기본안으로는 기각한다.

이 방식은 command 코드를 모두 trait 기반으로 유지하지만, Tauri 전용 lifecycle과 convenience
연산까지 core로 밀어 넣게 된다. 그러면 `oneshim-core`의 의미가 약해진다.

### B. Raw implementation 타입을 `AppState`에 그대로 둔다

기본안으로는 기각한다.

가장 단순한 단기 구현이지만, command와 callback이 내부 서비스 shape에 묶여 교체, 테스트,
리뷰가 모두 어려워진다.

### C. Binary-local facade / handle을 둔다

기본안으로 채택한다.

이 방식은 Tauri가 필요로 하는 concrete framework-facing 타입은 유지하면서도 boundary를
명시적이고 crate-local하게 만든다. 또한 여러 port와 local helper를 결합해도 그것을
`oneshim-core`에 억지로 넣지 않아도 된다.

### D. 모든 것을 actor / message-passing handle로 만든다

보편 규칙으로는 기각한다.

Actor-style handle은 exclusive async ownership이나 bounded queue가 필요한 resource에 강하다.
하지만 단순 facade만으로 충분한 command boundary까지 모두 actor화하는 것은 과한 비용이다.

---

## 결과

### 장점

1. `oneshim-core`가 Tauri convenience API가 아니라 재사용 가능한 contract에 집중할 수 있다.
2. Tauri state는 framework-friendly하게 유지하면서 raw implementation leak를 표준화하지 않는다.
3. Command가 넓은 서비스 내부 대신 명명된 boundary handle에 의존하게 되어 리뷰가 쉬워진다.
4. Legacy mega-state에서 feature-scoped state로 이동할 수 있는 방향이 생긴다.

### 단점

1. 일부 기능은 `src-tauri`에 facade / handle 타입을 하나 더 만들어야 한다.
2. 어떤 capability가 core port, local facade, actor handle 중 어디에 속하는지 판단이 필요하다.
3. migration 동안 legacy state field와 선호 패턴이 함께 존재할 수 있다.

---

## 리뷰 체크리스트

Tauri managed state를 추가하거나 수정하는 PR은 다음을 확인한다.

1. 새 state entry가 core port, binary-local facade / handle, framework-native runtime handle 중
   하나인가?
2. Raw implementation 타입을 넣는다면 facade / handle로 만들 수 없는 이유가 문서화되어 있는가?
3. Framework-only convenience 메서드를 `oneshim-core`에 추가하지 않았는가?
4. Resource가 async / exclusive / backpressure-sensitive해서 actor handle이 더 적절한가?
5. `AppState`를 더 키우기보다 좁은 managed state로 분리할 수 없는가?

---

## 리서치 메모

이 결정은 저장소 상황에 맞춘 추론이며, 아래 1차 자료를 참고했다.

- Alistair Cockburn, *Hexagonal architecture the original 2005 article*:
  https://alistair.cockburn.us/hexagonal-architecture
- Mark Seemann, *Composition Root*:
  https://blog.ploeh.dk/2011/07/28/CompositionRoot/
- Tauri v2 공식 문서, *State Management*:
  https://v2.tauri.app/develop/state-management/
- Tokio 공식 튜토리얼, *Shared state*:
  https://tokio.rs/tokio/tutorial/shared-state
- Tokio 공식 튜토리얼, *Channels*:
  https://tokio.rs/tokio/tutorial/channels

---

## 관련 ADR

- [ADR-001: Rust Client Architecture Patterns](./ADR-001-rust-client-architecture-patterns.ko.md)
- [ADR-007: Async Runtime Safety Patterns](./ADR-007-async-runtime-safety-patterns.md)
- [ADR-009: Client Architecture Baseline](./ADR-009-client-architecture-baseline.md)
