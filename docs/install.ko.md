[English](./install.md) | [한국어](./install.ko.md)

# 설치 가이드

이 문서는 Maekon 릴리즈 바이너리를 터미널에서 설치하는 방법을 제공합니다.

> 현재 공개 바이너리 릴리즈는 prerelease로 게시된 `v0.0.1-rc.5`입니다.
> GitHub의 `latest` stable 다운로드 URL은 첫 stable 릴리즈 전까지 사용할 수
> 없으므로, 현재 prerelease 설치는 버전을 명시적으로 고정해야 합니다.

호환성 메모: 릴리즈 파일명, 설치 스크립트명, `MAEKON_*` 환경 변수,
`maekon` CLI 명령은 설치 프로그램, 업데이터, 기존 사용자 호환성을 위해
현재 이름을 의도적으로 유지합니다.

## 현재 prerelease 설치

### macOS / Linux

```bash
curl -fsSL -o /tmp/maekon-install.sh \
  https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.sh
MAEKON_VERSION=v0.0.1-rc.5 bash /tmp/maekon-install.sh
```

### Windows (PowerShell)

```powershell
$tmp = Join-Path $env:TEMP "maekon-install.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp -Version v0.0.1-rc.5
```

## 최신 stable 설치

첫 stable 릴리즈가 게시된 뒤에는 버전을 고정하지 않은 설치 명령이 GitHub의
`latest` stable 릴리즈를 기본값으로 사용합니다.

### macOS / Linux

```bash
curl -fsSL https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.sh | bash
```

### Windows

```powershell
irm https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.ps1 | iex
```

## 특정 버전 설치

### macOS / Linux

```bash
curl -fsSL -o /tmp/maekon-install.sh \
  https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.sh
MAEKON_VERSION=v0.0.1-rc.5 bash /tmp/maekon-install.sh
```

### Windows

```powershell
$tmp = Join-Path $env:TEMP "maekon-install.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp -Version v0.0.1-rc.5
```

## 무결성 검증

- `scripts/install.sh`, `scripts/install.ps1`는 릴리즈 사이드카(`.sha256`)를 사용해 `SHA-256`을 항상 검증합니다.
- Ed25519 서명 검증(`.sig`)도 지원하며 강제할 수 있습니다.
  - macOS/Linux: `--require-signature` 또는 `MAEKON_REQUIRE_SIGNATURE=1`
  - Windows: `-RequireSignature`
- 서명 검증에는 설치 환경에 Python + PyNaCl이 필요합니다.
- 기본 업데이트 서명 공개키:
  - `fPiU9KchUIXZ7qOcjJIVp+W8rsO/WI7yStD+AiNuYvw=`
- 키 로테이션 시 공개키 덮어쓰기:
  - `MAEKON_UPDATE_PUBLIC_KEY=<base64-ed25519-public-key>`

## 스크립트 옵션

### macOS / Linux (`scripts/install.sh`)

```bash
bash /tmp/maekon-install.sh --help
```

주요 옵션:

- `--version <tag>` (기본값: `latest`)
- `--install-dir <path>` (기본값: `~/.local/bin`)
- `--repo <owner/name>` (기본값: `pseudotop/maekon-client`)
- `--base-url <url>` (릴리즈 에셋 소스 오버라이드; 로컬 smoke/rehearsal에 유용)
- `--require-signature`

### Windows (`scripts/install.ps1`)

```powershell
powershell -ExecutionPolicy Bypass -File $tmp -?
```

주요 파라미터:

- `-Version <tag>` (기본값: `latest`)
- `-InstallDir <path>` (기본값: `%LOCALAPPDATA%\MAEKON\bin`)
- `-Repository <owner/name>` (기본값: `pseudotop/maekon-client`)
- `-BaseUrl <url>` (릴리즈 에셋 소스 오버라이드; 로컬 smoke/rehearsal에 유용)
- `-RequireSignature`

## 제거

### macOS / Linux

```bash
curl -fsSL -o /tmp/maekon-uninstall.sh \
  https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/uninstall.sh
bash /tmp/maekon-uninstall.sh
```

### Windows

```powershell
$tmp = Join-Path $env:TEMP "maekon-uninstall.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/uninstall.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp
```

## 로컬 저장소에서 실행

이미 저장소를 clone한 경우:

```bash
MAEKON_VERSION=v0.0.1-rc.5 ./scripts/install.sh
./scripts/uninstall.sh
```

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\install.ps1 -Version v0.0.1-rc.5
powershell -ExecutionPolicy Bypass -File .\scripts\uninstall.ps1
```
