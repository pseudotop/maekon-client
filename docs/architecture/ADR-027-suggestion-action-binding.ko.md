[English](./ADR-027-suggestion-action-binding.md) | [한국어](./ADR-027-suggestion-action-binding.ko.md)

# ADR-027: 제안 액션 바인딩 (파생 방식, 게이트 보존)

**상태**: 승인됨 (Accepted)
**일자**: 2026-07-08
**범위**: `crates/maekon-core/src/models/intent/workflow.rs` (파생 헬퍼 + 프리셋 id 상수), `src-tauri/src/commands/suggestions/` (DTO 보강 + `run_suggestion_action`), `crates/maekon-web/frontend/src/overlay/` (Run 어포던스)
**관련**: ADR-001 (§6 불변 크레이트 의존 방향, §7 포트 배치), ADR-002 (단일 게이트 실행 경계), ADR-017 (FeedbackSignalSink)
**이슈**: #7917. 독립 devils-advocate + tech-lead 각 2회전 3-loop 리뷰로 IMPORTANT 0 도달 후 확정.

---

## 배경

제안 파이프라인(프로듀서 → 중복제거 큐 → 오버레이 UI → 피드백/학습)과 게이트 실행 엔진(`ensure_enabled` → `confirmation_policy` → 샌드박스 스텝 → 해시체인 감사)은 완성돼 있었으나 단절된 사일로였다. 제안은 "지금 실행할까요?"를 제안할 수 없었고 오버레이는 "자동 작업 없음"을 명시적으로 광고했다. 이 브리지는 생산성 코치에서 게이트 에이전트로 가는 첫 단계다.

적대 리뷰에서 두 후보 아키텍처를 검토했다:

1. **영속 바인딩** — 도메인 `Suggestion`에 `action_preset_id: Option<String>` 필드를 추가하고 스토리지/동기화/DTO로 운반.
2. **파생 바인딩 (채택)** — 스키마 무변경; 제안이 이미 가진 데이터에서 순수 core 헬퍼가 표시/실행 시점에 파생.

## 결정

### 1. 바인딩은 파생되며, 와이어로 운반되지 않는다

`suggested_action_preset(suggestion_type, source) -> Option<&'static str>`가 단일 정책 테이블이다. MVP는 정확히 한 쌍만 매핑한다:

| (type, source) | 프리셋 |
|---|---|
| (`NeedFocusTime`, `RuleBased`) | `PRESET_DEEP_WORK_START` (`"deep-work-start"`, builtin) |
| 그 외 전부 — 네트워크(`LlmServer`)·LLM(`LlmLocal`) 소스 포함 | `None` |

영속 대신 파생을 택한 결정적 근거: REST SSE 경로는 도메인 `Suggestion`을 직접 역직렬화하므로 영속 필드는 **와이어 주입 가능** — 서버 페이로드가 실행 어포던스를 만들어낼 수 있고, 이를 막으려면 별도 sanitization 불변식이 필요해진다. 파생은 주입할 필드 자체가 없어 그 취약점 클래스를 **by-construction으로 제거**한다.

**source 조건은 하중을 받는 조건이다.** `NeedFocusTime`은 서버가 발행할 수 있으므로(gRPC + SSE가 동결된 10-variant enum 공유) type-only 파생은 네트워크 푸시 제안에도 원클릭 어포던스를 부여하게 된다(`automation.enabled` + 필드 기본 `Auto`에서는 프롬프트 없이 실행). LLM 소스도 제외한다: LLM 작성 콘텐츠 + 실행 어포던스 조합은 고정 프리셋이라도 프롬프트 주입에 인접한 표면이다.

### 2. 실행은 기존 게이트 체인만 경유한다

오버레이 Run 버튼은 composition-root Tauri 커맨드 `run_suggestion_action(suggestion_id)` 하나만 호출하며, 이 커맨드는:

1. 제안 id를 예약(reserve-then-execute; 동시 이중 호출 거부 — `Auto` 기본값에서 이중 발화는 두 번 실행되고 `deep-work-start`는 실행마다 창을 닫는다);
2. 제안을 해석(manager 큐 우선, storage 폴백 — pending 목록과 동일한 이중성);
3. 제안 자신의 `(type, source)`에서 바인딩을 **재파생** — 클라이언트는 프리셋 id를 절대 공급하지 않으며, 비-`RuleBased` 제안은 거부;
4. 라이브 목록(`builtin_presets()` + `AutomationConfig.custom_presets`)에서 프리셋을 해석 — 실행 시점 재검증이 진짜 dangling-id 불변식;
5. `AutomationPort::run_workflow(&preset)` 호출 — 기존 체인 전체: `ensure_enabled` → `confirmation_policy`(Block / Confirm-HITL + 30s 타임아웃→거부 / Auto) → 샌드박스 스텝 → 스텝별 감사 + 스토리지 해시체인;
6. 성공한 `PresetRunResult`에서만: 표준 accept 피드백(`submit_suggestion_feedback_to_runtime(…, "accept", None)` — 큐→히스토리, scorer, tally write-through, 서버 통지) + `acted_at` 기록. 거부/차단/실패/타임아웃 실행은 **아무것도 발행하지 않는다** — 학습된 관련도 신호를 비-실행으로 오염시키지 않는다.

UI 어포던스는 표시 전용이다: `SuggestionViewDto.action: Option<{ label }>` (label-only — 클라이언트는 프리셋 id를 보지도 않는다), 파생 매핑 ∧ `automation.enabled` ∧ 프리셋 해석 가능일 때만 단일 공유 헬퍼가 계산. 히스토리 뷰는 바인딩하지 않는다(오래된 제안은 실행을 제안해선 안 된다). 카피는 정책 중립("자동화 게이트를 통해 실행됩니다 — 확인 설정이 적용됩니다") — 필드 기본값이 `Auto`이므로 프롬프트를 약속하지 않는다.

## 동결 불변식 (위반 시 신규 ADR 필요)

| 불변식 | 위치 |
|---|---|
| 네트워크 소스 제안은 실행 어포던스를 얻지 않는다; 미래의 영속 바인딩은 네트워크 소스를 반드시 제거해야 한다 (`execute_command`의 미사용 서명 토큰 경로와 동일한 유보) | 파생 술어 + 본 ADR |
| `automation.enabled` 기본 `false`; 모든 실행은 `ensure_enabled` 경유 | `AutomationConfig` |
| 이중 기본값: `confirmation_policy` 필드 기본 `Auto`(D2-② 사인오프) vs `ConfirmationRequirement` enum / `ExecutionPolicy.confirmation` 기본 `Confirm`(fail-safe) — 둘 다 보존, 병합 금지 | `config/sections/privacy.rs`, `config/enums.rs` |
| record-replay 템플릿 `can_auto_execute() == false`; `require_signed_token` 기본 `true`; `min_llm_confidence` 0.65 하한 | `record_template.rs`, `policy/models.rs` |
| 감사: automation 로거 버퍼 + SHA-256 해시체인은 `maekon-storage`에서 계산/영속 (persistence callback 배선) | `audit_chain.rs`, `web_server_runtime.rs` |
| ADR-002 단일 게이트: handler-to-driver 우회 금지; 오버레이/웹뷰 직접 실행 금지 | ADR-002 |
| ADR-001 §6: `maekon-suggestion`과 `maekon-automation`은 sibling 유지 (core-only 의존); 공유 시멘틱은 `maekon-core`에 | CI `check-architecture-deps.sh` |
| `SuggestionType`은 10-variant proto 동결 유지; 브리지는 variant를 추가하지 않는다 | 가드 테스트 + proto |

## 진화 경로 (재리뷰 트리거)

- **최초의 비-type-파생 프로듀서**(인스턴스별/맥락적/LLM 작성 바인딩; custom 프리셋 타깃)가 영속 필드 결정을 재개한다 — 위의 네트워크 제거 불변식이 강제 요건.
- 서버 작성 바인딩은 서명 메커니즘(proto 필드 + 검증)을 요구하며 JSON 와이어로는 불가.
- 별도 `Executed` 피드백 outcome(현재 성공은 가장 강한 수용 신호인 `Accepted` 재사용)과 타임아웃-vs-사용자거부 텔레메트리는 후속.
- `deep-work-start` 프리셋 자체의 페르소나/행동 갭(개발자 중심 스텝 vs 커뮤니케이션 중심 `NeedFocusTime` 대상)은 후속 이슈로 추적; 바인딩 테이블은 프리셋 재설계와 함께 진화한다.
