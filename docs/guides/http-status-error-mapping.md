[English](./http-status-error-mapping.md) | [한국어](./http-status-error-mapping.ko.md)

# HTTP Status Error Mapping Pattern

This guide defines how HTTP status codes from any external HTTP call should map into `CoreError` / `NetworkError` / `ApiError` variants for consistent wire codes per [ADR-019](../architecture/ADR-019-error-code-infrastructure.md).

## Motivation

ADR-019's `err.code()` wire codes enable group-by-code in Grafana, code-based i18n lookup, and code-based retry logic. If each HTTP dispatcher maps non-success responses to arbitrary variants, these three consumers can't rely on consistent behavior.

Before this pattern was established, 14 dispatchers across `oneshim-network`, `oneshim-audio`, `oneshim-web` each had a different subset of arms (typically just 401/404/429/503) and collapsed the rest into `Network::Generic` or a domain-specific catch-all (e.g., `OcrError`, `SttFailed`). Frontend saw `network.generic` for a 408 timeout the same as for a 502 bad gateway.

## The canonical mapping

| HTTP status | CoreError variant | Wire code | Retryable |
|---|---|---|---|
| 401 Unauthorized | `Auth` | `auth.failed` | No |
| 403 Forbidden | `Auth` | `auth.failed` | No |
| 404 Not Found | `NotFound` | `not_found.resource_missing` | No |
| 408 Request Timeout | `RequestTimeout` | `network.timeout` | Yes |
| 429 Too Many Requests | `RateLimit` | `network.rate_limit` | Yes (with backoff) |
| 502 Bad Gateway | `ServiceUnavailable` | `service.unavailable` | Yes |
| 503 Service Unavailable | `ServiceUnavailable` | `service.unavailable` | Yes |
| 504 Gateway Timeout | `RequestTimeout` | `network.timeout` | Yes |
| Other non-success | Domain-specific or `Network` | various | Depends |

## Pre-response (reqwest) timeout handling

Before we have an HTTP status code at all, `reqwest::Client::send()` can fail with a transport-level error. **Timeout errors must route to `NetworkCode::Timeout`**, not to the domain-specific fallback. Otherwise a connection-timeout and a 500-server-error would look identical in Grafana:

```rust
let response = builder.send().await.map_err(|e| {
    if e.is_timeout() {
        CoreError::RequestTimeout {
            code: NetworkCode::Timeout,
            timeout_ms: 0,  // sentinel, or self.timeout_secs * 1000 if available
        }
    } else {
        // domain-specific fallback
        CoreError::Network {
            code: NetworkCode::Generic,
            message: format!("<context>: {e}"),
        }
    }
})?;
```

The same pattern applies to body-read (`response.text().await`) and stream-chunk reads — both can time out after the headers have arrived. For `NetworkError`-based dispatchers, emit `NetworkError::Timeout { timeout_ms }` instead.

## Canonical implementation

```rust
if !status.is_success() {
    let message = format!("<context>: HTTP {status} {body}");
    return Err(match status.as_u16() {
        401 | 403 => CoreError::Auth {
            code: AuthCode::Failed,
            message,
        },
        404 => CoreError::NotFound {
            code: NotFoundCode::ResourceMissing,
            resource_type: "<describe>".into(),
            id: body,
        },
        408 | 504 => CoreError::RequestTimeout {
            code: NetworkCode::Timeout,
            timeout_ms: 0, // sentinel; request-site logs actual timeout
        },
        429 => CoreError::RateLimit {
            code: NetworkCode::RateLimit,
            retry_after_secs: 60, // or parse Retry-After header
        },
        502 | 503 => CoreError::ServiceUnavailable {
            code: ServiceCode::Unavailable,
            message,
        },
        _ => CoreError::Network {           // or domain-specific fallback
            code: NetworkCode::Generic,
            message,
        },
    });
}
```

For `NetworkError`-based dispatchers, substitute `NetworkError::Auth / Timeout / RateLimited / ServiceUnavailable / Http` (semantically equivalent; the `From<NetworkError> for CoreError` impl emits the same wire codes).

For `ApiError`-based web handlers, map to `ApiError::Unauthorized / Forbidden / NotFound / ServiceUnavailable / BadRequest / Internal` — `ApiError` lacks dedicated Timeout/TooManyRequests variants, so 408/429/504 collapse into `ServiceUnavailable`.

## Domain-specific fallback

The `_` wildcard should use a domain-specific variant when one exists, not always `Network::Generic`. Examples:

- OCR path: `_ => CoreError::OcrError { code: ProviderCode::OcrFailed, .. }`
- STT path: `_ => CoreError::SpeechToText { code: AudioCode::SttFailed, .. }`
- Analysis path: `_ => CoreError::Analysis { code: ProviderCode::AnalysisFailed, .. }`

This preserves domain context for the "didn't match any known status" bucket.

## Dispatchers currently following this pattern

16 dispatchers. **Impl** = mapping is implemented. **Tests** = has regression tests covering the mapping (specific-arm + fallback).

| Crate / module | Impl | Tests |
|---|---|---|
| `oneshim-network::http_client::check_response` | ✓ | ✓ specific (4 arms); fallback intentionally `Internal` |
| `oneshim-network::integration/http_transport::check_response` | ✓ | ✓ specific + fallback |
| `oneshim-network::sync/remote_transport::check_response_status` | ✓ | ✓ specific + fallback |
| `oneshim-network::ai_llm_client/request::send_and_parse` | ✓ | ✓ specific + fallback |
| `oneshim-network::local_llm_session` (Ollama, 404-only) | ✓ | ✓ specific (404) + fallback (500) |
| `oneshim-network::remote_embedding_client` | ✓ | ✓ specific + fallback |
| `oneshim-network::ai_ocr_client::extract_elements` | ✓ | ✓ specific + fallback |
| `oneshim-network::analysis_client::analyze` | ✓ | ✓ specific + fallback |
| `oneshim-network::analysis_client::summarize` | ✓ | ✓ specific (3 spot-checks) + fallback |
| `oneshim-network::http_api_session` | ✓ | ✓ specific + fallback |
| `oneshim-network::auth::login` | ✓ | ✓ specific + fallback |
| `oneshim-network::auth::refresh` | ✓ | ✓ specific (4 arms: 401/429/503/504) + fallback (500) — added iter-98 |
| `oneshim-network::sync/lan_transport::authenticate_with_peer` | ✓ | ✓ 6 tests (iter-194) — refactored the inline status `match` into a pure `map_challenge_status_to_error(status_code, peer_id) -> CoreError` helper; covered by 5 canonical status tests (401/403/429/503/504) + 1 500-fallback test. Skipped the originally-planned rustls-TlsAcceptor fixture in favor of extracting the mapping to a pure function — same coverage, fraction of the test-infra cost. Follow-up #5 from [ADR-019 Known Follow-ups](../architecture/ADR-019-error-code-infrastructure.md#known-follow-ups-not-blocking) |
| `oneshim-audio::cloud_stt` | ✓ | ✓ specific + fallback |
| `oneshim-audio::model_downloader` | ✓ | ✓ specific + fallback (needed `new_with_base_url` injection) |
| `oneshim-web::services::ai_model_catalog_web_service` | ✓ (ApiError form) | ✓ specific + fallback |

## Intentionally excluded

- **`oneshim-network::oauth::token_exchange`** — has its own `TokenErrorResponse` + `OAuthErrorKind` classification per RFC 6749 that handles semantic differentiation (`invalid_grant`, `invalid_client`, `server_error` etc.). Layering HTTP-status arms on top would duplicate/conflict.
- **`oneshim-network::integration/auth::oidc_device_flow`** — same reasoning; OIDC device flow has its own error codes per RFC 8628 (`authorization_pending`, `slow_down`, `expired_token`).
- **`oneshim-network::sse_client`** — retry loop, logs + retries indefinitely; no `CoreError` emission.
- **`oneshim-network::sync/lan_transport::push/pull`** — best-effort semantics (returns `Ok(bool)`), non-fatal peer failures.

## Adding a new HTTP dispatcher

When introducing a new HTTP call:

1. Use this canonical mapping for the `if !status.is_success()` block.
2. Pick the appropriate domain-specific fallback for the `_` wildcard.
3. Add a row to the dispatcher table above.
4. Optional: add a regression test asserting e.g. 401 → `Auth`, 429 → `RateLimit` for your specific endpoint.

## Verification

```bash
# The wire-contract snapshot test ensures codes stay stable:
cargo test -p oneshim-core --test wire_contract_snapshot

# Per-dispatcher tests (examples):
cargo test -p oneshim-network --lib http_client::tests::forbidden_403_maps_to_auth
cargo test -p oneshim-network --lib http_client::tests::request_timeout_408_maps_to_timeout
cargo test -p oneshim-network --lib http_client::tests::bad_gateway_502_maps_to_service_unavailable
cargo test -p oneshim-network --lib http_client::tests::gateway_timeout_504_maps_to_timeout
```

## Related

- [ADR-019](../architecture/ADR-019-error-code-infrastructure.md) — wire code infrastructure
- [gRPC Error Mapping Guide](./grpc-error-mapping.md) — parallel pattern for gRPC Status codes
