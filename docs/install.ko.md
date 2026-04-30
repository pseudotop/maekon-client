[English](./install.md) | [한국어](./install.ko.md)

# 설치 가이드

이 문서는 Maekon 릴리즈 바이너리를 터미널에서 설치하는 방법을 제공합니다.

호환성 메모: 릴리즈 파일명, 설치 스크립트명, `ONESHIM_*` 환경 변수,
`oneshim` CLI 명령은 설치 프로그램, 업데이터, 기존 사용자 호환성을 위해
현재 이름을 의도적으로 유지합니다.

## 빠른 설치

### macOS / Linux

```bash
curl -fsSL -o /tmp/oneshim-install.sh \
  https://raw.githubusercontent.com/pseudotop/oneshim-client/main/scripts/install.sh
bash /tmp/oneshim-install.sh
```

### Windows (PowerShell)

```powershell
$tmp = Join-Path $env:TEMP "oneshim-install.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/oneshim-client/main/scripts/install.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp
```

## 특정 버전 설치

### macOS / Linux

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
  - macOS/Linux: `--require-signature` 또는 `ONESHIM_REQUIRE_SIGNATURE=1`
  - Windows: `-RequireSignature`
- 서명 검증에는 설치 환경에 Python + PyNaCl이 필요합니다.
- 기본 업데이트 서명 공개키:
  - `GIdf7Wg4kvvvoT7jR0xwKLKna8hUR1kvowONbHbPz1E=`
- 키 로테이션 시 공개키 덮어쓰기:
  - `ONESHIM_UPDATE_PUBLIC_KEY=<base64-ed25519-public-key>`

## 스크립트 옵션

### macOS / Linux (`scripts/install.sh`)

```bash
bash /tmp/oneshim-install.sh --help
```

주요 옵션:

- `--version <tag>` (기본값: `latest`)
- `--install-dir <path>` (기본값: `~/.local/bin`)
- `--repo <owner/name>` (기본값: `pseudotop/oneshim-client`)
- `--base-url <url>` (릴리즈 에셋 소스 오버라이드; 로컬 smoke/rehearsal에 유용)
- `--require-signature`

### Windows (`scripts/install.ps1`)

```powershell
powershell -ExecutionPolicy Bypass -File $tmp -?
```

주요 파라미터:

- `-Version <tag>` (기본값: `latest`)
- `-InstallDir <path>` (기본값: `%LOCALAPPDATA%\ONESHIM\bin`)
- `-Repository <owner/name>` (기본값: `pseudotop/oneshim-client`)
- `-BaseUrl <url>` (릴리즈 에셋 소스 오버라이드; 로컬 smoke/rehearsal에 유용)
- `-RequireSignature`

## 제거

### macOS / Linux

```bash
curl -fsSL -o /tmp/oneshim-uninstall.sh \
  https://raw.githubusercontent.com/pseudotop/oneshim-client/main/scripts/uninstall.sh
bash /tmp/oneshim-uninstall.sh
```

### Windows

```powershell
$tmp = Join-Path $env:TEMP "oneshim-uninstall.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/oneshim-client/main/scripts/uninstall.ps1" `
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
