[English](./ADR-004-tauri-v2-migration.md) | [한국어](./ADR-004-tauri-v2-migration.ko.md)

# ADR-004: Tauri v2 마이그레이션 (iced → Tauri v2 + WebView)

**날짜**: 2026-03-04
**상태**: Accepted
**결정자**: ONESHIM 팀

## 배경

oneshim-ui 크레이트는 iced 0.12 기반 GUI를 구현했다. 다음 문제가 발생했다:

1. **렌더링 한계** — iced의 즉시 모드 렌더러는 복잡한 데이터 시각화(타임라인, 히트맵)에서 성능 저하
2. **웹 대시보드 중복** — Axum + React로 이미 동일 기능의 웹 UI 존재. 두 UI 유지 비용 증가
3. **플랫폼 일관성** — macOS/Windows/Linux 각각 다른 iced 렌더러 동작

## 결정

iced를 제거하고 Tauri v2를 사용하여 기존 React 웹 대시보드를 데스크탑 셸로 감싼다.

## 구현

- `src-tauri/` 디렉토리: Tauri 메인 바이너리
- 기존 `crates/oneshim-web/` React 앱을 Tauri WebView로 임베드
- IPC: `tauri::command` 매크로로 Rust ↔ JavaScript 통신
- System tray: `tauri::tray` API
- 자동 업데이트: **`self_update`** (유지). 커스텀 업데이터(`src-tauri/src/updater/`)가 SHA256 + Ed25519 검증, 롤백, 프리릴리스 필터링, 버전 플로어, 코디네이터 상태머신을 제공 — `tauri-plugin-updater`로는 이 기능들을 커버할 수 없음. 마이그레이션 평가 후 기각 (2026-04-01)

## 결과

- ✅ 단일 UI 코드베이스 (React)
- ✅ 크로스 플랫폼 일관성 (WebKit/WebView2)
- ✅ oneshim-ui 크레이트 제거로 의존성 감소
- ⚠️ Tauri IPC 학습 비용
- ⚠️ WebView 메모리 오버헤드 (~50MB)

## 대안 검토

| 대안 | 이유 기각 |
|------|----------|
| iced 유지 | 복잡 UI 한계, 두 UI 유지 비용 |
| Egui | iced와 동일한 한계 |
| Electron | 메모리/번들 크기 과다 |
