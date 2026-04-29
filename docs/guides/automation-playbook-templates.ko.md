[English](./automation-playbook-templates.md) | [한국어](./automation-playbook-templates.ko.md)

# 자동화 플레이북 템플릿

이 문서는 내장 워크플로우 프리셋을 실무 시나리오에 바로 적용하는 방법을 정리합니다.

## 사용 방법

1. 로컬 대시보드에서 `/automation` 페이지를 엽니다.
2. `Workflow` 카테고리를 선택합니다.
3. 내장 프리셋을 실행하고 Audit Log/KPI 카드 결과를 확인합니다.
4. 팀 환경에 맞게 사용자 정의 프리셋으로 확장합니다.

## 내장 템플릿 (권장 시작 순서)

| 프리셋 ID | 사용 시점 | 기대 효과 |
|---|---|---|
| `daily-priority-sync` | 업무 시작 시 | 캘린더/이슈/메신저 컨텍스트를 1분 내 정렬 |
| `bug-triage-loop` | 버그 큐 처리 시 | 트래커/모니터링/IDE 전환 비용 감소 |
| `customer-followup` | 고객 후속 대응 시 | CRM-문서-메일 흐름 표준화 |
| `release-readiness` | 릴리스 검증 전 | 저장 + 터미널 + 브라우저 루틴 고정 |
| `deep-work-start` | 집중 세션 시작 시 | 실행 중심 작업 화면으로 빠르게 전환 |

## 운영 가드레일

- 재현 가능한 정책 경계를 위해 샌드박스는 기본 활성화 상태를 유지합니다.
- `scene_action_override`는 만료 시각이 있는 예외 상황에만 사용합니다.
- Automation KPI 카드에서 `success_rate`, `blocked_rate`, `p95_elapsed_ms`를 지속 확인합니다.

## 팀 도입 팁

처음에는 반복 수작업이 명확한 템플릿 2~3개만 적용하고, 1주일 KPI 개선이 확인된 뒤 확장하세요.
