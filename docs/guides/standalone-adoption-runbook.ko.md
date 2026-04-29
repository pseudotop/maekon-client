[English](./standalone-adoption-runbook.md) | [한국어](./standalone-adoption-runbook.ko.md)

# Standalone 도입 런북

Maekon을 standalone 우선 모드로 운영하기 위한 실전 체크리스트입니다.

## Day 0 (설정)

1. `cargo run -p oneshim-app -- --offline` 실행.
2. `http://localhost:10090` 대시보드 접속.
3. Settings에서 다음을 기본값으로 유지:
- 샌드박스 활성화(`Standard` 또는 `Strict`),
- `external_data_policy`는 `PiiFilterStandard` 이상,
- `allow_unredacted_external_ocr=false`.

## Day 1-3 (베이스라인)

1. 필수 템플릿만 우선 활성화 (`daily-priority-sync`, `deep-work-start`).
2. KPI 기준선 수집:
- `success_rate`,
- `blocked_rate`,
- `p95_elapsed_ms`,
- `timing_samples`.

## Day 4-7 (통제 확장)

1. 템플릿 1개 추가 (`bug-triage-loop` 또는 `release-readiness`).
2. 차단 액션이 늘면:
- Automation 정책 카드 확인,
- `scene_action_override`는 사유/승인자/만료시각이 있는 경우만 사용.

## 주간 점검

1. Settings에서 metrics/events 내보내기.
2. Automation audit log에서 정책 거부 패턴 점검.
3. 차단률 상승 없이 속도 개선이 확인된 템플릿만 유지.

## 확산 배포 기준

- 1주일 이상 `success_rate` 추세 안정.
- 만료 없는 민감 오버라이드 장기 유지 금지.
- Replay scene overlay/액션 실행이 CI E2E에서 검증됨.
