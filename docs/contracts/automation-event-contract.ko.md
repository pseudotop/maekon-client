[English](./automation-event-contract.md) | [한국어](./automation-event-contract.ko.md)

# 자동화 이벤트 계약

이 문서는 자동화 scene/audit API의 버전드 페이로드 계약을 정의합니다.

## 계약 버전

- Audit 엔트리 페이로드: `automation.audit.v1`
- Scene 페이로드: `ui_scene.v1`
- Scene 액션 실행 페이로드: `automation.scene_action.v1`

## 대상 엔드포인트

- `GET /api/automation/contracts`
- `GET /api/automation/audit`
- `GET /api/automation/scene`
- `POST /api/automation/execute-scene-action`

## 호환성 규칙

1. 클라이언트는 `schema_version`이 존재하면 반드시 이를 기준으로 처리 분기해야 합니다.
2. 동일 버전 내 추가 필드(additive)는 하위 호환입니다.
3. 필드 의미/구조를 깨는 변경은 새 버전 문자열이 필요합니다.
4. 플랫폼 연동은 배포 전에 `GET /api/automation/contracts`를 확인해야 합니다.

## Scene 페이로드 필수 필드

- `schema_version`
- `scene_id`
- `captured_at`
- `screen_width`, `screen_height`
- `elements[]`의 `element_id`, `bbox_abs`, `bbox_norm`, `label`, `confidence`

## Audit 페이로드 필수 필드

- `schema_version`
- `entry_id`
- `timestamp`
- `command_id`
- `status`
- `elapsed_ms` (nullable)
