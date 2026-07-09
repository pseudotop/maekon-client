<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="./assets/brand/logo-full-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="./assets/brand/logo-full-light.svg">
    <img alt="Maekon" src="./assets/brand/logo-full-light.svg" width="400">
  </picture>
</p>

<p align="center">
  <a href="./README.md">English</a> | <a href="./README.ko.md">한국어</a> | <a href="./README.ja.md">日本語</a> | <a href="./README.zh-CN.md">简体中文</a> | <a href="./README.es.md">Español</a>
</p>
<p align="center">
  <a href="https://maekon.dev">웹사이트</a> · <a href="https://docs.maekon.dev">공식 문서</a> · <a href="https://github.com/pseudotop/maekon-client/releases">릴리스</a>
</p>


# Maekon

> **흩어진 업무 흔적을, 매일 성과로 이어지는 집중 인사이트로.**
> Maekon은 로컬 업무 신호를 집중 타임라인, 다음 행동 후보, 정책 기반 자동화 경로로 정리합니다.

Maekon은 ONESHIM 없이도 독립 사용 가능한 Apache-2.0 local-first 데스크톱 에이전트입니다. 로컬 컨텍스트 수집, 사용자가 검토하는 다음 행동 후보, 정책 기반 자동화, 내장 대시보드를 제공합니다. Rust로 구축되어 macOS, Windows, Linux에서 네이티브 성능을 발휘합니다.

## Source Build 빠른 시작

공개 저장소는 준비되었고, `v0.0.1-rc.6`가 현재 공개 prerelease로 게시되어
있습니다. GitHub의 `latest` release endpoint는 prerelease를 포함하지 않으므로
릴리즈 바이너리 테스트는 설치 문서의 버전 고정 명령을 사용하세요. 개발 및
debug 빌드는 로컬 source checkout에서 실행합니다.

```bash
git clone https://github.com/pseudotop/maekon-client.git
cd maekon-client

# Build the two bundled prerequisites the Tauri config requires before the app
# can run from source (a fresh checkout has neither yet):
#   1) the web dashboard frontend  -> crates/maekon-web/frontend/dist
#   2) the sandbox-worker sidecar   -> src-tauri/maekon-sandbox-worker-<target-triple>
(cd crates/maekon-web/frontend && pnpm install && pnpm build)
cargo build -p maekon-sandbox-worker
cp target/debug/maekon-sandbox-worker \
  "src-tauri/maekon-sandbox-worker-$(rustc -vV | sed -n 's/host: //p')"

# Run Maekon from source
./scripts/cargo-cache.sh run -p maekon-app -- --offline
```

릴리즈 설치 명령은 아래 설치 문서에 정리되어 있습니다. Prerelease 버전 고정,
서명 검증 강제, 제거 방법:
- 한국어: [`docs/install.ko.md`](./docs/install.ko.md)
- English: [`docs/install.md`](./docs/install.md)

## Maekon을 선택하는 이유

- **활동을 통제 가능한 업무 인사이트로 정리**: 컨텍스트, 타임라인, 집중 패턴, 방해 요소, 승인된 자동화 경로를 한 곳에서 추적합니다.
- **가벼운 온디바이스 처리**: Edge 처리(델타 인코딩, 썸네일, OCR)로 전송량을 줄이고 빠른 응답 속도를 유지합니다.
- **프로덕션 수준의 데스크톱 스택**: 크로스 플랫폼 바이너리, 자동 업데이트, 시스템 트레이 통합, 로컬 웹 대시보드를 지원합니다.

### 시장 포지셔닝 (2026)

Google DeepMind (AI Pointer, 2026-05) 와 OpenAI (Codex Chronicle, 2026-04) 가 동일 문제 공간에 진입했습니다 — **화면 맥락을 이해하고 자연 지시·포인팅으로 행동하는 AI**. Maekon은 다음 4축으로 차별화합니다:

1. **기본 local-first** — 픽셀, OCR, 신호가 on-device 유지. 클라우드 round-trip은 opt-in
2. **Source-first 감사** — 모든 신호에 origin, retention, PII filter step trace
3. **Policy-gated 자동화** — 자연 지시("이것 요약해", "저것 정리")를 명시적 검토·승인 경계가 있는 **next-action candidates** 로 처리 (직접 실행 X)
4. **앱·OS 횡단** — Chrome, native 앱, terminal, OS 워크플로우 가로지름 (3 OS: macOS, Windows, Linux). 단일 벤더 생태계에 묶이지 않음

전체 포지셔닝 매트릭스와 레퍼런스는 [`docs/market-positioning-references.ko.md`](./docs/market-positioning-references.ko.md) 참조.

## 대상 사용자

- 집중 패턴과 업무 컨텍스트에 대한 가시성을 원하는 개인 기여자
- 풍부한 데스크톱 신호를 기반으로 AI 지원 워크플로우 도구를 개발하는 팀
- 모듈식 고성능 클라이언트와 명확한 아키텍처 경계를 원하는 개발자

## 2분 빠른 시작

```bash
# 1) Standalone 모드로 실행 (보안 민감 환경 권장)
./scripts/cargo-cache.sh run -p maekon-app -- --offline

# 2) 로컬 대시보드 열기
# http://localhost:10090
```

Standalone 모드는 현재 사용 가능합니다.

Connected 모드는 opt-in 프리뷰 경로로만 제공됩니다.
릴리스 운영 환경에서는 Standalone 모드를 기본 경로로 사용하세요.

## 보안 및 개인정보 보호 요약

- PII 필터링 수준(Off/Basic/Standard/Strict)이 비전 파이프라인에 적용됩니다
- 로컬 데이터는 SQLite에 저장되며, 보존 정책으로 관리됩니다
- 자동화는 실행 정책, 샌드박스 프로필, 로컬 감사 로그를 통과합니다
- 보안 보고 및 대응 정책: [SECURITY.md](./SECURITY.md)
- Standalone 무결성 베이스라인: [docs/security/standalone-integrity-baseline.ko.md](./docs/security/standalone-integrity-baseline.ko.md)
- 무결성 운영 런북(영문): [docs/security/integrity-runbook.md](./docs/security/integrity-runbook.md)
- 문서 인덱스: [docs/README.ko.md](./docs/README.ko.md)
- 자동화 플레이북 템플릿: [docs/guides/automation-playbook-templates.ko.md](./docs/guides/automation-playbook-templates.ko.md)
- Standalone 도입 런북: [docs/guides/standalone-adoption-runbook.ko.md](./docs/guides/standalone-adoption-runbook.ko.md)
- 첫 5분 가이드: [docs/guides/first-5-minutes.ko.md](./docs/guides/first-5-minutes.ko.md)
- 자동화 이벤트 계약: [docs/contracts/automation-event-contract.ko.md](./docs/contracts/automation-event-contract.ko.md)
- AI 제공자 계약: [docs/contracts/ai-provider-contract.ko.md](./docs/contracts/ai-provider-contract.ko.md)

### 소스에서 직접 검증하기

위의 프라이버시 주장은 마케팅 문구가 아닙니다 — 각 주장은 이 저장소에서 직접 읽고, 빌드하고, 테스트할 수 있는 코드에 대응합니다. README와 소스는 동일한 검증 트리에서 함께 export되므로, 이 표는 항상 바로 옆에 있는 코드를 설명합니다.

| 주장 | 검증 위치 |
|---|---|
| 제외/민감 앱은 업로드 시점이 아니라 **캡처 시점에** 제외됩니다 | [`crates/maekon-vision/src/privacy/detection.rs`](./crates/maekon-vision/src/privacy/detection.rs) (`should_exclude_by_policy`), 캡처 게이트 배선: [`src-tauri/src/scheduler/loops/monitor_phases.rs`](./src-tauri/src/scheduler/loops/monitor_phases.rs) |
| 기기를 떠나는 모든 전송은 로컬 egress 원장에 기록되며 앱에서 열람 가능합니다 (Privacy → Egress ledger) | [`src-tauri/src/scheduler/egress_policy.rs`](./src-tauri/src/scheduler/egress_policy.rs) + 리더 라우트: [`crates/maekon-web/src/routes.rs`](./crates/maekon-web/src/routes.rs) |
| 메모리 그래프가 축적한 사용자에 대한 믿음(claims)은 열람 및 원클릭 철회가 가능합니다 (Privacy → Claims) | claims 라우트: [`crates/maekon-web/src/routes.rs`](./crates/maekon-web/src/routes.rs) |
| 동의는 fail-closed입니다: 유효한 동의가 없으면 캡처하지 않습니다 | [`crates/maekon-core/src/consent.rs`](./crates/maekon-core/src/consent.rs) |
| PII 필터링은 저장 전과 모든 egress 전에 실행됩니다 | [`crates/maekon-vision/src/privacy/`](./crates/maekon-vision/src/privacy/) |
| 자동화는 정책·샌드박스·감사 로깅을 우회할 수 없습니다 | [`crates/maekon-automation/src/`](./crates/maekon-automation/src/) |

### 소스 동기화 정책

이 저장소는 Maekon 내부 소스의 **검증된 스냅샷 export**입니다. 스냅샷은 릴리스 단위로 검증 후 export되며 — 릴리스 태그가 검증된 상태를 표시하고, 저장소는 내부의 모든 커밋이 아니라 릴리스를 추적합니다. README와 코드는 항상 같은 트리에서 나오므로, 위의 주장-코드 링크는 지금 읽고 있는 체크아웃을 정확히 가리킵니다.

## 기능

### 핵심 기능
- **실시간 컨텍스트 모니터링**: 활성 창, 시스템 리소스, 사용자 활동을 추적합니다
- **Edge 이미지 처리**: 스크린샷 캡처, 델타 인코딩, 썸네일, OCR 지원
- **정책 기반 자동화**: 승인된 액션을 정책 검사, 샌드박스 격리, 감사 로그 경로로 실행합니다
- **서버 연동 기능 (프리뷰 / Opt-in)**: 검토 가능한 다음 행동 후보와 피드백 동기화는 단계적 검증용으로 제공되며 기본 프로덕션 경로는 아닙니다
- **시스템 트레이**: 백그라운드에서 실행되며 빠른 접근이 가능합니다
- **자동 업데이트**: GitHub Releases 기반 자동 업데이트
- **크로스 플랫폼**: macOS, Windows, Linux를 지원합니다

### 로컬 웹 대시보드 (http://localhost:10090)
- **대시보드**: 실시간 시스템 지표, CPU/메모리 차트, 앱 사용 시간
- **타임라인**: 스크린샷 타임라인, 태그 필터링, 라이트박스 뷰어
- **리포트**: 주간/월간 활동 리포트, 생산성 분석
- **세션 재생**: 앱 세그먼트 시각화를 포함한 세션 재생
- **집중 분석**: 집중도 분석, 방해 요소 추적, 로컬 제안
- **설정**: 설정 관리, 데이터 내보내기/백업

### 데스크톱 알림
- **유휴 알림**: 30분 이상 비활성 시 트리거
- **장시간 세션 알림**: 60분 이상 연속 작업 시 트리거
- **높은 사용량 알림**: CPU/메모리가 90%를 초과하면 트리거
- **집중 제안**: 휴식 알림, 집중 시간 스케줄링, 컨텍스트 복원

## 요구 사항

- Rust 1.88.0 이상
- macOS 10.15+ / Windows 10+ / Linux (X11/Wayland)

## 개발자 빠른 시작 (소스에서 빌드)

### 빌드

```bash
# 임베드되는 웹 대시보드 에셋 빌드 (패키징/릴리스 빌드 전 필수)
./scripts/build-frontend.sh

# 개발 빌드
./scripts/cargo-cache.sh build -p maekon-app

# 릴리스 빌드
./scripts/cargo-cache.sh build --release -p maekon-app
```

### 빌드 캐시 (로컬 개발 권장)

```bash
# 선택: sccache 설치
brew install sccache

# 캐시를 사용하는 Rust 빌드 래퍼
./scripts/cargo-cache.sh check --workspace
./scripts/cargo-cache.sh test -p maekon-web
./scripts/cargo-cache.sh build -p maekon-app
```

`sccache`가 없으면 래퍼는 일반 `cargo`로 자동 폴백합니다.

`cargo-cache.sh`는 로컬 디스크 폭증 방지를 위해 `target` 용량 가드도 적용합니다:
- 소프트 제한(`MAEKON_TARGET_SOFT_LIMIT_MB`, 기본값 `8192`): `target/debug/incremental` 정리 후, 여전히 크면 `target/debug/deps` 정리
- 하드 제한(`MAEKON_TARGET_HARD_LIMIT_MB`, 기본값 `12288`): 추가로 `target/debug/build` 정리
- 자동 정리 토글: `MAEKON_TARGET_AUTO_PRUNE=1` (기본) / `0` (비활성화)
- 현재 캐시 상태 확인: `./scripts/cargo-cache.sh --status`

제한값 커스텀 예시:
```bash
MAEKON_TARGET_SOFT_LIMIT_MB=4096 \
MAEKON_TARGET_HARD_LIMIT_MB=6144 \
./scripts/cargo-cache.sh test --workspace
```

### 실행

```bash
# Standalone 모드 (권장)
./scripts/cargo-cache.sh run -p maekon-app -- --offline
```

Connected 모드는 프리뷰 전용이며, 명시적인 서버/인증 설정에서만 사용하도록 게이트되어 있습니다.
운영 환경 기본값은 Standalone 모드이며, Connected 모드는 환경 검증 후에만 사용하세요.

macOS headless CI/원격 디버그처럼 WindowServer가 없는 환경에서 트레이 초기화가 실패할 수 있으면:
```bash
MAEKON_DISABLE_TRAY=1 ./scripts/cargo-cache.sh run -p maekon-app -- --offline --gui
```
이 값은 비대화형 smoke/debug 경로에서만 사용하세요.

### 테스트

```bash
# Rust 테스트
./scripts/cargo-cache.sh test --workspace

# E2E 테스트 — 웹 대시보드
cd crates/maekon-web/frontend && pnpm test:e2e

# 린트 (정책: CI에서 경고 0건)
./scripts/cargo-cache.sh clippy --workspace

# 포맷 검사
./scripts/cargo-cache.sh fmt --check
```

### macOS WindowServer Smoke (Self-hosted)

실제 WindowServer 세션에서 macOS GUI 부트스트랩을 검증하려면 다음 수동 워크플로를 실행하세요.
- 워크플로: `.github/workflows/macos-windowserver-gui-smoke.yml`
- 러너 라벨: `self-hosted`, `macOS`, `windowserver`

## 설치

설치 가이드:
- 한국어: [`docs/install.ko.md`](./docs/install.ko.md)
- English: [`docs/install.md`](./docs/install.md)

### 빠른 설치 (터미널)

> 현재 공개 바이너리 릴리즈는 prerelease `v0.0.1-rc.6`입니다. GitHub의
> `latest` stable URL은 첫 stable 릴리즈 전까지 사용할 수 없으므로, 아래
> 명령은 prerelease 버전을 명시적으로 고정합니다.

macOS / Linux:
```bash
curl -fsSL -o /tmp/maekon-install.sh \
  https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.sh
MAEKON_VERSION=v0.0.1-rc.6 bash /tmp/maekon-install.sh
```

Windows (PowerShell):
```powershell
$tmp = Join-Path $env:TEMP "maekon-install.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp -Version v0.0.1-rc.6
```

### 릴리즈 아티팩트

[Releases](https://github.com/pseudotop/maekon-client/releases)에서 플랫폼별 파일을 받을 수 있습니다.

현재 게시된 prerelease는 `v0.0.1-rc.6`입니다. 아래 표는 설치 프로그램,
업데이터, checksum, signature 흐름에서 사용하는 release asset 이름을
문서화한 것입니다.

Maekon은 앱 표시 이름입니다. 현재 릴리즈 파일명은 설치 프로그램, 업데이터,
체크섬 호환성을 위해 의도적으로 `maekon-*` 형식을 유지합니다.

| 플랫폼 | 파일 |
|--------|------|
| macOS Universal (DMG 설치 파일) | `maekon-macos-universal.dmg` |
| macOS Universal (PKG 설치 파일) | `maekon-macos-universal.pkg` |
| macOS Universal | `maekon-macos-universal.tar.gz` |
| macOS Apple Silicon | `maekon-macos-arm64.tar.gz` |
| macOS Intel | `maekon-macos-x64.tar.gz` |
| Windows x64 (zip) | `maekon-windows-x64.zip` |
| Windows x64 (MSI) | `maekon-app-*.msi` |
| Linux x64 (DEB 패키지) | `maekon-*.deb` |
| Linux x64 | `maekon-linux-x64.tar.gz` |

## 설정

### 환경 변수

호환성 메모: `MAEKON_*` 환경 변수, `maekon` CLI 명령,
`com.maekon.app`, 기존 config/data 경로는 이 릴리즈 라인에서 안정적인
기술 식별자로 유지합니다.

| 변수 | 설명 | 기본값 |
|------|------|--------|
| `MAEKON_EMAIL` | 로그인 이메일 (Connected 모드 전용) | (Standalone에서는 선택사항) |
| `MAEKON_PASSWORD` | 로그인 비밀번호 (Connected 모드 전용) | (Standalone에서는 선택사항) |
| `MAEKON_TESSDATA` | Tesseract 데이터 경로 | (선택사항) |
| `MAEKON_DISABLE_TRAY` | 시스템 트레이 초기화 스킵 (headless CI/원격 GUI smoke 전용) | `0` |
| `RUST_LOG` | 로그 레벨 | `info` |

### 설정 파일

`~/.config/maekon/config.json` (Linux) / `~/Library/Application Support/com.maekon.app/config.json` (macOS) / `%APPDATA%\maekon\agent\config.json` (Windows):

```json
{
  "server": {
    "base_url": "https://api.example.com",
    "request_timeout_ms": 30000,
    "sse_max_retry_secs": 30
  },
  "monitor": {
    "poll_interval_ms": 1000,
    "sync_interval_ms": 10000,
    "heartbeat_interval_ms": 30000
  },
  "storage": {
    "retention_days": 30,
    "max_storage_mb": 500
  },
  "vision": {
    "capture_throttle_ms": 5000,
    "thumbnail_width": 480,
    "thumbnail_height": 270,
    "ocr_enabled": false
  },
  "update": {
    "enabled": true,
    "repo_owner": "pseudotop",
    "repo_name": "maekon-client",
    "check_interval_hours": 24,
    "include_prerelease": false
  },
  "web": {
    "enabled": true,
    "port": 10090,
    "allow_external": false
  },
  "notification": {
    "enabled": true,
    "idle_threshold_mins": 30,
    "long_session_threshold_mins": 60,
    "high_usage_threshold_percent": 90
  }
}
```

## 아키텍처

Hexagonal Architecture (Ports & Adapters) 패턴을 따르는 15개 패키지 Cargo 워크스페이스입니다. 14개 크레이트는 `crates/` 아래에 있고, 메인 바이너리/composition root는 `src-tauri/` (Tauri v2, 패키지명 `maekon-app`)에 있습니다.

```
maekon-client/
├── src-tauri/              # Tauri v2 바이너리 진입점 + composition root
│   ├── src/
│   │   ├── main.rs         # Tauri 앱 빌더 + DI 연결
│   │   ├── tray.rs         # 시스템 트레이 메뉴
│   │   ├── commands/       # Tauri IPC 명령
│   │   └── scheduler/      # 백그라운드 스케줄러
│   └── tauri.conf.json     # Tauri 설정
├── crates/
│   ├── maekon-core/       # 도메인 모델 + 포트 트레이트 + 에러 + 설정
│   ├── maekon-network/    # HTTP/SSE/WebSocket/gRPC, 압축, 인증
│   ├── maekon-suggestion/ # 제안 수신 및 처리
│   ├── maekon-storage/    # SQLite 로컬 저장소 + 스키마 마이그레이션
│   ├── maekon-monitor/    # 시스템 지표, 활성 창, 활동 추적
│   ├── maekon-vision/     # 화면 캡처, 델타 인코딩, OCR, PII 필터
│   ├── maekon-web/        # 로컬 웹 대시보드 (Axum REST + React)
│   ├── maekon-automation/ # 자동화 제어, 정책, 감사 로그
│   ├── maekon-analysis/   # LLM 분석 파이프라인, regime 분류
│   ├── maekon-embedding/  # 벡터 임베딩 + INT8 양자화
│   ├── maekon-audio/      # 오디오 캡처 + STT 파이프라인
│   ├── maekon-sandbox-worker/ # out-of-process 샌드박스 실행기
│   ├── maekon-api-contracts/ # 공유 API 타입 계약
│   └── maekon-lint/       # 워크스페이스 lint 도구
└── docs/
    ├── crates/             # 크레이트별 상세 문서
    ├── architecture/       # ADR 문서
    └── migration/          # 마이그레이션 문서
```

### 크레이트 문서

| 크레이트 | 역할 | 문서 |
|----------|------|------|
| maekon-core | 도메인 모델, 포트 인터페이스 | [상세](./docs/crates/maekon-core.md) |
| maekon-network | HTTP/SSE/WebSocket, 압축, 인증 | [상세](./docs/crates/maekon-network.md) |
| maekon-vision | 캡처, 델타 인코딩, OCR | [상세](./docs/crates/maekon-vision.md) |
| maekon-monitor | 시스템 지표, 활성 창 | [상세](./docs/crates/maekon-monitor.md) |
| maekon-storage | SQLite, 오프라인 저장소 | [상세](./docs/crates/maekon-storage.md) |
| maekon-suggestion | 제안 큐, 피드백 | [상세](./docs/crates/maekon-suggestion.md) |
| maekon-web | 로컬 웹 대시보드, REST API | [상세](./docs/crates/maekon-web.md) |
| maekon-automation | 자동화 제어, 정책, 감사 로그 | [상세](./docs/crates/maekon-automation.md) |
| maekon-analysis | LLM 분석 파이프라인, regime 분류 | — |
| maekon-embedding | 벡터 임베딩, INT8 양자화 | — |
| maekon-audio | 오디오 캡처, STT 파이프라인 | — |
| maekon-sandbox-worker | 샌드박스 자동화 액션 실행기 | — |
| maekon-api-contracts | 공유 API 타입 계약 | — |
| maekon-lint | 워크스페이스 lint 도구(language-check) | — |

전체 문서 색인: [docs/crates/README.md](./docs/crates/README.md)

기여 워크플로우: [CONTRIBUTING.md](./CONTRIBUTING.md)

문서 언어 및 일관성 규칙: [docs/DOCUMENTATION_POLICY.md](./docs/DOCUMENTATION_POLICY.md)
한국어 정책 문서: [docs/DOCUMENTATION_POLICY.ko.md](./docs/DOCUMENTATION_POLICY.ko.md)

## 개발

### 코드 스타일

- **언어**: 영문 기본 문서 + 주요 공개 가이드에 대한 한국어 번역 문서 제공
- **포맷**: `cargo fmt` 기본 설정
- **린트**: `cargo clippy` 경고 0건

### 새 기능 추가

1. `maekon-core`에서 포트 트레이트를 정의합니다
2. 해당 크레이트에서 어댑터를 구현합니다
3. `src-tauri/src/main.rs`에서 DI를 연결합니다
4. 테스트를 추가합니다

### 인스톨러 빌드

macOS .app 번들:
```bash
./scripts/cargo-cache.sh install cargo-bundle
./scripts/cargo-cache.sh bundle --release -p maekon-app
```

Windows .msi:
```bash
./scripts/cargo-cache.sh install cargo-wix
./scripts/cargo-cache.sh wix -p maekon-app
```

## 라이선스

Apache License 2.0 -- [LICENSE](./LICENSE) 참조

- [기여 가이드](./CONTRIBUTING.md)
- [행동 강령](./CODE_OF_CONDUCT.md)
- [보안 정책](./SECURITY.md)

## 기여하기

1. Fork
2. 기능 브랜치를 생성합니다 (`git checkout -b feature/amazing`)
3. 변경 사항을 커밋합니다 (`git commit -m 'Add amazing feature'`)
4. 브랜치를 푸시합니다 (`git push origin feature/amazing`)
5. Pull Request를 생성합니다
