[English](./automation-playbook-templates.md) | [한국어](./automation-playbook-templates.ko.md)

# 자동화 플레이북 템플릿

이 문서는 내장 워크플로우 프리셋을 실무 시나리오에 바로 적용하는 방법을 정리합니다.

## 사용 방법

1. 로컬 대시보드에서 `/automation` 페이지를 엽니다.
2. `Workflow` 카테고리를 선택합니다.
3. 내장 프리셋을 실행하고 Audit Log/KPI 카드 결과를 확인합니다.
4. 팀 환경에 맞게 사용자 정의 프리셋으로 확장합니다.

## 내장 템플릿 (권장 시작 순서)

`*-sync` / `*-loop` / `*-followup` 프리셋은 **샘플**입니다. 지정된 앱들을 앞으로
가져오는 동작이며, 앱 목록은 시작점으로 삼아 실제로 사용하는 앱으로 편집하세요
("앞으로 가져오기"가 OS별로 무엇을 의미하는지는 아래 **플랫폼 차이** 참고).

| 프리셋 ID | 사용 시점 | 샘플 흐름 (환경에 맞게 편집) |
|---|---|---|
| `daily-priority-sync` | 업무 시작 시 | Calendar, Notion, Slack을 앞으로 가져오기 |
| `bug-triage-loop` | 버그 큐 처리 시 | Slack, Terminal, VS Code를 앞으로 가져오기 |
| `customer-followup` | 고객 후속 대응 시 | Calendar, Notion, Mail을 앞으로 가져오기 |
| `release-readiness` | 릴리스 검증 전 | 저장 후 Terminal과 브라우저를 앞으로 가져오기 |
| `deep-work-start` | 집중 세션 시작 시 | 실행 중심 작업 화면으로 전환 (앱 비종속) |

## 플랫폼 차이 (앱 활성화)

위 앱 전환 프리셋은 `ActivateApp` 스텝을 실행합니다. 그 동작은 OS마다 다르므로
앱과 사전 조건을 그에 맞게 선택하세요.

| 플랫폼 | 방식 | 동작 |
|---|---|---|
| macOS | `open -a "<name>"` | **앞으로 가져오며, 실행 중이 아니면 앱을 실행합니다.** |
| Windows | `WScript.Shell.AppActivate` | **이미 열린** 창을 앞으로 가져옵니다. 실행하지 **않으므로** 앱을 먼저 켜세요. |
| Linux | `wmctrl -a` / `xdotool` | **이미 열린** 창을 활성화합니다. 실행하지 **않으므로** 앱을 먼저 켜세요. `wmctrl` 또는 `xdotool` 설치 필요. |

실무 가이드:

- **macOS**에서는 지정 앱이 설치되어 있어야 합니다. 실제 앱과 매칭되지 않는
  이름(예: 일반 라벨)은 non-zero로 종료되며, 내장 스텝은 `stop_on_failure`이므로
  해당 스텝에서 프리셋 전체가 중단됩니다.
- **Windows / Linux**에서는 원하는 앱을 먼저 열거나(또는 항상 켜두는 앱을
  참조하도록 프리셋을 편집), 이 프리셋은 실행이 아닌 포커스 전환만 수행합니다.
- 각 `ActivateApp` 셸아웃은 짧은 타임아웃으로 제한됩니다. 활성화/실행이 멈추면
  워크플로를 매다는 대신 해당 스텝이 즉시 실패합니다.

## 운영 가드레일

- 재현 가능한 정책 경계를 위해 샌드박스는 기본 활성화 상태를 유지합니다.
- `scene_action_override`는 만료 시각이 있는 예외 상황에만 사용합니다.
- Automation KPI 카드에서 `success_rate`, `blocked_rate`, `p95_elapsed_ms`를 지속 확인합니다.

## 팀 도입 팁

처음에는 반복 수작업이 명확한 템플릿 2~3개만 적용하고, 1주일 KPI 개선이 확인된 뒤 확장하세요.
