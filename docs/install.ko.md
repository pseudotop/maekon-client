[English](./install.md) | [한국어](./install.ko.md)

# 설치 가이드

이 문서는 Maekon 릴리즈 바이너리를 터미널에서 설치하는 방법을 제공합니다.

> 이 source snapshot의 prerelease 대상은 `v0.0.1-rc.12`입니다. 같은 GitHub
> Release에 자산이 게시된 뒤에만 아래 버전 고정 명령을 실행하세요. GitHub의
> `latest` stable 다운로드 URL은 첫 stable 릴리즈 전까지 사용할 수 없습니다.

호환성 메모: 릴리즈 파일명, 설치 스크립트명, `MAEKON_*` 환경 변수,
`maekon` CLI 명령은 설치 프로그램, 업데이터, 기존 사용자 호환성을 위해
현재 이름을 의도적으로 유지합니다.

## 릴리즈 후보 설치 (게시 후)

### macOS / Linux

```bash
curl -fsSL -o /tmp/maekon-install.sh \
  https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.sh
MAEKON_VERSION=v0.0.1-rc.12 bash /tmp/maekon-install.sh --require-signature
```

### Windows (PowerShell)

```powershell
$tmp = Join-Path $env:TEMP "maekon-install.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp -Version v0.0.1-rc.12 -RequireSignature
```

## 최신 stable 설치

첫 stable 릴리즈가 게시된 뒤에는 버전을 고정하지 않은 설치 명령이 GitHub의
`latest` stable 릴리즈를 기본값으로 사용합니다.

### macOS / Linux

```bash
curl -fsSL https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.sh | bash -s -- --require-signature
```

### Windows

```powershell
$tmp = Join-Path $env:TEMP "maekon-install.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp -RequireSignature
```

## 특정 버전 설치

### macOS / Linux

```bash
curl -fsSL -o /tmp/maekon-install.sh \
  https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.sh
MAEKON_VERSION=v0.0.1-rc.12 bash /tmp/maekon-install.sh --require-signature
```

### Windows

```powershell
$tmp = Join-Path $env:TEMP "maekon-install.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp -Version v0.0.1-rc.12 -RequireSignature
```

## 무결성 검증

- `scripts/install.sh`, `scripts/install.ps1`는 릴리즈 사이드카(`.sha256`)를 사용해 `SHA-256`을 항상 검증합니다.
- Ed25519 서명 검증(`.sig`)은 공개 설치에서 기본적으로 강제됩니다.
  - macOS/Linux: `--require-signature` 또는 기본값 `MAEKON_REQUIRE_SIGNATURE=1`
  - Windows: `-RequireSignature` 또는 기본값 `MAEKON_REQUIRE_SIGNATURE=1`
- 개발/테스트용 unsigned rehearsal은 명시적으로 opt-out 해야 합니다.
  - macOS/Linux: `--allow-unsigned` 또는 `MAEKON_REQUIRE_SIGNATURE=0`
  - Windows: `-AllowUnsigned` 또는 `MAEKON_REQUIRE_SIGNATURE=0`
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
- `--require-signature` (공개 설치 기본 동작)
- `--allow-unsigned` (개발/테스트 override 전용)

### Windows (`scripts/install.ps1`)

```powershell
powershell -ExecutionPolicy Bypass -File $tmp -?
```

주요 파라미터:

- `-Version <tag>` (기본값: `latest`)
- `-InstallDir <path>` (기본값: `%LOCALAPPDATA%\MAEKON\bin`)
- `-Repository <owner/name>` (기본값: `pseudotop/maekon-client`)
- `-BaseUrl <url>` (릴리즈 에셋 소스 오버라이드; 로컬 smoke/rehearsal에 유용)
- `-RequireSignature` (공개 설치 기본 동작)
- `-AllowUnsigned` (개발/테스트 override 전용)

## 제거

제거 후 다시 설치해도 데이터를 잃지 않도록 사용자 프로필과 자격 증명을 보존합니다
(#11065의 선택 3). 패키지 제거는 설치된 프로그램 파일을 지우며, Debian `purge`는 사용자
데이터가 남았다는 안내도 출력합니다. 패키지 스크립트는 다른 사용자의 홈을 검색하거나
키를 삭제하지 않습니다. 별도의 프로필 완전 삭제 명령은 제공하지 않습니다.

플레이버가 없는 기본 프로필 경로:

| 플랫폼 | 설정 | 데이터 |
| --- | --- | --- |
| Linux | `~/.config/maekon` | `~/.local/share/maekon` |
| Windows | `%APPDATA%\maekon` | `%LOCALAPPDATA%\maekon\data` |
| macOS | `~/Library/Application Support/maekon` | `~/Library/Application Support/maekon/data` |

Linux rc.11부터 DB 키의 영속 사본은 데이터 디렉터리의 소유자 전용 `.db_key` 파일이며,
커널 키링은 캐시입니다. macOS Keychain과 Windows Credential Manager의 동작은 그대로입니다.
공급자 자격 증명은 명시적으로 설정한 별도 파일을 사용할 수도 있습니다. 플레이버 빌드는
`maekon-<flavor>` 경로와 자격 증명 네임스페이스를 사용합니다.

프로필 파일을 직접 지우려면 먼저 Maekon을 종료하고 보관할 백업을 확인하세요. Linux의
`.db_key`를 지우면 남겨 둔 암호화 DB 사본을 읽지 못할 수 있습니다. 프로필 디렉터리 삭제는
OS 자격 증명 항목, 별도로 설정한 파일이나 백업의 삭제 및 안전한 완전 삭제를 입증하지
않습니다. Linux 커널 키링 항목은 만료 또는 재부팅까지 남을 수 있습니다. `secret-tool`은
다른 백엔드의 도구이므로 이 항목들의 정리 명령이 아닙니다.

설치 검증 레인은 앱이 정지한 상태의 프로필 파일을 패키지 제거 전후로 비교합니다.
그 뒤 일회용 테스트 프로필을 정리하는 단계는 파일시스템에서 직접 지울 수 있는지를
별도로 측정하며, 자격 증명 저장소나 백업의 삭제를 입증하지 않습니다. #8685의 수락 기준
7에도 이 구분을 적용합니다.

깨끗한 QEMU VM 검증은 Mesa `softpipe`를 명시합니다. 기본 `llvmpipe` 렌더러는
소프트웨어 CPU 에뮬레이션에서 AVX 관련 SIGSEGV가 재현됩니다(#13268). 제거 대조는
같은 릴리스에서 렌더러 지정을 해제하며, 두 실행 모두 기본 WebKit sandbox와 Ubuntu의
사용자 네임스페이스 제한을 유지합니다. 두 실행 모두 폐기용 VM 안에서만 코어 덤프 수집을
끄고 적용값을 다시 읽어 기록합니다. 코어 수집기의 정지 때문에 실제 종료 signal 관측이
막히는 것을 방지합니다. 영수증은 실제 앱 PID·종료 signal, HTTP 시간과
런타임 로그를 포함합니다. VM 통과를 하드웨어 GPU나 일반 Linux 데스크톱 전체의 검증으로
확대하지 않습니다.

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
MAEKON_VERSION=v0.0.1-rc.12 ./scripts/install.sh
./scripts/uninstall.sh
```

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\install.ps1 -Version v0.0.1-rc.12
powershell -ExecutionPolicy Bypass -File .\scripts\uninstall.ps1
```
