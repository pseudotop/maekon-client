[English](./ADR-TEMPLATE.md) | [한국어](./ADR-TEMPLATE.ko.md)

# ADR-XXX: <간결한 제목>

**Status**: Draft | Proposed | Accepted | Superseded | Deprecated
**Date**: YYYY-MM-DD
**Scope**: `<crate-또는-모듈-경로>`, `<부가-범위>`, ...
**Supersedes**: (선택) ADR-NNN, 또는 `none`
**Superseded by**: (선택, 이 ADR 이 다른 ADR 로 대체된 경우만) ADR-MMM
**Related**: (선택) ADR-NNN, ADR-MMM
**Implementation**: (선택) 이 ADR 을 수행한 공개 implementation record 또는 코드 경로

---

## Context

이 ADR 이 다루는 아키텍처 문제. **왜 (why)** 에 집중 — 이 결정을 촉발한 force, 제약, 사건, 요구사항. 사실 기반으로 쓰고 과거 ADR / 설계 문서를 re-설명하지 말고 참조.

## Decision

이 ADR 이 고정하는 단일 아키텍처 결정 (혹은 긴밀히 연결된 소규모 결정 클러스터). 다부분 결정은 번호 있는 sub-section 으로:

### 1. <결정의 첫 부분>

규칙 + 짧은 예시/스니펫 (명확해질 때만).

```rust
// 예시 또는 canonical form (선택)
```

**Rationale**: 대안 대비 이 선택의 이유.

### 2. <두 번째 부분, 필요 시>

...

## Consequences

### Positive

- 이점 1
- 이점 2

### Negative

- 비용 / trade-off 1
- 비용 / trade-off 2

### Neutral

- 긍정도 부정도 아닌 관찰 가능한 효과.

## Alternatives Considered

**A. <대안 이름>.** 반려 이유.
**B. <대안 이름>.** 반려 이유.

## Known Follow-ups (선택)

이 ADR 범위 밖이지만 머지 후 의미 있어질 항목. 후속 iteration 이 다시 링크할 수 있게 번호 부여:

1. **<Follow-up 제목>** — 무엇, 왜, 대략적 공수. Design doc 이 있으면 링크.

## Related Docs

- `docs/...` — 보조 명세 또는 가이드
- `docs/architecture/ADR-NNN-*.md` — 관련 아키텍처 결정

---

## 이 템플릿 사용법

1. `ADR-TEMPLATE.md` + `ADR-TEMPLATE.ko.md` 를 `ADR-XXX-<kebab-title>.md` + `.ko.md` 로 복사. `XXX` 는 다음 미사용 세 자리 번호 (`docs/architecture/README.md` 의 레지스트리 참조).
2. 모든 필수 헤더 필드 채우기. `Status` 는 `Draft` 또는 `Proposed` 로 시작; 리뷰 및 (해당 시) 구현 완료 후에만 `Accepted` 로 승격.
3. `Context` 를 먼저 써서 리뷰어가 *왜* 를 *무엇* 보다 먼저 이해하도록.
4. `Decision` 섹션이 핵심. 다페이지 구현 상세를 쓰게 된다면 companion implementation record 로 옮기고, 해당 기록이 공개 문서라면 `**Implementation**:` 으로 링크.
5. `Consequences` 필수. `Alternatives Considered` 강권 — 최소 2개 대안.
6. 작성 후 ADR 이 `docs/architecture/README.md` 레지스트리에 등록되었는지 확인.
7. Decision 이 실질적으로 바뀌면 조용히 수정하지 말 것: 이 ADR 을 `Supersedes` 하는 새 ADR 을 만들거나, 명시적 `## Update YYYY-MM-DD` 섹션을 append.

## 네이밍 컨벤션

- **파일명**: `ADR-XXX-<kebab-case-title>.md`. 한국어 companion: `ADR-XXX-<kebab-case-title>.ko.md`.
- **제목**: 간결 (≤ 60자). 문장 형태는 title 이 아니라 Context/Decision 본문에.
- **Status 키워드**: 가능한 단일 단어. Promotion 이력 (`Accepted (promoted from Proposed YYYY-MM-DD; <이유>)`) 허용.
- **Date**: ADR 최초 저작일. 후속 수정 시 변경하지 말 것; 실질적 변경은 `## Update YYYY-MM-DD` 하위 섹션 사용.
