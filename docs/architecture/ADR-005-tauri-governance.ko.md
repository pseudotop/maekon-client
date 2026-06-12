[English](./ADR-005-tauri-governance.md) | [한국어](./ADR-005-tauri-governance.ko.md)

# ADR-005: Tauri v2 거버넌스

**날짜**: 2026-03-08
**상태**: Accepted
**결정자**: MAEKON 팀
**관련**: [ADR-004: Tauri v2 마이그레이션](ADR-004-tauri-v2-migration.ko.md) | [ADR-006: IPC Command Contract](ADR-006-ipc-command-contract.ko.md) | [ADR-020: macOS Private API 정책](ADR-020-macos-private-api-policy.ko.md)

---

## 컨텍스트

MAEKON은 생산성 에이전트를 위한 크로스 플랫폼 데스크톱 셸이 필요하다. 셸은 다음 조건을 충족해야 한다.

1. 두 번째 UI 코드베이스 없이 기존 React 웹 대시보드를 렌더링한다.
2. 시스템 트레이, 데스크톱 알림, 자동 시작 같은 네이티브 시스템 통합을 제공한다.
3. 엔터프라이즈 배포에 적합한 작고 감사 가능한 바이너리를 생성한다.
4. macOS 공증 및 Windows 코드 서명을 지원한다.
5. Rust 백엔드와 JavaScript 프론트엔드 사이에 안전하고 감사 가능한 IPC 표면을 노출한다.

이전 GUI는 `iced` immediate-mode GUI 라이브러리(크레이트 `maekon-ui`)로 구현되었으며, ADR-004에서 교체되었다.

---

## 결정

**Tauri v2**를 데스크톱 셸로 사용한다.

- **UI 렌더러**: WKWebView(macOS), WebView2(Windows), WebKitGTK(Linux).
- **백엔드**: Rust(`src-tauri/`), 기존 Cargo 워크스페이스 공유.
- **IPC**: 강타입 Rust ↔ JavaScript 호출을 위한 `tauri::command` 매크로.
- **시스템 트레이**: `tauri::tray` API.
- **알림**: `tauri-plugin-notification`.
- **자동 업데이트**: semver 게이팅 + 서명 검증이 포함된 `tauri-plugin-updater`.

---

## 고려한 대안

| 대안 | 기각 이유 |
|------|----------|
| **iced 유지** | 복잡한 데이터 시각화(타임라인, 히트맵)에서 immediate-mode 렌더링 성능 저하. iced + React 두 UI 유지로 프론트엔드 비용 2배. |
| **Electron** | ~60 MB 바이너리 대비 Tauri 바이너리 ~5 MB. Electron은 자체 Chromium을 탑재해 공격 표면이 늘어나고 엔터프라이즈 바이너리 크기 기준 위반. |
| **Raw winit + wgpu** | WebView 통합 없음. 기존 React 대시보드를 재구현해야 함. |
| **egui** | iced와 동일한 한계 — immediate-mode, WebView 없음, 데이터 시각화 지원 제한. |

---

## 결과

### 장점

- 단일 UI 코드베이스(React). 기존 `crates/maekon-web/frontend/`를 수정 없이 재사용한다.
- 시스템 WebView(WKWebView / WebView2)를 통한 크로스 플랫폼 렌더링 일관성.
- 바이너리 크기 약 5 MB(Rust 바이너리) + WebView(시스템 제공). Chromium 번들 없음.
- 표준 Apple Developer ID 워크플로를 통한 macOS 공증 지원.
- 표준 Authenticode 워크플로를 통한 Windows 코드 서명 지원.
- Tauri의 CSP 적용으로 IPC 보안에 강력한 기본값 제공(ADR-006 참조).

### 단점 / 리스크

- **WKWebView 버전 의존성**: macOS 업데이트 시 WKWebView 동작 변경 가능. `tauri.conf.json`에서 최소 macOS 버전을 10.15로 설정. macOS 주요 업데이트 후 Tauri 릴리스 노트를 모니터링한다.
- **Windows WebView2**: WebView2 런타임이 반드시 존재해야 한다. 설치 프로그램은 미설치 기기를 위한 WebView2 부트스트래퍼를 번들로 포함한다.
- **Tauri IPC 학습 비용**: 기여자는 `invoke_handler!` 등록 패턴과 allowlist 모델을 이해해야 한다. ADR-006 참조.
- **WebView 메모리 오버헤드**: 단순 레이아웃의 iced 대비 런타임에 WebView 프로세스가 약 50 MB를 차지한다.

---

## 업데이트 정책

### 마이너 릴리스 (x.Y.z)

발행 후 30일 이내에 Tauri 마이너 릴리스를 따른다. 마이너 릴리스는 IPC 계약을 깨지 않고 기능을 추가하고 버그를 수정한다.

절차:
1. `src-tauri/Cargo.toml`의 `tauri` 버전을 업데이트한다.
2. `cargo check --workspace` 및 `cargo test --workspace`를 실행한다.
3. `tauri.conf.json` 스키마 URL이 최신인지 확인한다.
4. 머지 전에 전체 CI 파이프라인을 실행한다.

### 패치 릴리스 (x.y.Z)

발행 후 7일 이내에 보안 패치를 적용한다. CVE 태그 패치는 연기하지 않는다.

### 메이저 릴리스 (X.y.z)

Tauri 메이저 릴리스는 케이스별로 평가한다. Tauri 메이저 버전을 채택하기 전에 새 ADR이 필요하다. 평가 기준:

- IPC 계약 호환성(ADR-006 버저닝 정책 참조).
- 기존 커맨드에 영향을 주는 WebView API 변경.
- macOS 공증 및 Windows 서명 워크플로에 대한 영향.
- 플러그인 API 안정성(`tauri-plugin-notification`, `tauri-plugin-updater`).

---

## 보안 모델

Tauri v2는 `src-tauri/tauri.conf.json`에 설정된 Content Security Policy를 적용한다.

```json
"security": {
  "csp": "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self' http://127.0.0.1:10090; object-src 'none'; base-uri 'self'",
  "dangerousDisableAssetCspModification": false
}
```

핵심 사항:

- `script-src 'self'`: 애플리케이션과 번들된 스크립트만 실행 가능. 인라인 스크립트, 외부 CDN 스크립트 없음.
- `connect-src 'self' http://127.0.0.1:10090`: 프론트엔드는 임베디드 에셋과 로컬 Axum 웹 서버에서만 fetch 가능. WebView에서의 임의 외부 연결 없음.
- `object-src 'none'`: 플러그인 오브젝트(Flash 등) 차단.
- `dangerousDisableAssetCspModification: false`: 프론트엔드가 수정을 시도하더라도 Tauri가 CSP를 적용한다.

IPC 표면은 명시적으로 등록된 커맨드로만 제한된다. 전체 커맨드 계약은 ADR-006 참조.

---

## 거버넌스 책임

| 책임 | 담당 |
|------|------|
| Tauri 마이너/패치 업데이트 | 엔지니어링 리드 |
| Tauri 메이저 버전 평가 | 아키텍처 검토 (새 ADR 필요) |
| CSP 정책 변경 | 머지 전 보안 검토 필요 |
| IPC 표면 확장 | 아키텍처 검토 (ADR-006 업데이트 필요) |
| macOS 공증 자격증명 | DevOps / 릴리스 엔지니어링 |
| Windows 서명 인증서 | DevOps / 릴리스 엔지니어링 |
