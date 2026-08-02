[English](./http-status-error-mapping.md) | [한국어](./http-status-error-mapping.ko.md)

# HTTP Status 에러 매핑 패턴

이 문서는 외부 HTTP 호출의 응답 status code를 `CoreError` / `NetworkError` / `ApiError` variant로 일관되게 매핑하는 방법을 정의합니다 ([ADR-019](../architecture/ADR-019-error-code-infrastructure.ko.md) wire code 기준).

## 배경

ADR-019의 `err.code()` wire code는 Grafana group-by-code, code 기반 i18n 룩업, code 기반 retry 로직을 가능하게 합니다. 각 HTTP 디스패처가 non-success 응답을 임의의 variant로 매핑하면 이 세 소비자가 일관된 동작을 기대할 수 없습니다.

이 패턴이 확립되기 전에는 `maekon-network`, `maekon-audio`, `maekon-web` 전반 14개 디스패처가 각자 다른 arm subset(대개 401/404/429/503)만 처리하고 나머지는 `Network::Generic`이나 도메인 fallback(`OcrError`, `SttFailed` 등)으로 collapse했습니다. 프론트엔드는 408 timeout과 502 bad gateway를 동일한 `network.generic`으로 보게 됩니다.

## 정식 매핑

| HTTP status | CoreError variant | Wire code | 재시도 가능? |
|---|---|---|---|
| 401 Unauthorized | `Auth` | `auth.failed` | 아니오 |
| 403 Forbidden | `Auth` | `auth.failed` | 아니오 ([승인된 편차](#승인된-편차) 참조) |
| 404 Not Found | `NotFound` | `not_found.resource_missing` | 아니오 |
| 408 Request Timeout | `RequestTimeout` | `network.timeout` | 예 |
| 429 Too Many Requests | `RateLimit` | `network.rate_limit` | 예 (backoff) |
| 502 Bad Gateway | `ServiceUnavailable` | `service.unavailable` | 예 |
| 503 Service Unavailable | `ServiceUnavailable` | `service.unavailable` | 예 |
| 504 Gateway Timeout | `RequestTimeout` | `network.timeout` | 예 |
| 기타 non-success | 도메인별 또는 `Network` | 다양 | 경우에 따라 |

## Pre-response (reqwest) 타임아웃 처리

HTTP status code를 받기 전, `reqwest::Client::send()`는 transport-level 에러로 실패할 수 있습니다. **Timeout 에러는 반드시 `NetworkCode::Timeout`으로 라우팅**되어야 하며, 도메인별 fallback으로 가면 안 됩니다. 그렇지 않으면 Grafana에서 connection timeout과 500 server error가 동일하게 보입니다:

```rust
let response = builder.send().await.map_err(|e| {
    if e.is_timeout() {
        CoreError::RequestTimeout {
            code: NetworkCode::Timeout,
            timeout_ms: 0,  // sentinel 또는 self.timeout_secs * 1000 (가능하면)
        }
    } else {
        // 도메인별 fallback
        CoreError::Network {
            code: NetworkCode::Generic,
            message: format!("<컨텍스트>: {e}"),
        }
    }
})?;
```

Body read (`response.text().await`)와 stream chunk read에도 동일 패턴 적용 — 헤더 도착 후에도 타임아웃 가능. `NetworkError` 기반 디스패처는 `NetworkError::Timeout { timeout_ms }` 방출.

## 정식 구현

```rust
if !status.is_success() {
    let message = format!("<컨텍스트>: HTTP {status} {body}");
    return Err(match status.as_u16() {
        401 | 403 => CoreError::Auth {
            code: AuthCode::Failed,
            message,
        },
        404 => CoreError::NotFound {
            code: NotFoundCode::ResourceMissing,
            resource_type: "<설명>".into(),
            id: body,
        },
        408 | 504 => CoreError::RequestTimeout {
            code: NetworkCode::Timeout,
            timeout_ms: 0, // sentinel; 실제 timeout은 request-site 로그 참조
        },
        429 => CoreError::RateLimit {
            code: NetworkCode::RateLimit,
            retry_after_secs: 60, // 또는 Retry-After 헤더 파싱
        },
        502 | 503 => CoreError::ServiceUnavailable {
            code: ServiceCode::Unavailable,
            message,
        },
        _ => CoreError::Network {           // 또는 도메인별 fallback
            code: NetworkCode::Generic,
            message,
        },
    });
}
```

`NetworkError` 기반 디스패처는 `NetworkError::Auth / Timeout / RateLimited / ServiceUnavailable / Http`으로 대체(의미론적으로 동일; `From<NetworkError> for CoreError` impl이 같은 wire code 방출).

`ApiError` 기반 web handler는 `ApiError::Unauthorized / Forbidden / NotFound / ServiceUnavailable / BadRequest / Internal`로 매핑. `ApiError`는 전용 Timeout/TooManyRequests variant가 없으므로 408/429/504는 `ServiceUnavailable`로 collapse.

## 승인된 편차

병합된 매핑이 UI 로 하여금 **작동할 수 없는 지시**를 사용자에게 주게 만드는 경우에만
위 표에서 벗어날 수 있습니다. 벗어났다면 여기와 해당 디스패처의 모듈 doc 양쪽에
기록합니다 — 기록되지 않은 이탈은 drift 입니다.

### context-home 표면의 403 → `policy.denied` (#9625)

`maekon-network::context_home` 은 401 을 `auth.failed` 로, **403 을
`policy.denied`** 로 매핑합니다(둘을 `auth.failed` 로 병합하지 않음).

두 상태는 정반대의 대응을 요구합니다. 401 은 세션이 사라진 것이고 재로그인이
해결합니다. 403 은 인증은 됐으나 권한이 없는 것 — 재로그인은 아무것도 바꾸지
못하며, 여기서 "다시 로그인하세요"를 제시하는 표면은 사용자에게 빠져나갈 수 없는
루프를 건네는 셈입니다. wire code 의 명시된 목적(code 기반 i18n 룩업, code 기반
retry 로직)은 병합이 아니라 분리로 달성됩니다.

이 편차가 지킨 제약이며, 향후 편차도 지켜야 할 것:

- **신규 wire code 없음.** `policy.denied` 는 이미 동결된 54-code 레지스트리에
  있으므로 ADR-019 §5 재도입 체크리스트 대상이 아니고, `translateError` 에 전
  로케일 템플릿이 이미 있습니다.
- **`check_response` 는 건드리지 않음.** 공유 헬퍼를 넓히면 이 구분을 요청한 적
  없는 기존 호출자 전체의 동작이 바뀝니다. 분리는 필요한 디스패처 하나에
  국한됩니다.
- **나머지 arm 은 전부 표를 따르며**, 테스트(`canonical_table_arms_match_the_guide`)
  가 이를 고정해 편차가 조용히 자라지 못하게 합니다.

## 도메인별 fallback

`_` wildcard는 도메인별 variant가 있으면 `Network::Generic` 대신 사용. 예:

- OCR path: `_ => CoreError::OcrError { code: ProviderCode::OcrFailed, .. }`
- STT path: `_ => CoreError::SpeechToText { code: AudioCode::SttFailed, .. }`
- Analysis path: `_ => CoreError::Analysis { code: ProviderCode::AnalysisFailed, .. }`

알려진 status와 일치하지 않는 "기타" 버킷의 도메인 컨텍스트를 보존.

## 이 패턴을 따르는 디스패처

17개 디스패처. **구현** = 매핑 구현됨. **테스트** = 매핑을 검증하는 회귀 테스트 (specific-arm + fallback) 존재.

| Crate / module | 구현 | 테스트 |
|---|---|---|
| `maekon-network::http_client::check_response` | ✓ | ✓ specific (4 arm); fallback 의도적으로 `Internal` |
| `maekon-network::integration/http_transport::check_response` | ✓ | ✓ specific + fallback |
| `maekon-network::sync/remote_transport::check_response_status` | ✓ | ✓ specific + fallback |
| `maekon-network::ai_llm_client/request::send_and_parse` | ✓ | ✓ specific + fallback |
| `maekon-network::local_llm_session` (Ollama, 404 only) | ✓ | ✓ specific (404) + fallback (500) |
| `maekon-network::remote_embedding_client` | ✓ | ✓ specific + fallback |
| `maekon-network::ai_ocr_client::extract_elements` | ✓ | ✓ specific + fallback |
| `maekon-network::analysis_client::analyze` | ✓ | ✓ specific + fallback |
| `maekon-network::analysis_client::summarize` | ✓ | ✓ specific (3 spot-check) + fallback |
| `maekon-network::http_api_session` | ✓ | ✓ specific + fallback |
| `maekon-network::auth::login` | ✓ | ✓ specific + fallback |
| `maekon-network::auth::refresh` | ✓ | ✓ specific (4 arm: 401/429/503/504) + fallback (500) — iter-98 추가 |
| `maekon-network::sync/lan_transport::authenticate_with_peer` | ✓ | ⏸ deferred — LAN sync는 TLS 전용; mockito HTTP mock 으로 테스트 불가. rustls-TlsAcceptor + 테스트 cert 생성 필요. 시맨틱 매핑 구현은 완료되었고 defensive 성격 (LAN sync는 best-effort이고 peer-auth 실패는 드물고 non-catastrophic) |
| `maekon-audio::cloud_stt` | ✓ | ✓ specific + fallback |
| `maekon-audio::model_downloader` | ✓ | ✓ specific + fallback (`new_with_base_url` 주입 refactor 필요했음) |
| `maekon-web::services::ai_model_catalog_web_service` | ✓ (ApiError form) | ✓ specific + fallback |
| `maekon-network::context_home::map_failure_status` | ✓ (403 편차 — 위 참조) | ✓ 7개 — 정본 arm 표, 403 분리, bare-500 재시도성, 영구 4xx, Retry-After 전달, 메시지에 bearer 없음 |

## 의도적 제외

- **`maekon-network::oauth::token_exchange`** — 자체 `TokenErrorResponse` + `OAuthErrorKind` 분류(RFC 6749)가 semantic 차별화(`invalid_grant`, `invalid_client`, `server_error` 등)를 처리. HTTP-status arm을 위에 얹으면 중복/충돌.
- **`maekon-network::integration/auth::oidc_device_flow`** — 동일; OIDC device flow는 RFC 8628 기준 자체 에러 코드(`authorization_pending`, `slow_down`, `expired_token`).
- **`maekon-network::sse_client`** — retry loop, 무한 재시도 + 로그; `CoreError` 방출 없음.
- **`maekon-network::sync/lan_transport::push/pull`** — best-effort (returns `Ok(bool)`), non-fatal peer failures.

## 신규 HTTP 디스패처 추가

새 HTTP 호출 도입 시:

1. `if !status.is_success()` 블록은 위 canonical 매핑 사용
2. `_` wildcard는 적절한 도메인별 fallback 선택
3. 위 디스패처 테이블에 행 추가
4. (선택) 해당 엔드포인트의 401 → `Auth`, 429 → `RateLimit` 등을 검증하는 회귀 테스트 추가

## 검증

```bash
# Wire-contract snapshot test가 code 안정성 보장:
cargo test -p maekon-core --test wire_contract_snapshot

# 디스패처별 테스트 예시:
cargo test -p maekon-network --lib http_client::tests::forbidden_403_maps_to_auth
cargo test -p maekon-network --lib http_client::tests::request_timeout_408_maps_to_timeout
cargo test -p maekon-network --lib http_client::tests::bad_gateway_502_maps_to_service_unavailable
cargo test -p maekon-network --lib http_client::tests::gateway_timeout_504_maps_to_timeout
```

## 관련

- [ADR-019](../architecture/ADR-019-error-code-infrastructure.ko.md) — wire code 인프라
- [gRPC 에러 매핑 가이드](./grpc-error-mapping.ko.md) — gRPC Status code의 유사 패턴
