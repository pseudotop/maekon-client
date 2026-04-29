[English](./ADR-002-os-gui-interaction-boundary.md) | [한국어](./ADR-002-os-gui-interaction-boundary.ko.md)

# ADR-002: OS GUI 상호작용 경계와 런타임 분리

**상태**: 채택됨 (Accepted) — 2026-04-20 제안됨 → 채택됨 승격. M3 네이티브 어댑터 구현 완료.
**날짜**: 2026-02-25
**범위**: `oneshim-core` (`ElementFinder` / `OverlayDriver` / `FocusProbe` / `InputDriver` 포트), `oneshim-automation` (세션 상태 머신, capability/ticket 처리), `oneshim-vision` (R-tree 공간 인덱스, app-specific element-type override, accessibility 어댑터), `oneshim-web` (capability-token 핸들러), `src-tauri` (`MagicOverlayDriver` — ADR-004 Tauri v2 마이그레이션으로 원래 계획된 `oneshim-ui` 네이티브 오버레이 대신 WebView 브릿지 채택)

---

## 컨텍스트

현재 스택은 이미 다음을 지원한다.

- Scene 분석 (`GET /api/automation/scene`)
- Scene 액션 실행 (`POST /api/automation/execute-scene-action`)
- `oneshim-automation`의 정책/프라이버시/감사 제어

하지만 OS GUI 상호작용에는 더 강한 보장이 필요하다.

1. 현재 포커스된 네이티브 창 기준으로 컨트롤을 식별해야 한다.
2. 액션 전에 OS 화면 위에 명시적 하이라이트를 보여줘야 한다.
3. 사용자 확인 이후에만 실행해야 한다.

웹 렌더링만으로는 임의 네이티브 창 위 신뢰 가능한 오버레이와 실행 시점 포커스 일관성을 보장하기 어렵다.

---

## 결정 사항

### 1. Control Plane / Execution Plane을 분리한다

- **Control Plane** (`oneshim-web`): 관리, 모니터링, API 오케스트레이션
- **Execution Plane** (로컬 런타임): 포커스 조회, scene 분석, 네이티브 오버레이 하이라이트, 입력 실행

`oneshim-web`은 OS 네이티브 상호작용을 직접 호출하지 않는다.

### 2. 정책/프라이버시/감사는 자동화 계층 단일 관문을 유지한다

모든 GUI 실행 경로는 `oneshim-automation` 정책/프라이버시/감사 체크를 통과해야 한다. 핸들러에서 드라이버로 우회 호출을 금지한다.

### 3. 세션 기반 상호작용 프로토콜을 채택한다

흐름:

1. 후보 `propose`
2. 후보 `highlight`
3. 후보 `confirm`
4. 짧은 수명의 티켓으로 `execute`
5. `verify` + `audit`

원샷 직접 실행 경로는 호환성을 위해 유지하지만, 고위험 UX의 기본 경로로 사용하지 않는다.

### 4. 포커스/오버레이 코어 계약을 명시한다

`oneshim-core`에 포트를 추가한다.

```rust
#[async_trait]
pub trait OverlayDriver: Send + Sync {
    async fn show_highlights(&self, req: HighlightRequest) -> Result<HighlightHandle, CoreError>;
    async fn clear_highlights(&self, handle_id: &str) -> Result<(), CoreError>;
}

#[async_trait]
pub trait FocusProbe: Send + Sync {
    async fn current_focus(&self) -> Result<FocusSnapshot, CoreError>;
    async fn validate_execution_binding(
        &self,
        binding: &ExecutionBinding,
    ) -> Result<FocusValidation, CoreError>;
}
```

`validate_execution_binding`은 confirm/execute 재검증을 단일 호출로 처리해 TOCTOU 위험을 줄인다.

### 5. `UiSceneElement`를 재사용하고 후보 모델 중복을 피한다

`GuiCandidate`는 `UiSceneElement`를 래핑/프로젝션한 모델로 정의하고, 상호작용 메타데이터(랭킹 근거, 실행 가능 플래그)만 추가한다. 병렬 중복 모델을 만들지 않는다.

### 6. V2 세션 저장은 메모리 기반으로 시작한다

V2 세션 상태는 `oneshim-automation` 메모리에 저장한다.

- `Arc<RwLock<HashMap<SessionId, GuiInteractionSession>>>`
- TTL 기반 생명주기 + 주기적 정리(기본 30초)
- Phase 0-2에서는 SQLite 영속화를 하지 않는다

향후 영속화가 필요하면 `oneshim-core` storage port를 통해 도입한다.

### 7. 티켓 무결성과 세션 capability 인증을 강제한다

`GuiExecutionTicket` 필드:

- `session_id`, `focus_hash`, `scene_id`, `element_id`, `action_hash`
- `issued_at`, `expires_at`, `nonce`
- `signature` (HMAC)

HMAC 키(고정 환경설정):

- GUI V2 엔드포인트가 활성화된 경우 `ONESHIM_GUI_TICKET_HMAC_SECRET` 환경설정을 필수로 사용한다.
- 키가 없거나 비어 있으면 fail-closed로 세션 생성/티켓 발급을 거부한다.

`/sessions/:id/*` 엔드포인트는 세션 생성 시 발급한 per-session capability token(예: `X-Gui-Session-Token`)을 요구한다.

### 8. 접근성 우선, OCR 폴백 전략을 유지한다

탐지 순서:

1. 접근성 트리 어댑터
2. OCR 기반 finder 폴백
3. 선택적 템플릿 매처

후보 정렬은 소스 신뢰도, confidence, role 의도, 포커스 창 일치도를 합산한다.

### 9. 오버레이 신뢰 경계를 로컬/비상호작용으로 고정한다

오버레이 구현 요구사항:

- always-on-top + click-through non-interactive
- Maekon 로컬 프로세스에서만 렌더링
- 운영 추적을 위한 session/candidate marker 표시
- timeout/cancel/completion 시 즉시 clear

오버레이 구현은 `src-tauri`의 `MagicOverlayDriver`(WebView 브릿지; ADR-004 Tauri v2 마이그레이션으로 원래 계획된 `oneshim-ui` 네이티브 오버레이 대체)에 둔다. port는 유지되며 호출자는 `oneshim-core`의 `OverlayDriver` trait를 사용한다.

### 10. GUI 세션 전용 SSE 스트림을 사용한다

V2의 기본 이벤트 전달은 전용 세션 SSE를 사용한다.

- `GET /api/automation/gui/sessions/:id/events`
- 세션 범위 이벤트만 전달 (`gui_session.proposed`, `gui_session.highlighted`, `gui_session.executed`, `gui_session.expired` 등)

기존 `GET /api/stream`은 운영 요약 이벤트를 보조적으로 발행할 수 있으나, GUI 세션 상태의 단일 진실 소스로 사용하지 않는다.

---

## 목표 책임 분리

| Crate | ADR-002 이후 책임 |
|------|-------------------|
| `oneshim-core` | focus/overlay/session/ticket 포트 및 도메인 계약 |
| `oneshim-automation` | `GuiInteractionService` 오케스트레이션(`propose -> highlight -> confirm -> execute`) + 정책/프라이버시/감사 + 세션 상태 |
| `oneshim-vision` | accessibility 어댑터 (macOS AX / Windows UIA / Linux AT-SPI), R-tree 공간 인덱스, `ElementFinder` 구현 |
| `oneshim-web` | 얇은 전송 핸들러, 검증, 세션 API, SSE 이벤트 발행 |
| `src-tauri` (pkg `oneshim-app`) | DI 와이어링 + `MagicOverlayDriver` (WebView 오버레이 브릿지; ADR-004 에 따라 원래 계획된 `oneshim-ui` 네이티브 오버레이 대체) — `OverlayDriver`, `FocusProbe`, `ElementFinder`, `InputDriver` 연결 |

의존성 방향은 유지된다. 어댑터 간 통신은 `oneshim-core` port를 통해서만 수행한다.

---

## API 계약 (제안 V2)

기본 경로: `/api/automation/gui`

| Method | Path | 목적 |
|-------|------|------|
| `POST` | `/sessions` | 포커스 scene 기반 제안 세션 생성 |
| `POST` | `/sessions/:id/highlight` | OS 오버레이 하이라이트 렌더 |
| `POST` | `/sessions/:id/confirm` | 후보 확인 및 서명된 실행 티켓 발급 |
| `POST` | `/sessions/:id/execute` | 티켓 기반 실행(atomic 재검증 필수) |
| `GET` | `/sessions/:id` | 세션 상태/후보 요약 조회 |
| `DELETE` | `/sessions/:id` | 오버레이 정리 및 세션 종료 |
| `GET` | `/sessions/:id/events` | 전용 세션 SSE 스트림(기본 GUI 이벤트 채널) |

인증 시맨틱:

- `POST /sessions` 응답으로 per-session capability token을 발급한다.
- 이후 `:id` 경로는 해당 토큰을 필수로 요구한다.
- `GET /sessions/:id/events`도 동일한 per-session capability token을 필수로 요구한다.
- `web.allow_external=false`일 때 loopback 외 요청은 거부한다.

기존 `/scene`, `/execute-scene-action`는 호환성과 내부 도구 목적의 레거시 경로로 유지한다.

---

## 런타임 시퀀스

```text
Web UI
  -> oneshim-web handler
  -> oneshim-automation GuiInteractionService
     -> FocusProbe.current_focus()
     -> ElementFinder.analyze_scene()
     -> 후보 정렬
  <- 후보 + session token

사용자 highlight 요청
  -> OverlayDriver.show_highlights()

사용자 candidate confirm
  -> 서명된 GuiExecutionTicket 발급
  -> FocusProbe.validate_execution_binding(ticket.binding)
  -> InputDriver 실행
  -> 검증 + 감사 기록
  -> OverlayDriver.clear_highlights()
```

---

## 보안/프라이버시 불변식

1. 명시적 정책/동의 오버라이드 없이 민감 원본 데이터를 외부로 내보내지 않는다.
2. UI 페이로드는 민감 컨텍스트에서 `text_masked`를 기본으로 한다.
3. 실행/차단/오버라이드/티켓 실패를 모두 감사 로그에 남긴다.
4. 실행은 유효한 세션 capability token + 유효한 서명 티켓을 모두 요구한다.
5. 포커스 재검증은 실행 시점에 필수이며 단일 probe 호출로 원자적으로 처리한다.
6. 오버레이는 로컬 전용, 비상호작용, 제한된 생명주기를 갖는다.
7. GUI 세션 SSE는 세션 스코프를 강제해 다른 세션 이벤트를 구독할 수 없어야 한다.

---

## 실패 시맨틱

권장 HTTP 매핑:

- `400` 요청 스키마 오류
- `401` 세션 capability token 누락/불일치
- `403` 정책/프라이버시 거부
- `409` 포커스 또는 scene drift
- `422` 후보/티켓 무효화
- `503` 실행 런타임 미가용(헤드리스/권한 부족)
- `503` GUI V2 설정 오류(GUI V2 활성인데 `ONESHIM_GUI_TICKET_HMAC_SECRET` 누락)

`409`/`422`에서는 새 세션을 생성해 `propose -> highlight -> confirm`을 다시 수행한다.

---

## 롤아웃 계획

### Phase 0 (계약 + 기본 상태)

- core 모델/port/schema 버전 추가
- 메모리 세션 저장소 + cleanup task 추가
- 미지원 환경 no-op 어댑터 추가

### Phase 1a (proposal-only preview)

- `POST /sessions`, `GET /sessions/:id`
- 오버레이/실행 미활성

### Phase 1b (highlight preview)

- `POST /sessions/:id/highlight`, `DELETE /sessions/:id`
- 오버레이 렌더링 경로 활성
- V2 실행은 여전히 미활성

### Phase 2 (confirmed execution)

- `POST /sessions/:id/confirm`, `POST /sessions/:id/execute`
- 서명 티켓 검증 + atomic 포커스 재검증
- V2 경로에서 정책/프라이버시/감사 전면 강제

### Phase 3 (hardening)

- OS별 접근성 어댑터(macOS AX, Windows UIA, Linux AT-SPI)
- 후보 정렬/재시도 힌트/보정 품질 지표 고도화

---

## 테스트 전략

- 세션 상태머신 전이 단위 테스트 (`propose/highlight/confirm/execute/cancel/expire`)
- 티켓 서명/검증/만료/nonce 재사용 방지 단위 테스트
- 포커스 드리프트/atomic 검증 결과 단위 테스트
- `MockOverlayDriver`, `MockFocusProbe`, `MockElementFinder`, `MockInputDriver` 통합 테스트
- capability-token 강제 및 에러 매핑(`401/403/409/422/503`) 웹 핸들러 테스트

---

## 결과

장점:

- 웹은 제어/모니터링 표면 역할을 유지한다.
- OS GUI 상호작용이 명시적이고 감사 가능한 안전한 흐름으로 바뀐다.
- Hexagonal 경계와 기존 의존성 방향을 유지한다.

트레이드오프:

- 오버레이 생명주기, 세션 TTL, capability/티켓 검증으로 런타임 복잡도 증가
- Phase 3의 플랫폼별 어댑터 구현 비용이 높다

---

## 관련 문서

- `docs/architecture/ADR-001-rust-client-architecture-patterns.ko.md`
- `docs/contracts/automation-event-contract.ko.md`
- `docs/crates/oneshim-web.ko.md`
- `docs/crates/oneshim-automation.ko.md`
