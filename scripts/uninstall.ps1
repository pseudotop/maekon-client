[CmdletBinding()]
param(
    [string]$InstallDir = $(if ($env:ONESHIM_INSTALL_DIR) { $env:ONESHIM_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "ONESHIM\bin" }),
    [switch]$KeepPath
)

$ErrorActionPreference = "Stop"

$BinaryName = "oneshim.exe"
$TargetBinary = Join-Path $InstallDir $BinaryName

function Write-Info {
    param([string]$Message)
    Write-Host "[INFO] $Message"
}

function Remove-InstallDirFromUserPath {
    param([string]$Directory)

    $currentUserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ([string]::IsNullOrWhiteSpace($currentUserPath)) {
        return
    }

    $normalizedTarget = $Directory.TrimEnd('\').ToLowerInvariant()
    $segments = $currentUserPath.Split(";") | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    $filtered = $segments | Where-Object { $_.TrimEnd('\').ToLowerInvariant() -ne $normalizedTarget }

    if (@($filtered).Count -eq @($segments).Count) {
        return
    }

    $newPath = ($filtered -join ";")
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    $env:Path = (($env:Path.Split(";") | Where-Object { $_.TrimEnd('\').ToLowerInvariant() -ne $normalizedTarget }) -join ";")
    Write-Info "Removed $Directory from user PATH"
}

if (Test-Path $TargetBinary) {
    Remove-Item -Path $TargetBinary -Force
    Write-Info "Removed $TargetBinary"
} else {
    Write-Info "No installed binary found at $TargetBinary"
}

if ((Test-Path $InstallDir) -and ((Get-ChildItem $InstallDir | Measure-Object).Count -eq 0)) {
    Remove-Item -Path $InstallDir -Force
    Write-Info "Removed empty directory $InstallDir"
}

if (-not $KeepPath) {
    Remove-InstallDirFromUserPath -Directory $InstallDir
}

Write-Info "Uninstall complete"
