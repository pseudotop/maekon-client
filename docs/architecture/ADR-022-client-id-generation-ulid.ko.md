[English](./ADR-022-client-id-generation-ulid.md) | [한국어](./ADR-022-client-id-generation-ulid.ko.md)

# ADR-022: 클라이언트 ID 생성 — prefix+ULID 규칙

**Status**: Accepted
**Date**: 2026-05-28
**Scope**: `crates/maekon-core/src/id_generation.rs`, 엔티티 식별자를 생성하는 모든 클라이언트 crate
**Related**: server ADR-055 (prefix+ULID ID 생성), ADR-021 (Config 및 Consent Core 배치)
**Implementation**: `crates/maekon-core/src/id_generation.rs:14`, `crates/maekon-core/src/lib.rs` (re-export)

---

## 배경 (Context)

Maekon 클라이언트는 `maekon-core`, `maekon-network`, `maekon-automation`,
`maekon-storage`, `maekon-vision` 전반에 걸쳐 많은 호출 지점에서 문자열 식별자를
생성한다. 이 ADR 이전에는 `Uuid::new_v4().to_string()` 또는
`format!("{prefix}-{}", Uuid::new_v4())` 를 사용하여 RFC 4122 UUID v4 형식의 문자열을
생성했다.

서버는 server ADR-055 에서 `{prefix}_{ULID}` 형식의 식별자를 채택했다. 다음 경우에
크로스-경계 일관성이 중요하다:

- 클라이언트 생성 ID 가 sync payload, audit export, gRPC request metadata 에 포함되어
  서버 또는 운영자가 클라이언트 발생 레코드와 서버 발생 레코드를 연관시켜야 할 때.
- 정렬 가능한 ID 는 스토리지 인덱스 파편화를 줄이고 별도의 `created_at` 컬럼 없이
  시간 순서 조회를 가능하게 한다 (ULID 는 48비트 밀리초 타임스탬프를 인코딩한다).
- 의미 있는 prefix 는 로그 스캔과 디버깅을 빠르게 한다.

`consent.rs` 의 F-RC-C32-03 주석에서 이 결정을 명시적으로 유보했다:
> *"크로스-경계 trace ID 일관성이 우선순위가 된다면, Rust 쪽 `generate_id()` 유틸리티를
> 위한 ADR 을 작성할 것"* — `maekon-core/src/consent.rs` 에서 유보됨.

`ulid` crate (`version = "1"`) 는 이미 workspace 의존성으로 포함되어 있었다
(`maekon-web` 에서 사용 중). 새로운 crate 의존성은 필요하지 않다.

## 범위 및 면제 (Scope and Exemptions)

`generate_id` (prefix+ULID) 는 **엔티티 및 상관 식별자 전용**이다.

다음 범주는 **면제**이며 `Uuid::new_v4()` 또는 다른 CSPRNG 기반 프리미티브를
계속 사용해야 한다:

### 면제 범주 A — 암호화 nonce, 토큰, 시크릿

ULID 는 상위 비트에 예측 가능한 48비트 밀리초 타임스탬프를 포함하며, UUID v4 의 122비트
랜덤 필드보다 작은 80비트 랜덤 필드를 가진다. 완전한 예측 불가능성이 필요한 보안
민감 값에 ULID 는 부적절한 프리미티브다.

면제 지점 (반드시 `Uuid::new_v4()` 유지):

| 위치 | 역할 |
|---|---|
| `maekon-automation/src/gui_interaction/crypto.rs` — `new_capability_token()` 엔트로피 | 이 값에 SHA-256 을 적용하여 역량 토큰을 생성; 입력은 CSPRNG 수준이어야 함 |
| `maekon-automation/src/policy/token.rs` — `issue_policy_nonce()` | HMAC-SHA256 정책 토큰의 서명 nonce; 예측 불가능해야 함 |
| `maekon-automation/src/controller/mod.rs` — 커맨드 확인 `nonce` | 대기 확인 흐름의 변조 방지 nonce; 예측 불가능해야 함 |
| `maekon-automation/src/gui_interaction/service_execution.rs` — 역량 티켓 `nonce` | HMAC 서명 실행 티켓의 안티-리플레이 nonce; 예측 불가능해야 함 |

### 면제 범주 B — 미검증 형식 계약을 가진 서버-wire ID

클라이언트 생성 ID 가 서버로 전송되고 서버 측 형식 검증이 완전히 감사되지 않은
경우, 서버 계약이 ULID 형식 수용을 확인할 때까지 UUID v4 를 유지하는 것이 안전하다.

면제 지점:

| 위치 | 역할 |
|---|---|
| `maekon-storage/src/sqlite/device_identity.rs` — `device_id` | `IntegrationBootstrapRequest` 에서 서버로 전송; 서버 형식 계약 미검증 |

### 면제 범주 C — RFC 필수 및 외부 검증 ID

| 위치 | 이유 |
|---|---|
| `maekon-network/src/integration/auth/proof_factory.rs` — `jti` JWT 클레임 | RFC 7519 §4.1.7 은 UUID 요구; 서버 측 JWT 검증이 비-UUID `jti` 거부 |
| `maekon-network/src/integration/inbox_coordinator.rs` — `IntegrationEnvelope.nonce` | 서버-wire 안티-리플레이 필드; 서버 측 형식 검증 |
| `maekon-network/src/integration/http_transport/connect.rs` — `IntegrationBootstrapRequest.nonce` | 서버-wire 프로토콜 필드; 서버가 형식 검증 가능 |

### 면제 범주 D — 비문자열 Uuid 타입 필드

| 위치 | 이유 |
|---|---|
| `maekon-storage/src/sqlite/test_utils.rs` — `event_id: Uuid` | 필드 타입이 `uuid::Uuid` (String 아님) |

## 결정 (Decision)

### 1. maekon-core 의 `generate_id(prefix: &str) -> String`

클라이언트 생성 **엔티티 및 상관** 문자열 식별자는 `{prefix}_{ULID}` 형식을 채택하며,
`maekon-core::id_generation` 의 `generate_id` 로 구현된다:

```rust
// crates/maekon-core/src/id_generation.rs
pub fn generate_id(prefix: &str) -> String {
    validate_prefix(prefix);
    format!("{prefix}_{}", ulid::Ulid::new())
}
```

이 함수는:
- prefix 를 검증한다 (소문자 ASCII 영문/숫자/`_`, 영문으로 시작, 최대 63바이트)
- 잘못된 prefix 에서 패닉 (개발자 오류, 개발 시점에 발견됨)
- `maekon_core::generate_id` 로 re-export
- 기존 `ulid = { workspace = true }` 의존성 재사용

암호화 nonce, 토큰, 미검증 계약 서버-wire ID, RFC 필수 ID, 비문자열 타입 필드는
위 면제 조항에 따라 `Uuid::new_v4()` 를 유지한다.

### 2. Prefix 레지스트리 (이 ADR 이 권위적 기록)

| Prefix | 맥락 |
|---|---|
| `req` | gRPC / HTTP 요청 상관 ID (`x-request-id`) |
| `ses` | AI 세션, GUI 인터랙션 세션 |
| `flow` | OAuth / OIDC 디바이스 인증 흐름 |
| `sug` | AI 제안 |
| `ann` | 어노테이션 |
| `pomo` | 포모도로 세션 |
| `ovr` | 재교정 재정의 |
| `aud` | 감사 로그 항목 |
| `evt` | 감사 이벤트 (log_event 경로) |
| `consent` | 동의 레코드 |
| `tkt` | GUI 실행 티켓 (엔티티 ID; 내부 보안 nonce 아님) |
| `hl` | 오버레이 하이라이트 핸들 |
| `env` | 통합 봉투 (로컬 식별자) |
| `rcpt` | 통합 프롬프트 수신 |
| `q` | 통합 상태 저장소 큐 항목 |
| `scene` | UI 씬 |
| `rect` | UI 씬 직사각형 요소 |
| `ptr` | 포인터 동작 trace |
| `ctx` / `input` / `proc` / `win` / `clip` / `fa` / `tl` | 타임라인/이벤트 어셈블러 컨텍스트 유형 |
| `cch` | 코칭 엔진 메시지 |
| `msg` | 일반 메시지 |
| `clm` | 메모리 claim 노드 (ADR-023 로컬 메모리 그래프) |
| `edg` | 메모리 그래프 edge (ADR-023 로컬 메모리 그래프) |
| `tcand` | 영속 task candidate (ADR-028; ADR-028 Accepted 시 효력 발생) |
| `todo` | 사람이 확정한 영속 Todo (ADR-028; ADR-028 Accepted 시 효력 발생) |
| `tmut` | 영속 task transition receipt (ADR-028; ADR-028 Accepted 시 효력 발생) |
| `wctx` | 외부 work-context envelope (ADR-030; ADR-030 Accepted 시 효력 발생) |

### 3. 변환 범위

**엔티티 또는 상관** 문자열 식별자를 생성하는 모든 프로덕션
`Uuid::new_v4().to_string()` 호출 지점은 `maekon_core::generate_id("<prefix>")` 로
변환된다. 위 면제 범주의 호출 지점은 변경하지 않는다.

### 4. 테스트 어서션 업데이트

변환된 엔티티 ID 에 대해 UUID v4 wire 형식 (36자, 하이픈 4개, `uuid::Uuid::parse_str`)을
검증하던 통합 테스트 어서션은 `req_` prefix + 26자 ULID 형식을 검증하도록 업데이트된다.
면제 지점(예: `device_id` UUID 형식)의 어서션은 변경하지 않고 유지한다.

## 결과 (Consequences)

### 긍정적

- 크로스-경계 엔티티 및 상관 ID (`x-request-id`, audit `command_id`, suggestion ID,
  consent 레코드 등) 가 서버 ADR-055 ID 와 동일한 형식을 가지므로 운영자는 로그
  검색에 단일 패턴을 사용할 수 있다.
- 정렬 가능한 ID 는 별도 타임스탬프 컬럼 없이 시간 순서 조회를 가능하게 한다.
- prefix 가 있는 ID 는 로그와 디버그 출력에서 자기 설명적이다.
- 새로운 crate 의존성 없음 (`ulid` 는 이미 workspace 의존성이었음).

### 부정적

- 이전 릴리즈에서 생성된 기존 영속 엔티티 ID (예: JSON 의 `consent_id`) 는 UUID v4
  형식을 유지한다. 신규 ID 는 ULID 형식이 된다. `String` 필드 타입이 둘 다 수용하며
  마이그레이션이 필요 없고 스키마 검증이 UUID 형식을 강제하지 않는다.
- `generate_id` 는 잘못된 prefix 에서 패닉한다 — 이는 의도적 (개발자 오류)이며
  개발 중에 발견된다.

### 중립적

- `ulid::Ulid::new()` 는 단일 스레드에서 동일 밀리초 내에 단조 증가한다. 스레드 간
  단조성은 최선형이지만 유일성은 보장된다.
- 이 ADR 은 서버 레지스트리에 영향을 주지 않는다.
- 보안 민감 지점은 완전한 122비트 CSPRNG 랜덤성을 위해 `Uuid::new_v4()` 를 유지한다;
  면제 규칙은 임시가 아니라 영구적이다.

## 검토한 대안 (Alternatives Considered)

**A. 모든 곳에 bare UUID v4 유지.**
기각. 로그와 export 에서 크로스-경계 엔티티 ID 가 출처에 따라 구별되지 않으며,
정렬성을 위해 별도 타임스탬프 컬럼이 필요하다.

**B. 다른 ULID crate 또는 커스텀 생성기 사용.**
기각. `ulid` crate 는 이미 `maekon-web` 에서 사용하는 workspace 의존성이었다.

**C. nonce 포함 모든 지점에 UUID v7 채택.**
기각. UUID v7 도 상위 비트에 예측 가능한 타임스탬프를 포함하므로 암호화 nonce 에
부적절하다. 또한 `uuid` crate `v7` feature 는 추가 CSPRNG 인프라를 가져올 수 있다.

**D. 암호화 nonce 포함 모든 지점에 `generate_id` 사용.**
리뷰 후 기각. ULID 의 80비트 랜덤 필드와 예측 가능한 타임스탬프 컴포넌트는 완전한
예측 불가능성이 필요한 보안 컨텍스트에 불충분하다. UUID v4 는 122비트 CSPRNG 랜덤성을
제공하고 타임스탬프 누출이 없다.

## Update 2026-07-19 — 영속 task identifier

ADR-028은 Decision §2에 등록된 `tcand`, `todo`, `tmut` prefix를 추가 제안한다.
이 prefix는 ADR-028이 `Proposed`에서 `Accepted`로 바뀔 때만 효력이 생기며,
그 전에는 구현이 이를 발급하면 안 된다. 본 update는 이 ADR의 prefix syntax,
generator, validation, exemption rule을 변경하지 않는다.

## Update 2026-07-19 — Work-context envelope identifier

ADR-030은 외부 work-context envelope를 위해 Decision §2에 등록된 `wctx` prefix를
추가 제안한다. 이 prefix는 ADR-030이 `Proposed`에서 `Accepted`로 바뀔 때만 효력이
생기며, 그 전에는 구현이 이를 발급하면 안 된다. 본 update는 이 ADR의 prefix
syntax, generator, validation, exemption rule을 변경하지 않는다.

## 알려진 후속 작업 (Known Follow-ups)

1. **Prefix 거버넌스** — 새 엔티티 식별자는 PR 을 통해 이 ADR 의 위 표 (Decision §2)
   에 prefix 를 등록해야 한다.
2. **Lint 규칙** — 향후 `maekon-lint` 규칙이 엔티티 ID 프로덕션 코드에서 bare
   `Uuid::new_v4().to_string()` 호출을 금지하되, 위 Scope 섹션의 범주에 명시적
   면제를 부여할 수 있다.
3. **device_id 서버 계약 감사** — 향후 감사에서 서버가 `device_id` 에 임의 문자열을
   수용함을 확인하면, 새 PR 에서 `generate_id("dev")` 로 마이그레이션한다.

## 관련 문서 (Related Docs)

- `crates/maekon-core/src/id_generation.rs` — 구현
- `docs/architecture/ADR-021-config-consent-core-placement.md` — consent.rs 배경
