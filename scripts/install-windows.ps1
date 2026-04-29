# Maekon Windows installer
# Copies the binary and registers Registry Run autostart.

$ErrorActionPreference = "Stop"

$BinaryName = "oneshim.exe"
$InstallDir = "$env:LOCALAPPDATA\ONESHIM"
$RegistryPath = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
$RegistryName = "ONESHIM"

# 릴리즈 바이너리 경로
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = Split-Path -Parent $ScriptDir
$BinaryPath = Join-Path $RepoRoot "target\release\$BinaryName"

if (-not (Test-Path $BinaryPath)) {
    Write-Error "릴리즈 바이너리가 없습니다: $BinaryPath"
    Write-Host "먼저 빌드하세요: cargo build --release -p oneshim-app"
    exit 1
}

# 설치 디렉토리 생성
Write-Host "[INFO] 설치 디렉토리 생성: $InstallDir"
New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null

# 바이너리 복사
Write-Host "[INFO] 바이너리 복사: $BinaryPath -> $InstallDir\$BinaryName"
Copy-Item -Path $BinaryPath -Destination "$InstallDir\$BinaryName" -Force
Write-Host "[OK] 바이너리 설치 완료"

# 레지스트리 등록 (로그인 시 자동 시작)
$ExePath = Join-Path $InstallDir $BinaryName
Write-Host "[INFO] 레지스트리 등록: $RegistryPath\$RegistryName"
Set-ItemProperty -Path $RegistryPath -Name $RegistryName -Value "`"$ExePath`""
Write-Host "[OK] Maekon 설치 완료! 다음 로그인 시 자동 시작됩니다."
