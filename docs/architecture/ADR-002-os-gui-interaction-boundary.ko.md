[English](./ADR-002-os-gui-interaction-boundary.md) | [한국어](./ADR-002-os-gui-interaction-boundary.ko.md)

# ADR-002: OS GUI 상호작용 경계와 런타임 분리

**상태**: 채택됨 (Accepted) — 2026-04-20 제안됨 → 채택됨 승격. M3 네이티브 어댑터 구현 완료.
**날짜**: 2026-02-25
**범위**: `maekon-core` (`ElementFinder` / `OverlayDriver` / `FocusProbe` / `InputDriver` 포트), `maekon-automation` (세션 상태 머신, capability/ticket 처리), `maekon-vision` (R-tree 공간 인덱스, app-specific element-type override, accessibility 어댑터), `maekon-web` (capability-token 핸들러), `src-tauri` (`MagicOverlayDriver` — ADR-004 Tauri v2 마이그레이션으로 원래 계획된 `maekon-ui` 네이티브 오버레이 대신 WebView 브릿지 채택)

---

## 컨텍스트

현재 스택은 이미 다음을 지원한다.

- Scene 분석 (`GET /api/automation/scene`)
- Scene 액션 실행 (`POST /api/automation/execute-scene-action`)
- `maekon-automation`의 정책/프라이버시/감사 제어

하지만 OS GUI 상호작용에는 더 강한 보장이 필요하다.

1. 현재 포커스된 네이티브 창 기준으로 컨트롤을 식별해야 한다.
2. 액션 전에 OS 화면 위에 명시적 하이라이트를 보여줘야 한다.
3. 사용자 확인 이후에만 실행해야 한다.

웹 렌더링만으로는 임의 네이티브 창 위 신뢰 가능한 오버레이와 실행 시점 포커스 일관성을 보장하기 어렵다.

---

## 결정 사항

### 1. Control Plane / Execution Plane을 분리한다

- **Control Plane** (`maekon-web`): 관리, 모니터링, API 오케스트레이션
- **Execution Plane** (로컬 런타임): 포커스 조회, scene 분석, 네이티브 오버레이 하이라이트, 입력 실행

`maekon-web`은 OS 네이티브 상호작용을 직접 호출하지 않는다.

### 2. 정책/프라이버시/감사는 자동화 계층 단일 관문을 유지한다

모든 GUI 실행 경로는 `maekon-automation` 정책/프라이버시/감사 체크를 통과해야 한다. 핸들러에서 드라이버로 우회 호출을 금지한다.

### 3. 세션 기반 상호작용 프로토콜을 채택한다

흐름:

1. 후보 `propose`
2. 후보 `highlight`
3. 후보 `confirm`
4. 짧은 수명의 티켓으로 `execute`
5. `verify` + `audit`

원샷 직접 실행 경로는 호환성을 위해 유지하지만, 고위험 UX의 기본 경로로 사용하지 않는다.

### 4. 포커스/오버레이 코어 계약을 명시한다

`maekon-core`에 포트를 추가한다.

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

V2 세션 상태는 `maekon-automation` 메모리에 저장한다.

- `Arc<RwLock<HashMap<SessionId, GuiInteractionSession>>>`
- TTL 기반 생명주기 + 주기적 정리(기본 30초)
- Phase 0-2에서는 SQLite 영속화를 하지 않는다

향후 영속화가 필요하면 `maekon-core` storage port를 통해 도입한다.

### 7. 티켓 무결성과 세션 capability 인증을 강제한다

`GuiExecutionTicket` 필드:

- `session_id`, `focus_hash`, `scene_id`, `element_id`, `action_hash`
- `issued_at`, `expires_at`, `nonce`
- `signature` (HMAC)

HMAC 키(고정 환경설정):

- GUI V2 엔드포인트가 활성화된 경우 `MAEKON_GUI_TICKET_HMAC_SECRET` 환경설정을 필수로 사용한다.
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

오버레이 구현은 `src-tauri`의 `MagicOverlayDriver`(WebView 브릿지; ADR-004 Tauri v2 마이그레이션으로 원래 계획된 `maekon-ui` 네이티브 오버레이 대체)에 둔다. port는 유지되며 호출자는 `maekon-core`의 `OverlayDriver` trait를 사용한다.

### 10. GUI 세션 전용 SSE 스트림을 사용한다

V2의 기본 이벤트 전달은 전용 세션 SSE를 사용한다.

- `GET /api/automation/gui/sessions/:id/events`
- 세션 범위 이벤트만 전달 (`gui_session.proposed`, `gui_session.highlighted`, `gui_session.executed`, `gui_session.expired` 등)

기존 `GET /api/stream`은 운영 요약 이벤트를 보조적으로 발행할 수 있으나, GUI 세션 상태의 단일 진실 소스로 사용하지 않는다.

---

## 목표 책임 분리

| Crate | ADR-002 이후 책임 |
|------|-------------------|
| `maekon-core` | focus/overlay/session/ticket 포트 및 도메인 계약 |
| `maekon-automation` | `GuiInteractionService` 오케스트레이션(`propose -> highlight -> confirm -> execute`) + 정책/프라이버시/감사 + 세션 상태 |
| `maekon-vision` | accessibility 어댑터 (macOS AX / Windows UIA / Linux AT-SPI), R-tree 공간 인덱스, `ElementFinder` 구현 |
| `maekon-web` | 얇은 전송 핸들러, 검증, 세션 API, SSE 이벤트 발행 |
| `src-tauri` (pkg `maekon-app`) | DI 와이어링 + `MagicOverlayDriver` (WebView 오버레이 브릿지; ADR-004 에 따라 원래 계획된 `maekon-ui` 네이티브 오버레이 대체) — `OverlayDriver`, `FocusProbe`, `ElementFinder`, `InputDriver` 연결 |

의존성 방향은 유지된다. 어댑터 간 통신은 `maekon-core` port를 통해서만 수행한다.

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
  -> maekon-web handler
  -> maekon-automation GuiInteractionService
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
- `503` GUI V2 설정 오류(GUI V2 활성인데 `MAEKON_GUI_TICKET_HMAC_SECRET` 누락)

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
- `docs/crates/maekon-web.ko.md`
- `docs/crates/maekon-automation.ko.md`


## Update 2026-09-20 — Guard를 통과하는 비동기 후보 판단

### 맥락과 결정

구조화 원격 모델은 닫힌 후보 집합을 정렬할 수 있지만 결과가 사용자 확인이나 실행
capability는 아니다. 이 ADR의 기존 proposal 단계를 확장하고 평행 장면 모델이나 새 실행
경로를 만들지 않는다. 이 dated amendment는 정상 PR merge로 발효한다. 구현과 provider/
데이터 승인은 별도 gate다.

1. `maekon-core`가 async `CandidateDecisionPort`와 typed request/binding/candidate/outcome을
   소유한다. `maekon-network`는 HTTP adapter, `src-tauri`는 guard와 composition을 소유한다.
   automation은 core port에만 의존한다. 자유 생성 `LlmProvider`와 동기 로컬
   `WorkTypeClassifier`의 계약은 유지한다.
2. 기존 `GuiCandidate`/`UiSceneElement`에서 최소 opaque ID와 정제한 의미 설명을 projection한다.
   원래 goal hash, local candidate binding hash, scene/frame generation, consent/policy revision,
   monotonic deadline은 로컬에 둔다. 정제로 다른 목표가 같아질 수 있으므로 송신 payload hash가
   원래 binding을 대신하지 않는다.
3. 결과는 `selected`, `none`, `delegate`, `unavailable`이다. none은 적합한 후보 없음,
   unavailable은 정책/실행/오류, delegate는 별도 판단 요청이며 다른 provider를 자동 호출하지 않는다.
   selected는 현재 후보를 가리키며 capability를 부여하지 않는다.
4. 인식 confidence, Choice 상대 확률, 분포 confidence, 선택 후보의 Noul suitability를 분리한다.
   Noul confidence나 생성 rationale을 꾸며내지 않는다. Choice 다음 Noul은 실제 선택한 후보를
   평가하고 두 호출을 모두 계상하며 중간에 guard를 재검증한다. test 공개 전에 threshold를 동결한다.
5. 모든 요청과 cache hit 전에 현재 동의, 민감 앱/제외 정책, 최소 payload, endpoint, 자격,
   audit, 예산을 검증한다. off/local-only/승인 부재/audit 부재이면 외부 호출 0이다.
   기존 LLM decorator가 이 port를 자동 보호하지 않는다. composition은 raw network client가
   아니라 guard로 감싼 port만 노출한다.
6. HTTP await 동안 lock을 유지하지 않는다. 각 await 뒤 목표/후보/generation/policy·consent
   revision/monotonic 만료를 다시 읽고 다르면 폐기한다. 허용된 in-flight 요청 뒤 철회는 새 호출과
   결과 재사용을 막는다. best-effort 취소로 이미 발생한 호출/비용을 지우지 않는다.
7. wire DTO에는 좌표/rect, 실제 입력할 본문, raw screenshot/AX, ticket/nonce/signature,
   capability/API key/임의 metadata가 없다. goal·라벨·제목도 정제한다. 자격은 승인된 인증
   헤더에만 두며 payload/raw provider 오류/비밀이 든 Debug를 로그에 남기지 않는다.
   구조 검증 통과는 정책 허가 증거가 아니다.
8. principal/provider/endpoint/model/rubric/schema, 허가 revision, input/candidate hash와 local
   binding으로 cache를 분리한다. 계정 간 또는 만료 snapshot의 결과를 재사용하지 않는다.
   해시는 결속이지 익명화가 아니다. cache hit에서도 현재 동의를 재검증한다.
9. 중복/누락/추가 ID, 후보 밖 선택, 비유한 값, 잘못된 분포, Choice/argmax 불일치, 관측 모델
   불일치, 잘못된 usage와 과대 응답을 거부한다. 요청/응답 bytes, timeout, retry 횟수, 총예산을
   제한하고 숨겨진 재시도 없이 모든 시도를 기록한다.
10. 추천 뒤에도 highlight → confirm → 서명 ticket → 최신 focus/policy 검증 → execute를 유지한다.
    이 port는 deny를 약화하거나 사용자를 대신해 확인하거나 ticket/미검증 Skill을 활성화할 수 없다.
    초기 runtime composition은 사용자 추천/실행 경로에 대해 비활성이다.

### 대안과 결과

생성 의도 port에 후보 선택을 넣으면 출력/권한 의미가 섞인다. automation의 직접 HTTP는 runtime
보호를 우회한다. 평행 장면 모델은 기존 identity/최신성 소유권을 복제한다. 별도 core port와
보호 adapter는 DTO/오류 분기·시험을 늘리지만 provider 변경을 automation 밖에 두고 기존 실행
경계를 유지한다. proposal 단계 확장이므로 새 ADR 번호는 필요하지 않다.

### 구현과 검증 경계

예정 파일은 `crates/maekon-core/src/models/candidate_decision.rs`,
`crates/maekon-core/src/ports/candidate_decision.rs`,
`crates/maekon-network/src/candidate_decision_client.rs`,
`src-tauri/src/provider_adapters/guarded_candidate_decision.rs` 및 module/composition 선언이다.
이는 예정 경로이며 구현된 동작을 의미하지 않는다.

off/local-only/거부 시 호출 수, 정제 wire capture, 응답 검증, cache 격리, 각 await 도중의
철회/목표·후보·generation·TTL 변경 및 2차 적합성 호출 차단을 시험한다. 양성 대조와 guard
제거 시 red가 되는 반증을 둔다. mock은 provider 성능이나 설치본 수락이 아니다. 실제 평가와
데이터/예산 승인이 사용자 노출보다 앞서며, 실행에는 설치본 최신성·명시적 확인 증거가 계속 필요하다.

## Update 2026-09-21 — 선택형 후보 판단 공급자

Jev 호환은 Maekon 사용에 Jev 계정이나 유료 추론을 필수로 만들지 않는다.
core 적격성 정책은
[candidate_decision_policy.rs](../../crates/maekon-core/src/models/candidate_decision_policy.rs)에 둔다.
상세 평가 계약은 내부 ONESHIM 저장소의
`docs/development/ai-evaluation/candidate-decision-providers.md`다.
이 amendment는 proposal 단계를 확장하며 정상 PR merge로 발효한다.

1. core는 기본 Off인 순수 provider/mode/cost 적격성을 소유한다. 로컬 규칙, 검증된 loopback
   모델, 사용자 소유 공식 Codex/Claude CLI, Jev direct, Jev Gateway는 별도 경로다.
   subprocess도 클라우드로 전송할 수 있으므로 local-only 예외가 아니다.
   암묵적인 다른 공급자·유료 fallback은 없다.
2. CLI 인증 성공은 구독 billing 증거가 아니다. 최초 구독 경로는 확인된 구독 포함 관측과
   사용자 요청을 요구한다. API-key billing, 비용 미확인, background CLI는 거부한다.
   공식 CLI의 인증 방법을 변경하거나 중계하지 않는다.
3. 프로모션은 만료가 있는 계정·endpoint·model 결속 가격 관측이며 영구 무료가 아니다.
   만료 뒤에는 유료 API 허용 정책에서도 새 호출을 거부한다. 미관측 usage/cost는 unknown이다.
4. Jev v1의 엄격한 Choice/Noul 의미를 보존한다. 후속 결과 계약은 Jev 분포, 모델 자기 보고,
   로컬 휴리스틱, 미관측 증거를 분리한다. 다른 공급자의 결과에 Jev 확률을 만들어 넣지 않는다.
5. 적격성은 전송·실행 권한이 아니다. 모든 adapter는 consent/privacy/audit/budget,
   원 binding/deadline, 철회, cache 격리 검증을 유지한다. CLI adapter는 추가로 stream/file
   상한·취소와 판단 전용 도구/MCP/plugin 제한 capability를 검증한다.
6. 공급자 부재 때 수동 선택·확인 경로를 유지한다. 모델은 보류할 수 있고, 로컬 규칙이
   모델 품질과 같다고 주장하지 않는다. Selected는 ticket·확인·Skill 활성화·실행 권한이 아니다.

#12455는 이 계약과 순수 정책을 구현한다. #12456은 공통 결과와 로컬 runtime,
#12457은 제한된 구독 CLI 판단, #12458은 선택형 Gateway transport와 가격을 소유한다.
#12423은 공급자별 독립 동결 평가, #12424/#12425는 GUI·설치본 수락 gate를 유지한다.
기존 #12176 동의 소유권과 모든 실행 guard는 계속 적용한다. 정책 단위 테스트와 38개
설계 사례는 runtime·모델 품질·설치본 수락 증거가 아니다.


### 공급자 중립 결과와 로컬 runtime (#12456)

새 CandidateAssessmentPort는 기존 GUI 요청과 binding을 재사용한다.
Selected에는 후보 ID만 들어간다. 태그 있는 증거는 로컬 exact-label 규칙,
모델의 categorical 응답, 기존 Jev 분포를 구분한다. v1 호환 변환은 provider/model/rubric,
Choice confidence·확률, 선택 후보 suitability, 원 attempt ID·비용, cache source,
decision ID와 binding을 명시적으로 보존한다. 관측하지 못한 usage는 unknown으로 남긴다.

공급자를 먼저 고른 후 필요한 HTTP 구성이나 secret 조회만 수행한다. Off는 실행 불가
추천 결과를 반환한다. 명시한 LocalRules는 전역 AI 공급자와 독립적이며, 양끝 공백과
대소문자를 정규화한 목표·라벨의 유일한 정확 일치만 선택한다. 일치 없음·동명·마스킹 충돌은
보류하며 후보 순서나 recognition confidence로 동률을 깨지 않는다. 미지원 경로에서 유료
fallback을 하지 않는다. 기존 종량제 Jev v1 경로에도 ExplicitPaidApiAllowed가 필요하다.

로컬 runtime은 승인 없이 시작한다. 신뢰된 composition이 runtime당 한 번 만료·횟수 제한
승인을 제공하고 최신 snapshot을 bind해야 한다. 재시작은 승인을 복원하지 않는다.
A→B→A를 포함해 bind마다 epoch가 증가한다. 계정·모델·정책 변경은 영구 철회와 새 runtime을
요구한다. 최초의 전체 consent/privacy revision을 고정하여 변경된 동의가 기존 승인을
갱신하지 못한다. 로컬 데이터 권한은 full-text 동의와 민감/제외 화면 검사를 재사용하되
공급자별 원격 gate와 독립적으로 판단한다. 이 판단 자체는 새 OCR 작업을 시작하지 않는다.

시도 전과 비동기 관측·transport·audit 후에 현재 권한, 활성 창, 원 monotonic deadline을
재검증한다. 불변 runtime clock anchor로 원 wall-clock 만료도 검사하여 절전 후 rebind나
cache가 기한을 갱신하지 못하며, wall-clock 역행은 runtime을 영구 철회한다. Rust Instant만으로는
절전 중 시간 경과가 보장되지 않는다(https://doc.rust-lang.org/std/time/struct.Instant.html).
quota 예약은 원자적이며 취소나 감사 실패로 환급하지 않는다. 최종 publication은
bind/revoke lock 안에서 재검사한다. 규칙 캐시도 현재 권한과 durable audit를 요구하고,
원 deadline과 source decision을 보존하며 원 시도를 중복 기록하지 않는다. 모델 응답은
캐시하지 않는다. 이는 추천 검사다. consumer는 확인·dispatch 때 현재 scene과 실행/동의
gate를 다시 검증해야 한다(#12424/#12425, 기존 #12176 소유권 유지).

선택형 Ollama adapter는 loopback/localhost origin만 허용하고 모든 해석 주소를 고정한다.
proxy·redirect·retry를 금지하고 응답 크기를 제한한다. 정제된 후보 text와 요청별 ID만
폐쇄된 categorical schema로 보내며 tools를 제공하지 않는다. 모델 다운로드·daemon 시작·
원격 fallback을 하지 않는다. 신뢰된 daemon/configuration 승인은 정규 endpoint·model
digest·만료와 결속한다. 추론 전후 cloud 비활성 상태와 설치된 비원격 weights를 확인하며,
지원 여부가 불명확하거나 관측할 수 없으면 호출을 거부한다. 설치 tag의 접두사 없는
SHA-256 hex digest는 신뢰한 승인의 정규 `sha256:` digest와 대조한다.

loopback이나 이 probe만으로 임의 daemon의 신원 또는 설정 TOCTOU 부재가 증명되지는 않는다.
승인 reference는 신뢰된 daemon/configuration 검증에서 나와야 하며 사용자 IPC의 자기
주장이 될 수 없다. 여기서는 production 승인 발급자나 GUI consumer를 활성화하지 않으며,
그 live 검증은 #12424/#12425에 남긴다. fixture 테스트는 실제 모델 품질·가격·계정 사용권·
설치본 수락을 증명하지 않는다(#12423). CLI와 Gateway adapter는 #12457/#12458에 남는다.


### Gateway 증거 타입과 정확한 가격 계산 (#12458, 첫 단계)

core는 Gateway 계정·funding·routing·보고 비용·감사 증거를 별도 타입으로 정의하고,
신뢰된 계정 관측·후보 평가 전송·영속 감사용 port를 둔다. TypeSafe-compatible endpoint와
alias는 직접 Jev의 고정 모델과 구분한다. 관측하지 못한 비용과 confidence는 unknown이다.

USD는 소수 12자리까지 정수 picoUSD로 계산한다. 입력·출력 token 요금과 고정 요청 수수료를
모두 포함하며, 방향별 65,536 token 초과와 u64 금액 범위 초과를 거부한다. 예약액은 수수료를
포함한 허용 최대 token 사용량으로 계산한다.

이번 단계는 타입과 가격 계산만 제공한다. 증거 검증·계정 승인·HTTP 전송·영속 감사 구현·
runtime 연결은 #12458에 남는다. 일반 factory는 여전히 Gateway를 거부하며 이번 변경으로
계정 observer·키 조회·모델 호출·GUI consumer를 활성화하지 않는다. 공개 프로모션만으로
계정 사용권이나 실효 비용 0을 증명하지 않는다.
