[English](./ADR-020-macos-private-api-policy.md) | [한국어](./ADR-020-macos-private-api-policy.ko.md)

# ADR-020: macOS Private API 정책 (macOSPrivateApi: true)

**Status**: Accepted
**Date**: 2026-05-18
**Scope**: `src-tauri/tauri.conf.json`, `src-tauri/src/magic_overlay.rs`
**Supersedes**: none
**Related**: ADR-004 (Tauri v2 마이그레이션), ADR-005 (Tauri v2 거버넌스)
**Implementation**: `src-tauri/tauri.conf.json:12`, `src-tauri/src/magic_overlay.rs:124-162`

---

## 배경 (Context)

`tauri.conf.json`의 `app` 객체에 `"macOSPrivateApi": true`가 설정되어 있습니다.
이 플래그는 Tauri의 macOS 전용 비공개 API 표면을 활성화하며, 다음 기능을 포함합니다:

- **투명 창(Transparent window)** — 이 플래그 없이 macOS에서
  `WebviewWindowBuilder::transparent(true)`를 사용하면 묵시적 실패 또는 패닉이
  발생합니다.
- **풀 콘텐츠 뷰 타이틀 바** — `tauri.conf.json`에 설정된 `titleBarStyle: "Overlay"`와
  `hiddenTitle: true` 조합은 WebView를 타이틀 바 트래픽 라이트 버튼 뒤까지 확장하기
  위해 비공개 API 표면이 필요합니다.
- **`NSVisualEffectView` 바이브런시** — 미래 창에서 사용 가능하도록 예약되었으며
  현재는 미사용입니다.

주요 소비 지점은 `src-tauri/src/magic_overlay.rs`의
`MagicOverlayHandle::ensure_window()`입니다. 이 함수는 코칭/탐지 오버레이용
전체 화면, 항상 최상단, 투명 WebView 창을 생성합니다:

```
.transparent(true)
.always_on_top(true)
.decorations(false)
.shadow(false)
```

macOS에서 `WebviewWindowBuilder`의 `.transparent(true)`는 컴파일 타임에 Tauri의
`macos-private-api` 피처가 활성화되어 있고, 런타임에 `tauri.conf.json`에
`macOSPrivateApi: true`가 설정된 경우에만 동작합니다. 없으면 Tauri 2는 macOS에서
창을 흰색 또는 검은색 배경으로 렌더링하여 오버레이가 동작하지 않습니다.

메인 창도 `titleBarStyle: "Overlay"`와 `hiddenTitle: true`를 사용하여 프레임리스
타이틀 바(트래픽 라이트가 콘텐츠 영역 안에 내장)를 구현합니다. 이 역시 비공개
API 표면에 의존합니다.

### Apple 공증(Notarization) 및 App Store 현황

| 배포 채널 | 영향 |
|---|---|
| 직접 다운로드 (DMG/PKG 서명 + 공증) | **영향 없음.** Apple 공증은 `macOSPrivateApi` 앱을 거부하지 않습니다. Maekon은 표준 엔타이틀먼트(`hardened-runtime`, 화면 녹화, 접근성)로 공증을 통과합니다. |
| Mac App Store | **App Store 제출 불가.** MAS는 비공개 API를 사용하는 앱을 거부합니다(App Store 심사에서 강제). 이는 허용 가능한 제약입니다: Maekon은 현재도, 향후에도 Mac App Store 배포를 계획하지 않습니다. 화면 녹화 및 접근성 엔타이틀먼트 자체도 이미 MAS 비호환입니다. |

이 플래그는 Hardened Runtime을 비활성화하지 않으며(`"hardenedRuntime": true` 유지),
기존 엔타이틀먼트가 이미 부여하는 것 이상의 샌드박싱 우려를 도입하지 않습니다.

### SOC 2 / ISMS-P 감사 포지션

`macOSPrivateApi: true`는 공증 과정에서 Apple에 노출되지만, 추가적인 데이터 수집이나
네트워크 표면을 도입하지 않습니다. 이 플래그는 렌더링/창 관련 기능 활성화 목적에만
사용됩니다. "왜 비공개 API가 활성화되었는가?"라고 묻는 감사자는 이 ADR과
`src-tauri/src/magic_overlay.rs`를 주요 근거로 안내해야 합니다.

## 결정 (Decision)

### 1. `"macOSPrivateApi": true` 유지

플래그를 계속 활성화합니다. MagicOverlay 투명 창과 `titleBarStyle: "Overlay"` 메인
창 UX에 필요합니다. 비활성화하면 둘 다 깨집니다.

### 2. 이 ADR이 유일한 권위적 설명

JSON은 주석을 지원하지 않으므로 `tauri.conf.json`에 인라인 근거를 기재할 수 없습니다.
이 ADR 파일(`docs/architecture/ADR-020-macos-private-api-policy.md`)이 표준 참조입니다.
ADR-005(Tauri v2 거버넌스)는 `macOSPrivateApi` 항목에 대해 이 ADR을 교차 참조합니다.

### 3. 제거 트리거 조건

다음 **모든** 조건이 충족될 때 `macOSPrivateApi`를 재평가하고 `false`로 변경을 검토해야
합니다:

1. MagicOverlay 투명 창 기능이 제거되거나 투명도가 필요 없는 메커니즘(예: 시스템
   알림 UI)으로 교체된 경우.
2. 메인 창이 더 이상 `titleBarStyle: "Overlay"` / `hiddenTitle: true`를 사용하지
   않는 경우.
3. 미래 창에서도 `NSVisualEffectView` 바이브런시가 필요하지 않은 경우.

이 ADR을 대체하는 미래 ADR은 세 가지 조건 모두를 다루어야 합니다.

### 4. App Store 배포 없음

Maekon은 명시적으로 Mac App Store 배포를 목표로 하지 않습니다. MAS 규칙과
`macOSPrivateApi` 및 `com.apple.security.device.screen-capture` 엔타이틀먼트 간의
비호환성은 인지하고 수용합니다.

## 결과 (Consequences)

### 긍정적

- MagicOverlay 투명 코칭/탐지 오버레이가 macOS에서 올바르게 동작합니다.
- 메인 창이 예상되는 프레임리스 외관을 가집니다(트래픽 라이트가 콘텐츠 영역에 내장).
- Apple 공증(DMG + PKG)이 기존 엔타이틀먼트로 성공합니다.

### 부정적

- 이 플래그가 활성화된 동안 Maekon을 Mac App Store에 제출할 수 없습니다
  (화면 녹화 엔타이틀먼트도 독립적으로 MAS 차단 요인입니다).
- Tauri에 익숙하지 않은 감사자나 신규 기여자가 이 ADR 없이는 설정을 의심스럽게
  볼 수 있습니다.

### 중립적

- Hardened Runtime은 계속 활성화됩니다. 이 플래그는 창 렌더링에만 영향을 미치며
  런타임 보안 포지션에는 영향을 주지 않습니다.
- Linux 및 Windows 빌드는 이 설정에 영향을 받지 않습니다.

## 검토한 대안 (Alternatives Considered)

**A. `macOSPrivateApi` 비활성화 및 불투명 창 사용.**
MagicOverlay를 투명도 없이 재구현해야 하며 UX가 크게 저하됩니다. 메인 창 타이틀 바도
Overlay 스타일을 잃게 됩니다. 기각.

**B. 별도 헬퍼 프로세스로 투명 오버레이 구현(Tauri 창 미사용).**
커스텀 Swift/ObjC 헬퍼가 필요하며 별도 빌드 파이프라인, 서명 ID, IPC가 필요합니다.
현재 접근 방식보다 훨씬 복잡합니다. 기각.

**C. macOS에서 오버레이 제공 중단.**
코칭/탐지 오버레이는 핵심 기능입니다. macOS에서 비활성화하는 것은 허용되지 않습니다.
기각.

## 알려진 후속 작업 (Known Follow-ups)

1. **MAS 적합성 감사** — Maekon이 App Store 배포를 검토하게 된다면 포괄적인
   엔타이틀먼트 감사가 먼저 필요합니다(화면 녹화, 접근성, `macOSPrivateApi` 모두
   독립적으로 MAS 차단 요인). 비즈니스 결정이 변경될 때 별도 ADR로 추적합니다.

## 관련 문서 (Related Docs)

- `docs/architecture/ADR-004-tauri-v2-migration.md` — Tauri v2 마이그레이션 배경
- `docs/architecture/ADR-005-tauri-governance.md` — `tauri.conf.json` 거버넌스 규칙
- `src-tauri/src/magic_overlay.rs` — 주요 소비 지점 (투명 오버레이 창)
- `src-tauri/tauri.conf.json` — `macOSPrivateApi: true`를 설정하는 구성 파일
- `src-tauri/assets/maekon.entitlements` — Hardened Runtime 엔타이틀먼트
- `docs/guides/macos-release-signing-runbook.md` — 서명 및 공증 절차
