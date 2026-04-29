# Maekon Windows uninstaller
# Removes Registry Run autostart and the installed binary.

$ErrorActionPreference = "Stop"

$BinaryName = "oneshim.exe"
$InstallDir = "$env:LOCALAPPDATA\ONESHIM"
$RegistryPath = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
$RegistryName = "ONESHIM"

# 레지스트리 삭제
if (Get-ItemProperty -Path $RegistryPath -Name $RegistryName -ErrorAction SilentlyContinue) {
    Write-Host "[INFO] 레지스트리 항목 삭제: $RegistryName"
    Remove-ItemProperty -Path $RegistryPath -Name $RegistryName -Force
    Write-Host "[OK] 레지스트리 삭제 완료"
} else {
    Write-Host "[INFO] 레지스트리 항목 없음 (이미 제거됨)"
}

# 바이너리 삭제
if (Test-Path "$InstallDir\$BinaryName") {
    Write-Host "[INFO] 바이너리 삭제: $InstallDir\$BinaryName"
    Remove-Item -Path "$InstallDir\$BinaryName" -Force
    Write-Host "[OK] 바이너리 삭제 완료"
} else {
    Write-Host "[INFO] 바이너리 없음 (이미 제거됨)"
}

# 설치 디렉토리 삭제 (비어 있으면)
if ((Test-Path $InstallDir) -and ((Get-ChildItem $InstallDir | Measure-Object).Count -eq 0)) {
    Remove-Item -Path $InstallDir -Force
    Write-Host "[OK] 설치 디렉토리 삭제 완료"
}

Write-Host "[OK] Maekon 제거 완료!"
