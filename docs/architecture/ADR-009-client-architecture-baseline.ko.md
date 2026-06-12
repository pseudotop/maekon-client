[English](./ADR-009-client-architecture-baseline.md) | [한국어](./ADR-009-client-architecture-baseline.ko.md)

# ADR-009: 클라이언트 아키텍처 베이스라인

**상태**: Accepted
**날짜**: 2026-03-17
**범위**: `client-rust/`, 특히 `maekon-app` 패키지(현재 `src-tauri/`에 위치 — 이전 `crates/maekon-app/` 디렉토리는 ADR-004 Tauri v2 마이그레이션으로 제거됨; 패키지명 `maekon-app`은 유지), `maekon-web`, 통합 런타임, AI 프로바이더 표면

---

## 컨텍스트

클라이언트 아키텍처는 다음에 걸쳐 여러 차례 구조적 정리를 거쳤다.

- 프로바이더 표면 모델링
- AI 런타임 wiring
- 통합 플레인 런타임 설계
- `maekon-app` composition-root 구조
- `maekon-web` delivery/service 레이어링

그 변경들은 더 이상 실험적인 정리가 아니다. 이제 향후 개발을 위한 의도된 베이스라인을 나타낸다. 명시적인 ADR이 없으면 이후 작업에서 다음 문제들이 점진적으로 재도입될 위험이 높다.

- 핸들러 비대화
- delivery 코드로의 `AppState` 누수
- 서비스 엔트리 drift
- `setup.rs`의 composition-root 성장
- spec 기반이 아닌 AI 프로바이더 동작
- 외부 통합 플레인이 로컬 컨트롤 플레인으로 다시 무너지는 현상

이 ADR은 현재 형태를 유지하고 개선해나갈 표준으로 동결한다.

---

## 결정 사항

### 1. 핵심 레이어링은 안정적으로 유지된다

다음 레이어 역할이 이제 고정된다.

1. `maekon-core`는 domain contract 레이어다.
2. 어댑터 크레이트는 `maekon-core`의 port를 구현한다.
3. `maekon-app`은 composition root이자 런타임 오케스트레이터다.
4. `maekon-web`은 delivery 레이어일 뿐이다.
5. 외부 통합은 로컬 데스크톱 컨트롤 플레인과 별개로 유지된다.
6. AI 프로바이더 동작은 surface 기반, contract 기반으로 유지된다.

이 ADR은 ADR-001이나 ADR-002를 대체하지 않는다. 현재 클라이언트 형태에 대한 수락된 베이스라인으로 두 ADR을 운용화한다.

### 2. `maekon-web`은 고정된 delivery 패턴을 사용한다

`maekon-web`은 다음 구조를 유지해야 한다.

1. 핸들러는 얇게 유지된다.
2. 좁은 delivery substate는 `WebContext` 구조체로 표현된다.
3. `WebContext` 정의는 [web_contexts/mod.rs](../../crates/maekon-web/src/services/web_contexts/mod.rs)에 위치한다.
4. 서비스는 delivery 경계에서의 공개 오케스트레이션 엔트리포인트다.
5. Assembler와 helper 모듈은 DTO 형성과 순수 변환 로직을 소유한다.
6. 핸들러와 서비스는 미들웨어 같은 명시적인 cross-cutting 예외를 제외하고 `AppState`를 직접 가져와서는 안 된다.

필요한 핸들러 흐름:

```rust
State(WebContext) -> QueryService/CommandService -> Assembler/Helper
```

금지된 drift:

1. 기능 서비스 파일 내부에 새 `WebContext` 구조체 정의
2. `context.queries()` 및 `context.commands()` 팩토리 헬퍼 재도입
3. 핸들러로 domain invariant 이동
4. web 전용 delivery 관심사가 `maekon-core`로 누수

권장 핸들러 경계:

```rust
XxxQueryService::new(context)
XxxCommandService::new(context)
```

### 3. `maekon-app`은 Builder/Coordinator 조합을 유지한다

`maekon-app`은 현재 app-layer 조합 스타일을 보존해야 한다.

필요한 형태:

1. `setup.rs`는 순수 조립 스크립트에 가깝게 유지된다.
2. 런타임 부트스트랩은 app-layer builder와 coordinator에 속한다.
3. 장기 실행 오케스트레이션은 bundle, runtime coordinator, launch builder에 속한다.

이는 이미 사용 중인 런타임 모듈들에 적용된다.

- `integration_runtime`
- `agent_runtime`
- `web_server_runtime`
- `background_runtime`
- `storage_runtime`
- `update_runtime`

금지된 drift:

1. `setup.rs`가 다시 기능 구현 파일로 성장
2. Tauri setup wiring에 직접 런타임 전용 오케스트레이션 임베딩
3. 명확한 아키텍처적 이유 없이 새 런타임 슬라이스에 대해 builder 우회

### 4. 통합 플레인은 별도로 유지된다

통합 아키텍처는 올바른 방향으로 수락되었으며 보존되어야 한다.

필요한 형태:

1. 로컬 `/api`는 1st-party 컨트롤 플레인으로 남는다.
2. 외부 통합은 자체 인증과 런타임 모델을 가진 별도 플레인으로 남는다.
3. 통합 런타임은 아웃바운드이고 클라이언트가 시작하는 방식으로 유지된다.
4. Privacy, policy, audit 게이트는 모든 외부 egress에 필수다.

필요한 모델링 분리:

1. `session/auth`
2. `egress/outbox`
3. `inbox`
4. `policy/audit`

이 관심사들은 하나의 일반 컨트롤러로 붕괴되어서는 안 된다.

### 5. AI 프로바이더 런타임은 spec 기반으로 유지된다

AI/프로바이더 아키텍처도 베이스라인으로 수락된다.

필요한 형태:

1. 프로바이더 동작은 프로바이더 표면 계약과 카탈로그 spec에서 구동된다.
2. `managed_oauth`, `direct_http`, `subprocess_cli`, self-hosted 표면은 명시적인 표면으로 모델링된 채 유지된다.
3. 설정, 런타임, UI는 동일한 표면 계약을 소비한다.
4. 새 프로바이더는 특수 케이스 로직을 도입하기 전에 spec 기반 경로를 확장해야 한다.

금지된 drift:

1. 표면 계약이 이미 존재하는 경우의 임시 벤더 분기
2. 프로바이더 표면 확인을 우회하는 delivery-layer AI 동작
3. 불일치하는 설정/런타임/프로바이더 해석 재도입

### 6. 명시적 예외는 허용되지만 좁다

다음 예외는 허용되며 위반으로 간주하지 않는다.

1. 미들웨어는 cross-cutting 인증과 경계 적용을 위해 `State<AppState>`를 직접 사용할 수 있다.
2. 순수 사양 헬퍼 모듈은 오케스트레이션 엔트리포인트가 아닐 때 함수 지향으로 유지될 수 있다.
3. 테스트 모듈은 fixture를 위해 `AppState`를 직접 생성할 수 있다.

이것들은 명시적 예외이며 더 넓은 패턴으로 일반화해서는 안 된다.

---

## 결과

### 장점

1. 클라이언트에는 이제 미래 작업을 위한 명확한 아키텍처 베이스라인이 있다.
2. 새 작업은 로컬 스타일 선호 대신 안정적인 규칙에 따라 판단할 수 있다.
3. 핸들러, 컨텍스트, 서비스, assembler가 더 명확한 역할을 가지므로 `maekon-web`을 리뷰하기가 훨씬 쉬워진다.
4. `maekon-app`이 과대화된 composition root로 회귀할 가능성이 줄어든다.
5. 통합 및 AI/프로바이더 작업이 이미 해결된 구조적 문제를 다시 열지 않고도 발전할 수 있다.

### 단점

1. 일부 기여자들은 경계 규칙이 필요 이상으로 엄격하다고 볼 수 있다.
2. 소규모 기능도 단축 구현에 비해 서비스나 헬퍼 타입이 하나 더 필요할 수 있다.
3. 미들웨어와 순수 헬퍼 모듈은 명시적 예외로 남아 리뷰에서 판단이 필요하다.

### 운영 영향

미래의 리팩터는 형식적인 분리가 아닌 실제 아키텍처 개선에 최적화해야 한다.

이 ADR은 다음을 의미하지 **않는다**:

1. 모든 헬퍼가 자체 타입이 되어야 한다
2. 모든 유틸리티가 서비스가 되어야 한다
3. 리팩터링이 무한히 계속되어야 한다

이 시점부터 기본값은 베이스라인 자체를 반복적으로 재설계하기보다 이 베이스라인 위에서 제품을 확장하는 것이다.

---

## 리뷰 체크리스트

클라이언트 아키텍처를 건드리는 실질적인 변경은 여전히 다음에 답해야 한다.

1. DDD와 Hexagonal 의존성 방향을 보존하는가?
2. `maekon-web`을 delivery 레이어로 유지하는가?
3. `WebContext -> service -> assembler/helper` 흐름을 보존하는가?
4. 통합과 자동화를 위한 privacy, policy, audit 게이트를 보존하는가?
5. spec 기반 AI/프로바이더 아키텍처를 보존하는가?
6. `setup.rs`를 다시 키우지 않고 runtime wiring을 builder/coordinator에 유지하는가?

---

## 관련 ADR

- [ADR-001: Rust Client Architecture Patterns](./ADR-001-rust-client-architecture-patterns.md)
- [ADR-002: OS GUI Interaction Boundary and Runtime Split](./ADR-002-os-gui-interaction-boundary.md)
- [ADR-003: Directory Module Pattern for Large Source Files](./ADR-003-directory-module-pattern.md)
- [ADR-007: Async Runtime Safety Patterns](./ADR-007-async-runtime-safety-patterns.md)
- [ADR-008: Network Resilience Patterns](./ADR-008-network-resilience-patterns.md)
- [ADR-014: Tauri Managed State Boundary](./ADR-014-tauri-managed-state-boundary.md)
