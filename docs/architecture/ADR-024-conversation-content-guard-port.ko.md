[English](./ADR-024-conversation-content-guard-port.md) | [한국어](./ADR-024-conversation-content-guard-port.ko.md)

# ADR-024: 대화 콘텐츠 가드 포트(Conversation Content Guard Port)

**Status**: Accepted
**Date**: 2026-06-03
**Scope**: `src-tauri/src/provider_adapters/guarded_conversation.rs`, `src-tauri/src/provider_adapters/types.rs`, `src-tauri/src/session_manager/factory.rs`, `crates/maekon-core/src/ports/conversation_session.rs`
**Related**: ADR-001 (§6 의존성 방향, §7 포트 배치), ADR-021 (consent core 배치)
**Implementation**: `src-tauri/src/provider_adapters/guarded_conversation.rs`, `types.rs`의 `ConversationContentGuard` impl, `factory.rs`의 `SessionManagerImpl::decorate_session`

---

## Context

채팅(대화) 경로는 사용자 콘텐츠를 외부 provider(클라우드 HTTP API, provider 소유 CLI 서브프로세스)에 **PII 가드 없이** 전송하고 있었다. 기존 `GuardedLlmProvider`/`GuardedOcrProvider` 데코레이터는 `LlmProvider`/`OcrProvider` 트레잇(intent/스크린 컨텍스트/OCR)만 sanitize하는데, 이는 `ConversationSession`과 다른 트레잇 패밀리다. 채팅은 `ConversationSession::send_message`로 흐르며 `AuditingSession`(role 메타데이터만 로깅, 콘텐츠 무마스킹)으로만 래핑되어, provider 서브프로세스/HTTP 전송에 평문으로 도달했다.

이 갭(E21 리뷰 B1)은 진행 중 마이그레이션과 무관하게 모든 외부 채팅 세션에 대해 출시 코드에 이미 존재했다. Maekon은 데스크탑 모니터링 제품이므로, 마스킹되지 않은 사용자 콘텐츠의 off-device 전송은 중대한 프라이버시 결함이다.

수정 전 두 가지 설계 질문을 정해야 했다:

1. **가드 데코레이터를 어디에 두는가** — `maekon-core`(순수 포트) vs adapter crate?
2. **데코레이터가 가드에 어떻게 의존하는가** — 넓은 구체 타입 `ExternalOcrPrivacyGuard` vs 좁은 포트 추상화?

## Decision

### 1. `ConversationSession` 포트에 `is_external()` 추가

`ConversationSession`에 기본 메서드 `fn is_external(&self) -> bool { false }`를 추가한다(`LlmProvider::is_external`와 대칭). off-device로 데이터를 내보내는 어댑터는 `true`로 오버라이드한다: `ClaudeSubprocessSession`, `GenericSubprocessSession`(Codex/Gemini CLI), `HttpApiSession`(클라우드 — 단 localhost 엔드포인트인 Ollama는 **제외**). `LocalLlmSession`(Ollama)은 기본값 `false` 유지.

**Rationale**: 가드는 on-device/off-device 세션을 구분해 로컬 백엔드를 불필요하게 게이팅하지 않아야 한다. 기본 구현 메서드라 기존 impl 전부와 하위 호환된다.

### 2. 좁은 `ConversationContentGuard` 포트를 src-tauri-local로 도입

```rust
#[async_trait]
pub(crate) trait ConversationContentGuard: Send + Sync {
    /// 안전 전송을 보장할 수 없으면 Err(fail-closed)를 반환한다.
    async fn sanitize_outbound(&self, message: &SessionMessage)
        -> Result<SessionMessage, CoreError>;
}
```

이 포트는 `maekon-core`가 **아니라** `src-tauri/src/provider_adapters/guarded_conversation.rs`에 정의한다. `ExternalOcrPrivacyGuard`가 이를 구현한다(스크린/텍스트 LLM 경로와 동일한 fail-closed consent/active-window/sensitive-app 게이트 + `sanitize_title_with_level` 필터 재사용).

**Rationale**: ADR-001 §7에 따르면 2개 이상 crate가 소비하는 포트 트레잇은 `maekon-core`에 두어야 하고, 구현체와 유일 소비자가 모두 한 adapter crate에 있는 트레잇은 그 crate에 둔다. `ExternalOcrPrivacyGuard`(impl)와 `GuardedConversationSession`(소비자)이 모두 `src-tauri`에 있으므로 포트도 `src-tauri`에 둔다 — **core 오염이 아니다**. 좁은 단일 메서드 포트(Interface Segregation)는 데코레이터를 넓은 OCR/LLM 가드 표면에서 분리하고, 데코레이터의 분기 로직(통과/sanitize/fail-closed)을 손쉽게 mock 가능하게 한다 — 가장 가까운 구조적 형제인 `AuditingSession`→`AuditLogPort` 선례(역시 `ConversationSession` 데코레이터가 구체 타입이 아닌 포트에 의존)와 일치한다.

### 3. `GuardedConversationSession` 데코레이터는 adapter 레이어에

```rust
async fn send_message(&self, message: &SessionMessage) -> Result<ResponseStream, CoreError> {
    if !self.inner.is_external() {
        return self.inner.send_message(message).await; // 로컬: 통과
    }
    let sanitized = self.guard.sanitize_outbound(message).await?; // fail-closed
    self.inner.send_message(&sanitized).await
}
```

데코레이터는 `inner.is_external()`로 self-gate한다. 가드 에러는 inner 세션 호출 **전에** `?`로 전파된다 — 구조적으로 fail-closed다.

### 4. `SessionManagerImpl` DI 배선, audit 최외곽

`SessionManagerImpl`에 `Option<Arc<dyn ConversationContentGuard>>` 필드 추가(빌더 `with_privacy_guard`). `factory.rs::decorate_session`이 새로 생성된 모든 세션을 `AuditingSession(GuardedConversationSession(inner))`로 래핑한다 — audit 최외곽, 가드는 전송에 가장 가깝게. 가드는 `app_runtime_launch/session_wiring.rs`에서 `ExternalOcrPrivacyGuard`로 생성한다(active-window 게이트용 전용 `ProcessTracker`, egress 감사 추적용 공유 세션 `AuditLogger`).

**Rationale**: fail-safe defaults(가드 실패 시 차단)와 단일 egress 초크포인트는 확립된 AI-게이트웨이/DLP 모범사례와 일치한다.

## Consequences

### Positive

- 채팅 경로 PII 갭(E21 B1) 해소: 외부 채팅 콘텐츠가 전송 전 sanitize되고, 가드 실패 시 전송이 차단된다(fail-closed).
- 가드 로직을 서브프로세스 spawn 없이 단위 테스트 가능(데코레이터는 mock `ConversationContentGuard`, impl은 생성된 `ExternalOcrPrivacyGuard` + mock `ProcessMonitor`).
- `AuditingSession`의 포트 주입 선례와 일관; 기존 consent 게이트/PII 필터 재사용 — 중복 sanitization 로직 없음.
- egress 감사 추적(`privacy.external_llm.allowed`)이 채팅 경로까지 커버.

### Negative

- 외부 세션당 `Arc<dyn ConversationSession>` 간접 레이어 1개 추가.
- 코드베이스에 가드 의존 방식 2종 공존: LLM/OCR 가드는 구체 `ExternalOcrPrivacyGuard` 보유, 채팅 가드는 `ConversationContentGuard` 포트 의존. 의도된 것이나(Alternatives 참조) 후속 독자가 이해해야 할 분기점.
- `session_wiring`이 런타임 모니터를 공유하지 않고 가드 전용 `ProcessTracker`를 생성.

### Neutral

- 로컬(Ollama) 세션도 `decorate_session`을 거치지만 `is_external()` 게이트로 가드를 그대로 통과한다.

## Alternatives Considered

**A. 데코레이터+포트를 `maekon-core`에.** 거부: `ExternalOcrPrivacyGuard`는 `maekon_automation::AuditLogger`/`maekon_vision::PrivacyGateway`에 의존하므로, 가드/데코레이터 코드를 core에 두면 adapter→core 역의존이 생긴다(ADR-001 §6). 포트도 §7 "2개 이상 crate 소비" 기준 미달이라 core는 잘못된 위치.

**B. 구체 `ExternalOcrPrivacyGuard` 직접 의존(새 포트 없음).** 기존 LLM/OCR/Analysis 가드 3종과 일치하고 새 트레잇이 없다. 1차 선택지로는 거부: 데코레이터의 세 분기를 구체 가드로 테스트하려면 mock `ProcessMonitor`+consent 파일로 생성해야 하고(더 무겁다), 데코레이터가 OCR/LLM 전용 표면에 결합되며(ISP), 가장 가까운 형제 `AuditingSession`이 이미 구체 타입이 아닌 포트에 의존한다. 좁은 포트는 "미래 구현"이 아니라 현재의 테스트 용이성+레이어 계약으로 정당화된다.

## Known Follow-ups

1. **첨부 본문 sanitization** — `sanitize_outbound`은 현재 `content`와 free-text `context`를 마스킹한다. base64로 전송되는 텍스트형 파일 첨부는 아직 decode/sanitize되지 않음 → raw 첨부 본문을 커버하거나 제거하도록 후속 추적.
2. **매니저 경유 회귀 테스트** — create→guard→transmit 전체 체인 커버리지는 서브프로세스 spawn에 묶임 → 실제 CLI 없이 `SessionManagerImpl` 배선을 검증하는 fake-inner 통합 테스트 추가.

## Related Docs

- `docs/architecture/ADR-001-rust-client-architecture-patterns.md` — §6 의존성 방향, §7 포트 배치
- `docs/architecture/ADR-021-config-consent-core-placement.md` — consent/config core-배치 경계 예외
