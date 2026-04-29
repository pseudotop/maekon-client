[English](./autostart.md) | 한국어

# 자동 시작 (Autostart) 운영 가이드

## 개요

Maekon은 사용자 로그인 시 자동으로 실행되는 자동 시작 기능을 제공합니다.

- **정책**: opt-in — 사용자가 명시적으로 활성화해야 합니다
- **활성화 방법**: 앱 내 Settings → Startup 토글
- **지원 플랫폼**: macOS, Windows, Linux (데스크톱 세션 필요)

자동 시작을 활성화하면 로그인 후 Maekon이 백그라운드에서 자동으로 시작되어 작업 컨텍스트 수집을 즉시 시작합니다.

---

## 플랫폼별 동작

### macOS

| 항목 | 값 |
|------|-----|
| 등록 위치 | `~/Library/LaunchAgents/com.oneshim.agent.plist` |
| 메커니즘 | `launchctl load` / `launchctl unload` |
| 단일 인스턴스 | Unix domain socket (tauri-plugin-single-instance) |

**비고**: macOS Gatekeeper 공증(notarization)이 적용된 바이너리에서만 정상 작동합니다. 미서명 빌드에서는 자동 시작 등록이 실패할 수 있습니다.

### Windows

| 항목 | 값 |
|------|-----|
| 등록 위치 | 레지스트리 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` |
| 메커니즘 | Windows 레지스트리 API |
| 단일 인스턴스 | 명명된 파이프(Named Pipe) (tauri-plugin-single-instance) |

**비고**: 표준 사용자 계정에서도 HKCU 레지스트리에 쓸 수 있으므로 관리자 권한이 필요하지 않습니다.

### Linux

| 항목 | 값 |
|------|-----|
| 1차 메커니즘 | systemd user service `~/.config/systemd/user/oneshim.service` (Type=notify) |
| 2차 fallback | XDG Autostart `~/.config/autostart/oneshim.desktop` |
| 단일 인스턴스 | D-Bus name `com.oneshim.client.SingleInstance` |

Linux에서는 먼저 `systemctl --user`를 사용할 수 있는지 확인하고, 가능하면 systemd user service 방식을 사용합니다. systemd가 없는 환경(일부 컨테이너, 구형 배포판 등)에서는 XDG Autostart `.desktop` 파일 방식으로 자동 전환됩니다.

**Type=notify 의미**: `sd_notify(3)`를 통해 systemd에 "준비 완료" 신호를 보냅니다. 이를 통해 systemd가 Maekon이 실제로 초기화를 마쳤는지 추적할 수 있습니다. `TimeoutStartSec=30` 내에 신호가 오지 않으면 서비스 시작 실패로 처리됩니다.

---

## Linux 환경별 지원

| 환경 | 지원 | 비고 |
|------|------|------|
| systemd user session (대부분의 데스크톱 배포판) | ✅ | Type=notify로 정확한 readiness 신호 제공 |
| systemd 부재 환경 (XDG fallback) | ✅ | `.desktop` 파일 사용; readiness 신호 없음 |
| Snap 패키지 | ❌ | Snap 내장 자동 시작 사용 권장 |
| Flatpak 패키지 | ❌ | Flatpak background portal API 사용 권장 |
| Headless 환경 (SSH, 디스플레이 없음) | ❌ | 데스크톱 세션 필요 |

> **systemd 최소 버전**: systemd 219 이상 필요 (Ubuntu 20.04+, Fedora 33+, Debian 10+ 기본 충족)
> systemd 218 이하 환경에서는 XDG fallback이 자동 적용됩니다.

**지원 여부 판단 로직**: Maekon은 시작 시 다음 순서로 환경을 감지합니다.

1. `FLATPAK_ID` 환경 변수 존재 → Flatpak 환경 감지, 자동 시작 비활성화
2. Snap 감지 (`SNAP` 환경 변수) → Snap 환경 감지, 자동 시작 비활성화
3. `$DISPLAY` / `$WAYLAND_DISPLAY` 부재 + `$DBUS_SESSION_BUS_ADDRESS` 부재 → headless 환경, 비활성화
4. `systemctl --user is-system-running` 성공 여부로 systemd/XDG 분기

---

## 마이그레이션 (PR-B1 → PR-B2 업그레이드)

PR-B1(v0.4.40 이전)은 systemd `Type=simple` 서비스 파일을 사용했습니다. PR-B2(Maekon v0.4.41+)는 더 정확한 readiness 신호를 위해 `Type=notify`로 변경합니다.

**두 가지 PR-B1 변형에 대해**: PR-B1 era는 다음 두 가지 변형이 존재합니다.

- v0.4.40-rc.1 / rc.2: `Description=ONESHIM Desktop Agent`
- v0.4.40-rc.3 / v0.4.40: `Description=Maekon Desktop Agent`

PR-B2의 자동 마이그레이션은 두 변형 모두 인식합니다 (`KNOWN_PRIOR_HASHES`에 두 hash 등록).

### 자동 마이그레이션

Maekon v0.4.41+ 첫 실행 시 다음 과정이 자동으로 수행됩니다:

1. `~/.config/systemd/user/oneshim.service` 파일의 SHA-256 hash 계산
2. 알려진 PR-B1 템플릿 hash와 비교
3. **일치하는 경우**: 자동으로 PR-B2 템플릿으로 덮어씁니다
   - `daemon-reload`는 다음 로그인 시까지 지연 (현재 실행 중인 서비스 중단 없음)
   - 로그에 `autostart: service file migrated` 기록
4. **불일치하는 경우** (사용자 customization 감지):
   - 자동 마이그레이션 스킵
   - 로그에 `WARN autostart: service file has local modifications, skipping auto-migration` 기록
   - 수동 마이그레이션 안내 (아래 절차 참조)

### 수동 마이그레이션 (서비스 파일을 커스터마이징한 사용자)

```bash
# 1. 기존 customization 백업
cp ~/.config/systemd/user/oneshim.service \
   ~/.config/systemd/user/oneshim.service.backup

# 2. 필요한 변경사항 적용:
#    - Type=simple  →  Type=notify
#    - 추가: NotifyAccess=main
#    - 추가: TimeoutStartSec=30
#    - 기존 Environment= 줄 등 customization은 유지

# 3. 변경사항 확인
grep -E "^Type=|^NotifyAccess=|^TimeoutStartSec=" \
  ~/.config/systemd/user/oneshim.service

# 4. systemd 리로드 및 서비스 재시작
systemctl --user daemon-reload
systemctl --user restart oneshim.service

# 5. 상태 확인
systemctl --user status oneshim.service
```

**변경 후 서비스 파일 예시 (핵심 섹션)**:

```ini
[Service]
Type=notify
NotifyAccess=main
TimeoutStartSec=30
ExecStart=/usr/local/bin/oneshim
Restart=on-failure
RestartSec=5
```

---

## 트러블슈팅

### "Settings → Startup 토글이 회색이에요"

자동 시작을 지원하지 않는 환경에서는 토글이 비활성화됩니다.

1. 토글에 마우스를 올려 tooltip 메시지를 확인합니다.
   > Tooltip 텍스트는 `crates/oneshim-web/frontend/src/i18n/locales/ko.json` (`settings.autostart.unsupported_*`)에 정의되어 있습니다.
2. 환경별 대처 방법:
   - **Snap 사용자**: `snap services` 명령 또는 Snap Center에서 "Run on system startup" 옵션 확인
   - **Flatpak 사용자**: Flatpak background portal API를 통한 자동 시작은 GNOME Settings (또는 KDE 시스템 설정)에서 "백그라운드 자동 시작" 옵션을 통해 설정합니다. `~/.var/app/...` 디렉토리는 사용자 데이터 저장 경로로, 자동 시작 설정과는 무관합니다.
   - **Headless 사용자**: SSH 세션은 자동 시작 대상이 아닙니다. 데스크톱 세션에서 설정해야 합니다.

### "활성화했는데 로그인 후 시작이 안 돼요"

**1단계: systemd 서비스 상태 확인**

```bash
systemctl --user status oneshim.service
journalctl --user -u oneshim.service -n 50
```

**2단계: 자주 발생하는 원인과 해결책**

| 증상 | 원인 | 해결책 |
|------|------|--------|
| `timeout: starting` | `TimeoutStartSec=30` 초과 (HDD 환경, 대용량 DB) | `TimeoutStartSec=60` 으로 증가 |
| `Failed to connect to bus` | D-Bus 미실행 | `systemctl --user start dbus` 또는 로그인 재시도 |
| `No such file or directory` | 바이너리 경로 변경 | `which oneshim` 확인 후 service 파일 `ExecStart=` 수정 |
| 즉시 종료, 로그 없음 | 이중 실행 감지 | 기존 프로세스 확인: `pgrep -a oneshim` |

**`TimeoutStartSec` 수정 방법**:

```bash
# ~/.config/systemd/user/oneshim.service 편집
sed -i 's/^TimeoutStartSec=.*/TimeoutStartSec=60/' \
  ~/.config/systemd/user/oneshim.service
systemctl --user daemon-reload
systemctl --user restart oneshim.service
```

**3단계: 로그 위치**

- **macOS**: `~/Library/Logs/oneshim/`
- **Windows**: `%LOCALAPPDATA%\oneshim\logs\`
- **Linux**: `~/.local/share/oneshim/logs/` 또는 `journalctl --user -u oneshim`

### "service 파일이 customize되어 마이그레이션 스킵됨"

로그에서 다음 메시지를 확인한 경우:

```
WARN autostart: service file has local modifications, skipping auto-migration
```

위 "[수동 마이그레이션](#수동-마이그레이션-서비스-파일을-커스터마이징한-사용자)" 절차를 따르세요. 기존 customization(`Environment=`, `ConditionEnvironment=` 등)을 유지하면서 `Type=notify`와 `NotifyAccess=main`, `TimeoutStartSec=30`만 추가하면 됩니다.

### "로그인할 때마다 두 개의 Maekon 프로세스가 떠요"

단일 인스턴스 감지가 작동하지 않는 경우입니다.

**진단**:

```bash
# 실행 중인 oneshim 프로세스 목록 확인
pgrep -a oneshim

# D-Bus name 점유 여부 확인
dbus-send --session --print-reply \
  --dest=org.freedesktop.DBus \
  /org/freedesktop/DBus \
  org.freedesktop.DBus.ListNames \
  | grep oneshim
```

**가능한 원인**:

- **Headless SSH 세션에서 실행**: `$DBUS_SESSION_BUS_ADDRESS`가 설정되지 않아 D-Bus 연결 실패 → 단일 인스턴스 감지 비활성화됨 (이 경우 duplicate process가 발생할 수 있습니다)
- **두 개의 autostart 경로 공존**: systemd service + XDG `.desktop` 파일이 동시에 등록된 경우

**수동 정리**:

```bash
# XDG .desktop 파일 제거 (systemd 사용 환경에서 중복인 경우)
rm -f ~/.config/autostart/oneshim.desktop

# 실행 중인 중복 프로세스 종료
pkill -f oneshim
# 이후 정상적으로 단일 프로세스만 재시작
systemctl --user start oneshim.service
```

---

## 단일 인스턴스 동작

Maekon은 동시에 하나의 인스턴스만 실행됩니다.

| 상황 | 동작 |
|------|------|
| 첫 번째 실행 | 정상 시작 |
| 두 번째 실행 시도 | 첫 번째 인스턴스 윈도우로 포커스 이동 후 즉시 종료 |

플랫폼별 신호 전달 메커니즘:

- **macOS**: Unix domain socket (tauri-plugin-single-instance)
- **Windows**: 명명된 파이프 (Named Pipe)
- **Linux**: D-Bus name `com.oneshim.client.SingleInstance`

**알려진 한계 — Wayland tray-only 시작 시**:

첫 번째 인스턴스가 메인 윈도우를 한 번도 표시하지 않고 tray-only 모드로 시작된 경우(Wayland 일부 환경), 두 번째 실행(예: dock 아이콘 클릭)이 포커스 신호를 보내도 윈도우가 표시되지 않을 수 있습니다.

이는 PR-B1 risk register §13에서 known limitation으로 수용된 사항입니다. 이 동작이 발생하면:

1. 트레이 아이콘 클릭 → "Show window" 메뉴 항목 선택
2. 또는 터미널에서 `oneshim --show-window` 실행 (후속 PR에서 `window.create()` fallback 추가 예정)

---

## 참고

- **PR-B1 spec**: `docs/superpowers/specs/2026-04-25-phase9-pr-b-autostart-ipc-foundation-design.md`
- **PR-B2 spec**: `docs/superpowers/specs/2026-04-25-phase9-pr-b2-autostart-linux-deep-design.md`
- **ADR-019**: 와이어 코드 인프라 (`crates/oneshim-core/src/error_codes/autostart.rs`)
- **단일 인스턴스 플러그인**: tauri-plugin-single-instance v2
- **설치 가이드**: [`docs/install.ko.md`](./install.ko.md) — 바이너리 설치 방법 (파일명 / 환경 변수 레거시 이름 안내 포함)
