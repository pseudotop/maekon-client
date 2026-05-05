[English](./automation-event-contract.md) | [한국어](./automation-event-contract.ko.md)

# Automation Event Contract

This document defines versioned payload contracts for automation scene/audit APIs.

## Contract versions

- Audit entry payload: `automation.audit.v1`
- Scene payload: `ui_scene.v1`
- Scene action execution payload: `automation.scene_action.v1`

## Endpoints

- `GET /api/automation/contracts`
- `GET /api/automation/audit`
- `GET /api/automation/scene`
- `POST /api/automation/execute-scene-action`

## Compatibility rules

1. Clients MUST read and branch on `schema_version` when present.
2. New additive fields are backward compatible inside same version.
3. Breaking field changes require a new schema version string.
4. Platform integrations should monitor `GET /api/automation/contracts` before rollout.

## Scene payload required fields

- `schema_version`
- `scene_id`
- `captured_at`
- `screen_width`, `screen_height`
- `elements[]` with `element_id`, `bbox_abs`, `bbox_norm`, `label`, `confidence`

## Audit payload required fields

- `schema_version`
- `entry_id`
- `timestamp`
- `command_id`
- `status`
- `elapsed_ms` (nullable)
