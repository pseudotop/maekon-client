[English](./ADR-008-network-resilience-patterns.md) | [한국어](./ADR-008-network-resilience-patterns.ko.md)

# ADR-008: 네트워크 복원성 패턴

**상태**: Accepted (2026-04-20 Proposed에서 승격; `map_reqwest_error` / `extract_retry_after` / circuit breaker / backoff 모두 구현되어 `maekon-network/src/{http_client,resilience,sync/remote_transport,integration/http_transport/mod,ai_llm_client/request}.rs` + gRPC 오류 매핑 전반에 사용 중)
**날짜**: 2026-03-09
**범위**: `maekon-network` 크레이트, 네트워크 관련 모든 어댑터

> **예시 코드의 CoreError 문법 참고**: 아래 예시는 [ADR-019](./ADR-019-error-code-infrastructure.md) 이전의
> 문법 `CoreError::RateLimit { retry_after_secs }`를 사용한다. ADR-019 이후에는
> 타입화된 `code` 필드가 있는 구조체 변형으로 작성해야 한다:
> ```rust
> CoreError::RateLimit {
>     code: maekon_core::error_codes::NetworkCode::RateLimit,
>     retry_after_secs,
> }
> ```
> 복원성 패턴 자체(retry/backoff/circuit-breaker/retry-after 파싱)는 ADR-019에 의해 변경되지 않는다.

---

## 컨텍스트

데스크톱 에이전트는 HTTP REST, SSE, WebSocket, gRPC를 통해 연결된 서버와 통신한다. 데스크톱 환경은 서버 프로세스가 절대 보지 못하는 네트워크 장애를 만든다: WiFi 단절, VPN 재연결, sleep/wake 사이클, 롤링 서버 배포. 에이전트는 버퍼링된 데이터를 잃거나 복구 중인 서버를 과부하시키지 않으면서 이를 처리해야 한다.

세 가지 증분 수정이 이 ADR이 해결하는 공백을 표면화했다.

| 피벗 커밋 | 날짜 | 경로 | 발견 |
|---|---|---|---|
| `b13a46b` | 2026-02-28 | `http_client.rs` | `RequestTimeout` + `is_retryable`: backoff 존재, **jitter 없음** |
| `ffa2478` | 2026-03-01 | `batch_uploader.rs` | Queue OOM 수정; flush retry 추가, **circuit breaker 없음** |
| `50ac66b` | 2026-03-08 | `sse_client.rs` | SSE 재연결 루프 추가, **jitter 없음** |

---

## 결정 사항

### 1. Jitter가 있는 지수 백오프

**규칙**: 모든 retry 루프는 jitter가 있는 지수 백오프를 사용해야 한다. 설정 가능한 최대값에서 상한을 설정한다.

```rust
// 지수 백오프 + 지터 계산
fn backoff_delay(attempt: u32, base_ms: u64, max_ms: u64) -> Duration {
    let exp = base_ms.saturating_mul(2u64.saturating_pow(attempt.min(10)));
    let jitter = rand::thread_rng().gen_range(0..=(exp / 4));
    Duration::from_millis((exp + jitter).min(max_ms))
}
```

현재 상태:

| 위치 | 상태 | 조치 |
|---|---|---|
| `HttpApiClient::execute_with_retry()` | Backoff, jitter 없음 | `backoff_delay()` 사용 |
| `SseStreamClient::connect()` | Backoff (`retry_delay * 2`), jitter 없음 | `backoff_delay()` 사용 |
| `BatchUploader::flush()` | Backoff, jitter 없음 | `backoff_delay()` 사용 |

기본 상한: SSE/HTTP는 30초, batch flush는 60초. jitter 없이는 동시에 연결이 끊긴 모든 클라이언트가 동일한 타임스탬프에 재연결하여 복구 중에 서버 부하가 급등한다.

---

### 2. Token Refresh 중복 제거

**규칙**: 한 번에 하나의 refresh 요청만 진행 중일 수 있다. `needs_refresh = true`를 확인한 동시 호출자들은 진행 중인 refresh가 완료될 때까지 기다려야 한다.

`auth.rs`의 현재 문제: 모든 호출자가 `RwLock` guard를 해제하고 개별적으로 `refresh()`를 호출하여 N개의 병렬 POST 요청을 보낸다.

필요한 패턴 — `AtomicBool` + `Notify`:

```rust
pub struct TokenManager {
    state: Arc<RwLock<Option<TokenState>>>,
    refreshing: AtomicBool,           // 리프레시 진행 중 여부
    refresh_notify: Arc<Notify>,      // 완료 시 대기 태스크 일괄 깨움
    client: reqwest::Client,
    base_url: String,
}

pub async fn get_token(&self) -> Result<String, CoreError> {
    if self.refreshing.load(Ordering::Acquire) {
        self.refresh_notify.notified().await;
    }

    let needs_refresh = { /* expiry check via RwLock */ };
    if needs_refresh {
        if self.refreshing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let result = self.do_refresh().await;
            self.refreshing.store(false, Ordering::Release);
            self.refresh_notify.notify_waiters();
            result?;
        } else {
            self.refresh_notify.notified().await; // 다른 태스크가 리프레시 중
        }
    }
    // state RwLock에서 토큰 반환
}
```

`refresh_notify`는 `Arc<Notify>` — 모든 `TokenManager` 클론이 공유한다.

---

### 3. Circuit Breaker

**규칙**: 반복 실패를 경험하는 네트워크 클라이언트는 복구 중인 서버를 과부하시키지 않도록 circuit breaker를 구현해야 한다.

상태: **Closed** (정상) → **Open** (요청 차단) → **Half-Open** (probe).

```rust
/// 서킷 브레이커 — 연속 장애 시 요청 차단
pub struct CircuitBreaker {
    state: AtomicU8,             // 0=Closed, 1=Open, 2=HalfOpen
    failure_count: AtomicU32,
    failure_threshold: u32,      // 기본값: 5
    recovery_timeout: Duration,  // 기본값: 30 s
    last_failure_ms: AtomicU64,  // Unix ms 타임스탬프
}
```

범위 (2026-03-09 원본): `BatchUploader`에 적용. flush 경로는 현재 호출당 `max_retries` 번 retry하며 스케줄러 tick 사이에 기억이 없어서, 5초 사이클마다 영구적으로 다운된 서버를 계속 두드리는 것이 가능하다.

`HttpApiClient::execute_with_retry()`는 이미 호출당 경계가 있으므로 제외된다.

**범위 업데이트 2026-04-20 (D7 확장)**: breaker는 이제 `RemoteEmbeddingProvider`, `AnalysisClient`, `RemoteOcrProvider`, `RemoteLlmProvider`, `HttpApiSession`도 보호한다. 동일한 엔드포인트를 대상으로 하는 여러 어댑터(예: 다른 모델의 두 OpenAI 클라이언트)가 하나의 breaker로 수렴하도록 5개 어댑터 모두 `scheme://host:port`로 키잉된 공유 `CircuitBreakerRegistry`를 통해 엔드포인트별 breaker를 확인한다.
원본 circuit-breaker 확장 설계는 내부 계획 산출물로 보관되며 공개 최소 내보내기의 일부가 아니다.

분류는 `resilience::classify_for_breaker`에서 중앙화된다:
- 5xx / transport / 401 / 429 → `Failure` (엔드포인트 건강)
- 2xx → `Success`
- 기타 4xx (400, 404, 422) → `Neutral` — 호출자 버그; 동일 엔드포인트를 대상으로 하는 다른 모든 호출자의 공유 breaker를 트리거해서는 안 됨

스트리밍 세션(`HttpApiSession`)은 3단계 의미론을 사용한다: 초기 HTTP 상태가 breaker를 구동하며, 스트림 중간 단절은 기록하지 않는다. 이는 "서버가 확인" = 성공인 BatchUploader 패턴과 일치한다.

`ai_ocr_client::ensure_runtime_ocr_model_ready`의 Ollama 모델 기능 probe는 의도적으로 래핑하지 않는다 — 요청당 한 번 발생하는 사이드카 호출은 범위 밖이며 주 OCR 전송이 breaker 상태를 구동한다.

통합 트랜스포트(`sync/remote_transport`, `integration/http_transport`)는 연기 상태 유지 — breaker 배치 결정(어댑터 레이어 vs port-trait 레이어)은 port-trait 라운드 pending 후속 작업이다.

---

### 4. Rate Limit 헤더 파싱

**규칙**: HTTP 429 응답은 반드시 `Retry-After` 헤더를 파싱해야 한다. 하드코딩된 폴백은 헤더가 없을 때만 허용된다.

`http_client.rs`의 현재 문제:

```rust
// 현재: Retry-After 헤더 무시, 60초 하드코딩
429 => Err(CoreError::RateLimit { retry_after_secs: 60 }),
```

필요한 교체:

```rust
/// 429 응답의 Retry-After 헤더를 파싱한다. 부재/파싱 실패 시 60초 기본값 반환.
fn extract_retry_after(response: &reqwest::Response) -> u64 {
    response.headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(60)
}

429 => Err(CoreError::RateLimit { retry_after_secs: extract_retry_after(&resp) }),
```

`execute_with_retry()`는 이미 delay를 `retry_after_secs`로 오버라이드하므로 추가 변경이 필요 없다.

---

## 결과

**반드시 해야 할 일** (새 네트워크 코드 머지의 게이트):

1. `backoff_delay()`가 `maekon-network/src/resilience.rs`에 추가되어 모든 인라인 delay 계산을 교체한다.
2. `extract_retry_after()`가 `check_response`의 하드코딩된 `60`을 교체한다.
3. `TokenManager`가 refresh 중복 제거를 위해 `AtomicBool` + `Arc<Notify>`를 갖게 된다.

**해야 할 일** (다음 스프린트):

4. `CircuitBreaker`가 `resilience.rs`에 구현되어 `BatchUploader`에 연결된다.
5. 각 패턴에 대한 단위 테스트: jitter 범위, 단일 refresh 어서션, circuit 상태 전환, 헤더 폴백.

**제약사항**: 새 워크스페이스 의존성이 필요 없다. `rand`는 이미 `maekon-vision`을 통해 존재한다. 모든 변경사항이 `maekon-network` 내에 포함된다 — ADR-001 §6의 크레이트 의존성 규칙과 일관성이 있다.
