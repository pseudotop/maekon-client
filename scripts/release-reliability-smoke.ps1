[CmdletBinding()]
param(
    [string]$AssetsDir = $(if ($env:MAEKON_SMOKE_ASSETS_DIR) { $env:MAEKON_SMOKE_ASSETS_DIR } else { "dist" }),
    [string]$InstallScript = "",
    [string]$AssetName = $(if ($env:MAEKON_SMOKE_ASSET_NAME) { $env:MAEKON_SMOKE_ASSET_NAME } else { "maekon-windows-x64.zip" }),
    [string]$InstallDir = $(if ($env:MAEKON_SMOKE_INSTALL_DIR) { $env:MAEKON_SMOKE_INSTALL_DIR } else { "" }),
    [string]$ListenHost = $(if ($env:MAEKON_SMOKE_HOST) { $env:MAEKON_SMOKE_HOST } else { "127.0.0.1" }),
    [int]$Port = $(if ($env:MAEKON_SMOKE_PORT) { [int]$env:MAEKON_SMOKE_PORT } else { 18091 }),
    [bool]$RequireSignature = $(if ($env:MAEKON_SMOKE_REQUIRE_SIGNATURE) { $env:MAEKON_SMOKE_REQUIRE_SIGNATURE -ne "0" } else { $false }),
    [switch]$SkipUpdaterTests
)

$ErrorActionPreference = "Stop"
$repoRoot = if ($env:MAEKON_SMOKE_REPO_ROOT) {
    Resolve-Path $env:MAEKON_SMOKE_REPO_ROOT
} else {
    Resolve-Path (Join-Path $PSScriptRoot "..")
}

if ([string]::IsNullOrWhiteSpace($InstallScript)) {
    if ($env:MAEKON_INSTALL_SCRIPT) {
        $InstallScript = $env:MAEKON_INSTALL_SCRIPT
    } else {
        $InstallScript = Join-Path $repoRoot "scripts/install.ps1"
    }
}

function Write-Info {
    param([string]$Message)
    Write-Host "[SMOKE] $Message"
}

function Throw-IfMissing {
    param(
        [string]$Path,
        [string]$Label
    )
    if (!(Test-Path $Path)) {
        throw "$Label not found: $Path"
    }
}

Throw-IfMissing -Path $AssetsDir -Label "Asset directory"
Throw-IfMissing -Path $InstallScript -Label "Install script"

$artifactPath = Join-Path $AssetsDir $AssetName
$checksumPath = "$artifactPath.sha256"
$signaturePath = "$artifactPath.sig"
Throw-IfMissing -Path $artifactPath -Label "Artifact"
Throw-IfMissing -Path $checksumPath -Label "Checksum"
if ($RequireSignature) {
    Throw-IfMissing -Path $signaturePath -Label "Signature"
}

$pythonCommand = Get-Command python3 -ErrorAction SilentlyContinue
if (-not $pythonCommand) {
    $pythonCommand = Get-Command python -ErrorAction SilentlyContinue
}
if (-not $pythonCommand) {
    throw "Python is required to host local release assets"
}

$tempDir = Join-Path $env:TEMP ("maekon-release-smoke-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tempDir -Force | Out-Null
if ([string]::IsNullOrWhiteSpace($InstallDir)) {
    $InstallDir = Join-Path $tempDir "bin"
}

$serverProcess = $null
Push-Location $repoRoot
try {
    Write-Info "Serving assets from $AssetsDir on http://$ListenHost`:$Port"
    $serverProcess = Start-Process `
        -FilePath $pythonCommand.Source `
        -ArgumentList @("-m", "http.server", "$Port", "--bind", "$ListenHost") `
        -WorkingDirectory $AssetsDir `
        -PassThru `
        -WindowStyle Hidden

    # Wait for HTTP server to start listening (up to 10 seconds)
    $maxWait = 20
    $waited = 0
    $ready = $false
    while ($waited -lt $maxWait) {
        if ($serverProcess.HasExited) {
            throw "Failed to start local HTTP server"
        }
        try {
            $tcp = New-Object System.Net.Sockets.TcpClient
            $tcp.Connect($ListenHost, $Port)
            $tcp.Close()
            $ready = $true
            break
        } catch {
            Start-Sleep -Milliseconds 500
            $waited++
        }
    }
    if (-not $ready) {
        throw "HTTP server not listening on ${ListenHost}:${Port} within 10 seconds"
    }

    $baseUrl = "http://$ListenHost`:$Port"
    Write-Info "Running installer against local base URL"
    $installArgs = @(
        "-ExecutionPolicy", "Bypass",
        "-File", $InstallScript,
        "-InstallDir", $InstallDir,
        "-BaseUrl", $baseUrl
    )
    if ($RequireSignature) {
        $installArgs += "-RequireSignature"
    } else {
        $installArgs += "-AllowUnsigned"
    }
    & powershell @installArgs

    $target = Join-Path $InstallDir "maekon.exe"
    Throw-IfMissing -Path $target -Label "Installed binary"
    $sidecarTarget = Join-Path $InstallDir "maekon-sandbox-worker.exe"
    Throw-IfMissing -Path $sidecarTarget -Label "Installed sandbox worker sidecar"

    Write-Info "Validating binary format"
    if (!(Test-Path $target)) {
        throw "Installed binary not found: $target"
    }
    $fileSize = (Get-Item $target).Length
    if ($fileSize -lt 1MB) {
        throw "Binary too small ($fileSize bytes), likely corrupt: $target"
    }
    Write-Info "Binary OK: $target ($([math]::Round($fileSize / 1MB, 1)) MB)"

    if (-not $SkipUpdaterTests) {
        Write-Info "Running updater reliability regression tests"
        $bashCommand = Get-Command bash -ErrorAction SilentlyContinue
        if ($bashCommand) {
            & $bashCommand.Source -lc "./scripts/cargo-cache.sh test -p maekon-app release_reliability_ -- --nocapture"
        } else {
            & cargo test -p maekon-app release_reliability_ -- --nocapture
        }
    }

    Write-Info "Release reliability smoke completed"
} finally {
    if ($serverProcess -and -not $serverProcess.HasExited) {
        Stop-Process -Id $serverProcess.Id -Force -ErrorAction SilentlyContinue
    }
    Remove-Item -Path $tempDir -Recurse -Force -ErrorAction SilentlyContinue
    Pop-Location
}
