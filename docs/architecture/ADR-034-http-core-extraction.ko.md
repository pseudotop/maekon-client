[English](./ADR-034-http-core-extraction.md) | [한국어](./ADR-034-http-core-extraction.ko.md)

# ADR-034: `maekon-http-core` — 어댑터 아래에 놓는 공유 아웃바운드 HTTP 기반

**상태**: Proposed — 2026-08-05
**Date**: 2026-08-05
**Scope**: 신규 크레이트 `maekon-http-core`(하드닝된 아웃바운드 클라이언트, 재시도/백오프, 서킷 브레이커); `maekon-network`(소비자로 전환, 전송 계층은 유지); 향후 `maekon-integration`
**Related**: ADR-001 §3(DI·어댑터 경계), D7 circuit-breaker broadening(2026-04-20), #9855(Google Calendar OAuth 팩토리), #9639(integration 롤업)
**Issue**: TBD

---

## 배경 (Context)

서드파티 통합은 `crates/maekon-network/src/integration/` 에 산다 — 2026-08-05 실측 **12,707줄**. 이 하위 트리에는 이미 Google Calendar 커넥터가 완성돼 있다(1,561줄: HTTP API, 매핑, 커서, health, 통합 테스트 2개). 빠진 것은 등록된 OAuth 팩토리 하나뿐이다(#9855). 제품 의도는 **커넥터 프리셋을 몇 개 더** 추가하되, 광범위한 카탈로그가 아니라 필수 도구로 한정하는 것이다.

`CLAUDE.md` 의 아키텍처 규칙은 명시적이다:

> **Forbidden**: 어댑터 크레이트 간 직접 의존(예: monitor → storage). 크로스-크레이트 통신은 전부 `maekon-core` 트레이트를 거쳐야 한다.

따라서 "통합을 자체 크레이트로 옮긴다"를 순진하게 실행할 수 없다. `integration/` 은 `maekon-network` **내부**에 있는 원시요소에 얹혀 있고, 새 어댑터 크레이트는 다른 어댑터 크레이트에 의존할 수 없다.

### `integration/` 이 실제로 빌려 쓰는 것

`grep -rh "^use crate::" crates/maekon-network/src/integration` 실측:

```
crate::error::NetworkError
crate::outbound::{hardened_client_builder, read_text_capped, BodyReadError, TransportPolicy}
crate::provider_error_body::provider_error_body_state
crate::resilience::{jittered_backoff_delay, RetryBackoffGate, RetryBackoffPolicy,
                    extract_retry_after, scale_duration, MAX_RETRY_AFTER_SECS}
```

부수적 헬퍼가 아니다. `hardened_client_builder` 는 리다이렉트 정책을 고정하고, `read_text_capped` 는 응답 본문 읽기 크기를 제한한다(DoS 가드). **이것을 두 번째 크레이트로 복제하는 선택지는 즉시 기각해야 한다** — 보안 원시요소의 사본이 둘이면 갈라지고, 다음 수정은 한쪽에만 들어간다.

### 원시요소는 통합 전용이 아니다

| 모듈 | LOC | 크레이트 내부 의존 | 소비 모듈 | 그중 `integration/` |
|---|---:|---|---:|---:|
| `outbound.rs` | 327 | **없음** | 2 | 1 |
| `resilience.rs` | 361 | `crate::error::NetworkError`(1곳) | 14 | 3 |
| `circuit_breaker.rs` | 585 | **없음** | 8 | 0 |

`resilience` 는 네트워크 크레이트 **전체**의 기반이다 — `auth`, `http_client`, `sse_client`, `batch_uploader`, `grpc`, AI 클라이언트 4종, `sync`, `context_home`. 통합이 소유하고 빌려주는 물건이 아니다. 그래서 둘 중 어느 쪽 **안**이 아니라 **아래**에 놓아야 한다.

## 결정 (Decision)

**`maekon-http-core`** 를 도입한다. 도메인 지식도 전송 계층 의견도 없는, 아웃바운드 HTTP 역학만 담은 작은 크레이트다.

```
                maekon-core            (도메인 모델 + 포트 트레이트)
                  ↑        ↑
       maekon-http-core    │           (하드닝 클라이언트, 재시도/백오프, 브레이커)
          ↑         ↑      │
maekon-network   maekon-integration    (어댑터 — 서로 의존하지 않는다)
```

`maekon-http-core` 는 `maekon-core` 에 의존해도 된다(이미 `maekon_core::backoff::exponential_delay` 를 쓴다). 어댑터가 어댑터에 의존하지 않으므로 ADR-001 §3 이 유지된다.

### 내용물

| 이동 | 이유 |
|---|---|
| `outbound.rs` | 내부 의존 0 — 그대로 들어냄 |
| `resilience.rs` | 내부 의존 1개, 절단 가능(아래) |
| `circuit_breaker.rs` | 내부 의존 0 — **의도적으로 포함**(아래) |

`circuit_breaker` 는 오늘 `integration/` 소비자가 없어 이동이 필수는 아니다. 그럼에도 포함하는 이유는, 남겨두면 이 ADR 이 해결하려는 문제를 그대로 재현하기 때문이다 — 서드파티 API 를 호출하는 커넥터는 엔드포인트별 브레이커를 원하게 되고, 그때 선택지는 `maekon-network` 의존(금지) 아니면 복제(위에서 기각)뿐이다.

### 끊어야 할 결합 — 단 하나

`RetryBackoffGate::on_failure` 가 `&NetworkError` 를 받는 이유(`resilience.rs:114`)는 오직 하나 — *서버가 Retry-After 힌트를 줬는가?*

```rust
pub fn on_failure(&mut self, now: Instant, error: &NetworkError) -> Duration {
    let delay = match error {
        NetworkError::RateLimited { retry_after_secs } => { /* clamp */ }
        _ => jittered_backoff_delay(...),
    };
```

오류 참조 대신 **답 자체**를 받는다:

```rust
/// 실패한 시도가 백오프 게이트에 알려주는 것.
pub enum RetryHint {
    /// 서버가 이만큼 기다리라고 지시했다(429 / Retry-After).
    After(u64),
    /// 힌트 없음 — 지수 백오프를 쓴다.
    None,
}
```

`maekon-network` 는 `impl From<&NetworkError> for RetryHint` 한 줄을 유지하므로, 호출부는 `gate.on_failure(now, (&err).into())` 가 되고 clamp 동작은 그대로다. 향후 `maekon-integration` 은 자기 오류 타입에서 자기 변환을 제공한다.

추출 과정의 **의미론적 변경은 이것이 전부**다. 나머지는 import 경로 재작성이다.

## 기각한 대안

**`integration/` 을 `maekon-network` 에 그대로 둔다.** 오늘로선 유효하고 더 싸다 — 다만 그 크레이트의 본래 일은 client↔server 전송(HTTP/gRPC/SSE/auth)이고, 서드파티 커넥터는 계속 커질 별개 관심사다. 이 ADR 은 **커넥터 프리셋을 여럿 만들 계획이기 때문에** 쓰였다. 커넥터가 하나뿐이라면 추출은 값을 못 한다.

**원시요소를 `maekon-core` 로 승격한다.** 기각: `hardened_client_builder` 는 `reqwest::ClientBuilder` 를 반환한다. 이를 `maekon-core` 에 넣으면 모든 크레이트가 의존하는 도메인 크레이트로 `reqwest` 가 끌려 들어와, 인프라 없이 테스트 가능하다는 성질(ADR-001 §5)이 파괴된다.

**원시요소를 `maekon-integration` 에 복제한다.** 위와 같이 기각: `read_text_capped` 는 본문 크기 상한이고 `hardened_client_builder` 는 리다이렉트 정책을 고정한다. 보안 통제의 사본 2개는 공유 사본 1개보다 엄밀히 나쁘다.

## 마이그레이션

어느 단계도 트리를 반쯤 옮긴 상태로 남기지 않도록 의도적으로 쪼갠다:

1. **P1 — 크레이트 생성, 3개 모듈 이동, 결합 절단.** `maekon-network` 가 이동한 이름을 재수출(`pub use maekon_http_core::…`)하므로 다른 크레이트는 아직 변하지 않는다. 검증: 워크스페이스 빌드, 기존 테스트 무수정 통과.
2. **P2 — 소비자 재지정.** 약 20곳의 `use crate::{outbound,resilience,circuit_breaker}::…` 를 `maekon_http_core::…` 로 바꾸고 재수출을 제거한다. 검증: `maekon-network` 에 `pub use maekon_http_core` 가 남지 않는다.
3. **P3 — `maekon-integration` 추출**, `integration/` 을 옮기고 `maekon-core` + `maekon-http-core` 에만 의존시킨다. 검증: 새 크레이트를 포함한 `./scripts/check-crate-boundaries.sh` 통과, `maekon-integration` 이 `maekon-network` 의존 트리에 없고 그 역도 없음.
4. **P4 — 커넥터 레지스트리.** 지원 커넥터를 명시하는 레지스트리 + 커넥터별 feature flag 로, "필수 도구만" 을 관례가 아니라 **컴파일 타임 사실**로 만든다.

P1·P2 는 단독으로 성립하며 P3 가 없더라도 할 값어치가 있다 — 형제 모듈 묶음이던 resilience 기반이 이름 붙은 테스트 가능한 단위가 된다.

## 결과 (Consequences)

**긍정.** 복제 없이 어댑터 규칙이 유지된다. 커넥터 크레이트가 나머지 클라이언트와 같은 하드닝 클라이언트·브레이커를 쓴다. `maekon-network` 가 제 일에 맞게 줄어든다. 커넥터별 feature flag 가 표현 가능해진다.

**부정.** 빌드할 크레이트가 하나 늘고, P2 에서 약 20개 파일의 import 경로가 흔들린다. resilience 원시요소를 고치는 변경이 이제 크레이트 하나가 아니라 둘을 건드린다.

**중립.** `maekon-http-core` 는 오늘이라면 소유자가 애매할 향후 아웃바운드 관심사(프록시 정책, 호스트별 rate limit)의 자연스러운 거처가 된다.

## Non-goals

- 재시도·백오프·브레이커·리다이렉트 **동작** 변경. 추출은 동작 보존이며, `RetryHint` 는 기존 `RateLimited` clamp 를 정확히 재현한다.
- 어떤 커넥터를 출하할지 결정. 그것은 P4 레지스트리의 내용이지 이 ADR 의 것이 아니다.
- `NetworkError` 이동. `maekon-network` 에 남고, `resilience` 의 사용 1곳만 절단했다(P1).
- gRPC/SSE 전송. client↔server 관심사이므로 `maekon-network` 에 남는다.

## Amendments (개정)

**P3 실행 (2026-08-05).** 초안과 두 곳이 달라졌으며, 둘 다 P3 가 드러낸 사실이 강제한 것이다:

1. **`provider_error_body` 는 결국 `maekon-http-core` 로 옮겼다** — 원래 non-goal 을 뒤집는다. 그 non-goal 은 P1 범위 기준의 서술이었고, P3 에서 이 모듈의 소비자가 새 경계의 **양쪽**에 존재함이 드러났다(`maekon-network` 5곳 + `maekon-integration` 의 `http_transport` 1곳). 어느 쪽도 서로 의존할 수 없다. `reqwest::StatusCode` 만 쓰는 107줄 순수 아웃바운드 역학 — 이 크레이트의 관할 그 자체 — 이므로 복제나 절단보다 이동이 맞다.
2. **Google Calendar OAuth 리터럴 2개는 `maekon_core::ports::oauth` 로 옮겼다** (`GOOGLE_CALENDAR_PROVIDER_ID`, `GOOGLE_CALENDAR_READONLY_SCOPE`). 커넥터(`maekon-integration`)는 SecretStore 네임스페이스·요청 스코프로, OAuth 프로바이더 레지스트리(`maekon-network`)는 대응 provider config 구성에 같은 리터럴을 쓴다. 어댑터끼리는 의존할 수 없고 사본 2개가 갈라지면 토큰 조회가 조용히 깨지므로, 단일 사본을 core 에 둔다. 커넥터 모듈이 재수출한다.

Decision 절에 스케치했던 `From<&NetworkError> for RetryHint` impl 은 **구현하지 않았다** — 게이트의 유일한 소비자가 `CoreError` 를 들고 있어 도착 즉시 dead code 였을 것이다. `runtime_loop` 는 지역 7줄 `retry_hint()` 로 변환한다.

**P4 실행 (2026-08-06)** — "레지스트리 + 그것이 가능케 하는 전부"보다 의도적으로 좁게:

- `maekon-integration/src/connectors.rs` 가 컴파일 타임 레지스트리다: 필수 도구당 `BuiltinConnector` 1행, 각자 Cargo feature(`connector-google-calendar`, 기본 on) 뒤에 선다. 테스트가 목록을 정확히 필수 집합으로 고정하고 MK-EXT read-only 스코프 불변식을 기계적으로 강제한다.
- 컴포지션 루트(`oauth_provider_registry.rs`)가 레지스트리를 읽어 provision 된 항목마다 `OAuthProviderConfig` 를 추가한다 — #9855 등록을 결정 A(캘린더는 필수)로 실행한 것. 크레덴셜(`MAEKON_GOOGLE_CALENDAR_OAUTH_CLIENT_ID`) 부재는 오류가 아니라 비활성 커넥터다.
- **P4 범위에서 명시적으로 제외**: MK-EXT extension IPC/UI 표면 부활. #9639 가 `register_package` 호출부 부재로 "동작할 수 없는 기능을 광고하는 IPC"라며 은퇴시켰고, 부활 절차(어노테이션 → IPC 등록 → 실제 `register_package` 호출부)가 `src-tauri/src/lib.rs` 에 문서화되어 `tests/ipc_command_contract.rs` 가드가 지킨다. OAuth provider 배선은 보이지 않는 배관이라 안전하지만, 표면 절반 부활은 dead-advertising 결함을 재현한다. 사용자 도달 vertical(연결 UI → sync 스케줄 → 타임라인)은 이 ADR 계층 위의 별도 슬라이스다.
