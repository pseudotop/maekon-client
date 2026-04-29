# 정책 토큰 계약 (플랫폼 ↔ 클라이언트)

자동화 명령 실행에 사용하는 정책 토큰의 발급/검증 계약을 정의한다.

## 토큰 형식

- 비서명 토큰:
  - `{policy_id}:{nonce}`
- 비서명 명령 스코프 토큰:
  - `{policy_id}:{nonce}:h{command_hash}`
- 서명 토큰 (`require_signed_token=true`):
  - `{policy_id}:{nonce}:{signature}`
- 서명 명령 스코프 토큰 (`require_signed_token=true`):
  - `{policy_id}:{nonce}:h{command_hash}:{signature}`

## 필드 규칙

- `policy_id`
  - 클라이언트의 활성 정책 캐시에 존재해야 한다.
- `nonce`
  - 최소 8자 이상이어야 한다.
  - `a-z`, `A-Z`, `0-9`, `_`, `-` 문자만 허용한다.
  - 무작위 UUID 계열 값 생성을 권장한다.
- `signature` (서명 정책일 때)
  - 64자 hex 문자열이어야 한다.
  - 계산식:
    - `sha256("{policy_id}:{nonce}:{secret}")`
    - 명령 스코프 포함 시: `sha256("{policy_id}:{nonce}:{command_hash}:{secret}")`
- `command_hash` (선택)
  - 64자 hex 문자열이어야 한다.
  - 토큰에서는 `h` 접두어를 포함한 `h{command_hash}` 형태여야 한다.
  - 명령 스코프(`command_id`, `session_id`, `action`, `timeout_ms`)에 토큰을 바인딩한다.

## 서명 시크릿

- 환경 변수:
  - `ONESHIM_POLICY_TOKEN_SIGNING_SECRET`
- 사용 주체:
  - 플랫폼 발급기(서명 정책).
  - 클라이언트 검증기(`require_signed_token=true`).

## 검증 조건

클라이언트는 아래 조건을 모두 만족할 때만 토큰을 유효로 판단한다.

1. 토큰 형식이 유효하다.
2. 정책 캐시 TTL이 만료되지 않았다.
3. `policy_id`가 캐시 정책과 매칭된다.
4. `nonce` 형식이 유효하다.
5. 캐시 TTL 범위 내 재사용 토큰이 아니다.
6. 서명 정책일 경우:
   - 서명 필드가 존재하고 hex 형식이 유효하다.
   - 서명 다이제스트가 계산값과 일치한다.
7. `command_hash`가 포함된 경우:
   - 토큰 해시와 명령 스코프에서 재계산한 해시가 일치해야 한다.

## 발급 API (클라이언트 유틸리티)

- `PolicyClient::issue_command_token(policy_id)`
  - nonce를 생성한다.
  - 정책 설정에 따라 서명/비서명 토큰을 발급한다.
  - 서명 정책인데 시크릿이 없으면 실패 폐쇄(fail-closed)한다.
- `PolicyClient::issue_command_token_for_command(policy_id, cmd)`
  - nonce를 생성한다.
  - 토큰에 명령 스코프 해시를 포함한다.
  - 다른 명령으로의 토큰 재사용을 차단한다.

## 보안 메모

- 재사용 방지는 캐시 TTL 범위에서 동작하므로, 플랫폼은 짧은 명령 발급 윈도우를 권장한다.
- 시크릿 교체 시 정책 캐시 무효화/재동기화와 함께 운영해야 한다.
