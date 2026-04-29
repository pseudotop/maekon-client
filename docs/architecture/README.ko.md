[English](./README.md) | [한국어](./README.ko.md)

# Architecture Decision Records (ADR) 레지스트리

Architecture Decision Records 는 `client-rust` 워크스페이스의 단일 아키텍처 결정을 기록합니다. *무엇* 을 결정했고 *왜* 했는지의 authoritative 레코드입니다. 구현 상세는 companion implementation record 에 두며 ADR 자체에는 두지 않습니다.

## 새 ADR 작성

1. [`ADR-TEMPLATE.md`](./ADR-TEMPLATE.md) (또는 한국어 companion `ADR-TEMPLATE.ko.md`) 읽기.
2. 아래 레지스트리의 다음 미사용 ID 로 `ADR-XXX-<kebab-case-title>.md` 복사.
3. 모든 필수 헤더 + Context / Decision / Consequences / Alternatives 섹션 채우기.
4. PR open; `Status` 는 `Draft` 또는 `Proposed` 로 시작.
5. 승인 후 `Accepted` 로 변경. 동시 구현 시 `Implementation` 필드에 코드 포인터 기재.
6. 아래 레지스트리 테이블에 등록.

## 레지스트리

| ID | 제목 | Status | Scope |
|----|------|--------|-------|
| [001](./ADR-001-rust-client-architecture-patterns.md) | Rust Client Architecture Patterns | Accepted | 전체 워크스페이스 |
| [002](./ADR-002-os-gui-interaction-boundary.md) | OS GUI Interaction Boundary and Runtime Split | Accepted | core / automation / vision / web / src-tauri |
| [003](./ADR-003-directory-module-pattern.md) | Directory Module Pattern for Large Source Files | Accepted | 모든 crate |
| [004](./ADR-004-tauri-v2-migration.md) | Tauri v2 Migration (iced → Tauri v2 + WebView) | Accepted | Desktop shell |
| [005](./ADR-005-tauri-governance.md) | Tauri v2 Governance | Accepted | `src-tauri/tauri.conf.json`, permissions |
| [006](./ADR-006-ipc-command-contract.md) | Tauri IPC Command Contract | Accepted | `src-tauri/src/commands/` |
| [007](./ADR-007-async-runtime-safety-patterns.md) | Async Runtime Safety Patterns | Accepted | tokio 사용 전체 crate |
| [008](./ADR-008-network-resilience-patterns.md) | Network Resilience Patterns | Accepted | `oneshim-network` |
| [009](./ADR-009-client-architecture-baseline.md) | Client Architecture Baseline | Accepted | `oneshim-app` 패키지, web, integration runtime |
| [010](./ADR-010-local-integration-harness-boundary.md) | Local Integration Harness Boundary | Accepted | Integration harness |
| [011](./ADR-011-standalone-analysis-pipeline.md) | Standalone Analysis Pipeline | Accepted | `oneshim-analysis`, AnalysisProvider 포트 |
| [012](./ADR-012-adaptive-tiered-memory.md) | Adaptive Tiered Memory | Accepted | Adaptive trigger, regime manager |
| [013](./ADR-013-llm-summary-vector-rag.md) | LLM Segment Summary + Vector RAG | Accepted | Embedding pipeline, vector store |
| [014](./ADR-014-tauri-managed-state-boundary.md) | Tauri Managed State Boundary | Accepted | `src-tauri` managed state |
| [015](./ADR-015-frame-storage-port.md) | Frame Storage Port Abstraction | Accepted | core 포트, storage 어댑터 |
| [016](./ADR-016-config-change-bus.md) | Config Change Bus | Accepted | `ConfigManager`, runtime 구독자 |
| [017](./ADR-017-feedback-signal-sink.md) | FeedbackSignalSink | Accepted | core 포트, suggestion, analysis |
| [018](./ADR-018-regime-manager-persistence.md) | RegimeManager Persistence | Accepted | core 포트, storage, analysis |
| [019](./ADR-019-error-code-infrastructure.md) | Error Code Infrastructure + AWS Bedrock Intentional Non-Support | Accepted | 전체 crate — wire-format error code |

**다음 가용 ID**: `ADR-020`.

## 컨벤션 요약

전체 authoritative 템플릿은 [`ADR-TEMPLATE.md`](./ADR-TEMPLATE.md) 참조. 핵심 규칙:

1. **파일명**: `ADR-XXX-<kebab-case-title>.md`; 한국어 companion 은 `.ko.md` 추가.
2. **헤더 필드** (순서): `Status`, `Date`, `Scope`, 선택 `Supersedes` / `Superseded by` / `Related` / `Implementation`.
3. **Status 키워드**: `Draft` → `Proposed` → `Accepted`. 종료 상태: `Superseded` (새 ADR 링크), `Deprecated` (대체 없음; rationale 필요).
4. **`Accepted` ADR 조용히 수정하지 말 것.** 실질적 변경은 새 ADR (`Supersedes`) 또는 `## Update YYYY-MM-DD` sub-section.
5. **최소 섹션**: Context, Decision, Consequences, Alternatives Considered.
6. **코드 예시**: canonical form 만 보이기, 전체 사용 예 나열 금지. 수백 줄 인라인 대신 `crates/.../path.rs:line` 링크.
7. **한국어 companion**: 공개 기여자가 읽을 가능성이 있는 ADR 은 필수. 운영 내부 ADR 은 팀이 영문 기본이면 생략 가능.

## Status 키워드 레퍼런스

| 키워드 | 의미 |
|-------|------|
| `Draft` | 저자가 여전히 iterate 중; 리뷰 대상 아님. |
| `Proposed` | 리뷰 준비; 변경 가능. |
| `Accepted` | 승인되어 효력 중. 이 영역의 새 결정은 준수하거나 supersede 해야 함. |
| `Superseded` | 이후 ADR 로 대체됨 (`Superseded by` 링크). 역사적 기록 유지. |
| `Deprecated` | 더 이상 효력 없지만 직접 대체 없음. rationale 를 본문에 설명. |

`Approved` 는 피할 것 — 과거 일부 ADR 에서 사용되었지만 `Accepted` 와 구분되는 일관된 의미 없음. ADR-019 drift audit (iter-186) 이 모두 `Accepted` 로 통일.

## 구현 기록과의 관계

- **`docs/architecture/`** (이 디렉토리) — 아키텍처 결정의 *무엇과 왜*.
- Companion implementation record — 특정 변경의 *어떻게*. 계획 artifact 성격의 기록은 public-minimal export 밖에 둘 수 있습니다.

새 ADR 이 구현이 필요할 때 일반적 흐름:

```
Draft ADR → Proposed ADR → companion implementation record
                       → 구현 PR 들
Accept ADR  (공개 implementation record + 코드 경로 가리키는 `Implementation:` 포인터 포함)
```
