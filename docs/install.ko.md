[English](./install.md) | [한국어](./install.ko.md)

# 설치 가이드

이 문서는 Maekon 릴리즈 바이너리를 터미널에서 설치하는 방법을 제공합니다.

호환성 메모: 릴리즈 파일명, 설치 스크립트명, `ONESHIM_*` 환경 변수,
`oneshim` CLI 명령은 설치 프로그램, 업데이터, 기존 사용자 호환성을 위해
현재 이름을 의도적으로 유지합니다.

## 빠른 설치

### macOS

```bash
curl -fsSL -o /tmp/oneshim-install.sh \
  https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.sh
bash /tmp/oneshim-install.sh
```

### Linux

공식 Linux 릴리즈 아티팩트는 현재 임시 보류 상태입니다. upstream Tauri/Wry
GTK 런타임 스택이 아직 문서화된 `glib 0.18.x` advisory 예외를 포함하고
있기 때문입니다. Linux 소스 빌드는 개발 및 내부 검증 용도로 계속 사용할 수
있습니다.

```bash
sudo apt-get update
sudo apt-get install -y build-essential libwebkit2gtk-4.1-dev libgtk-3-dev libglib2.0-dev libclang-dev
cargo build --release -p oneshim-app --features grpc
```

내부 릴리즈 rehearsal에서 별도 Linux archive를 제공하는 경우에만 명시적으로
opt-in 합니다.

```bash
ONESHIM_ALLOW_EXPERIMENTAL_LINUX_INSTALL=1 \
  bash /tmp/oneshim-install.sh --base-url <internal-release-asset-base-url>
```

### Windows (PowerShell)

```powershell
$tmp = Join-Path $env:TEMP "oneshim-install.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp
```

## 특정 버전 설치

### macOS

```bash
ONESHIM_VERSION=v0.0.4 bash /tmp/oneshim-install.sh
```

### Windows

```powershell
powershell -ExecutionPolicy Bypass -File $tmp -Version v0.0.4
```

## 무결성 검증

- `scripts/install.sh`, `scripts/install.ps1`는 릴리즈 사이드카(`.sha256`)를 사용해 `SHA-256`을 항상 검증합니다.
- Ed25519 서명 검증(`.sig`)도 지원하며 강제할 수 있습니다.
  - macOS 및 experimental Linux 설치: `--require-signature` 또는 `ONESHIM_REQUIRE_SIGNATURE=1`
  - Windows: `-RequireSignature`
- 서명 검증에는 설치 환경에 Python + PyNaCl이 필요합니다.
- 기본 업데이트 서명 공개키:
  - `GIdf7Wg4kvvvoT7jR0xwKLKna8hUR1kvowONbHbPz1E=`
- 키 로테이션 시 공개키 덮어쓰기:
  - `ONESHIM_UPDATE_PUBLIC_KEY=<base64-ed25519-public-key>`

## 스크립트 옵션

### macOS / experimental Linux (`scripts/install.sh`)

```bash
bash /tmp/oneshim-install.sh --help
```

주요 옵션:

- `--version <tag>` (기본값: `latest`)
- `--install-dir <path>` (기본값: `~/.local/bin`)
- `--repo <owner/name>` (기본값: `pseudotop/maekon-client`)
- `--base-url <url>` (릴리즈 에셋 소스 오버라이드; 로컬 smoke/rehearsal에 유용)
- `--require-signature`
- `ONESHIM_ALLOW_EXPERIMENTAL_LINUX_INSTALL=1` (공식 Linux 아티팩트 보류 중 Linux archive 설치 시 필요)

### Windows (`scripts/install.ps1`)

```powershell
powershell -ExecutionPolicy Bypass -File $tmp -?
```

주요 파라미터:

- `-Version <tag>` (기본값: `latest`)
- `-InstallDir <path>` (기본값: `%LOCALAPPDATA%\ONESHIM\bin`)
- `-Repository <owner/name>` (기본값: `pseudotop/maekon-client`)
- `-BaseUrl <url>` (릴리즈 에셋 소스 오버라이드; 로컬 smoke/rehearsal에 유용)
- `-RequireSignature`

## 제거

### macOS / experimental Linux

```bash
curl -fsSL -o /tmp/oneshim-uninstall.sh \
  https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/uninstall.sh
bash /tmp/oneshim-uninstall.sh
```

### Windows

```powershell
$tmp = Join-Path $env:TEMP "oneshim-uninstall.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/uninstall.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp
```

## 로컬 저장소에서 실행

이미 저장소를 clone한 경우:

```bash
./scripts/install.sh
./scripts/uninstall.sh
```

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\install.ps1
powershell -ExecutionPolicy Bypass -File .\scripts\uninstall.ps1
```
