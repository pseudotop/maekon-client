[CmdletBinding()]
param(
    [string]$Version = $(if ($env:ONESHIM_VERSION) { $env:ONESHIM_VERSION } else { "latest" }),
    [string]$Repository = $(if ($env:ONESHIM_REPOSITORY) { $env:ONESHIM_REPOSITORY } else { "pseudotop/maekon-client" }),
    [string]$InstallDir = $(if ($env:ONESHIM_INSTALL_DIR) { $env:ONESHIM_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "ONESHIM\bin" }),
    [string]$BaseUrl = $(if ($env:ONESHIM_RELEASE_BASE_URL) { $env:ONESHIM_RELEASE_BASE_URL } else { "" }),
    [switch]$RequireSignature
)

$ErrorActionPreference = "Stop"

$BinaryName = "oneshim.exe"
$SidecarName = "oneshim-sandbox-worker.exe"
$AssetName = "oneshim-windows-x64.zip"
$PublicKeyB64 = if ($env:ONESHIM_UPDATE_PUBLIC_KEY) { $env:ONESHIM_UPDATE_PUBLIC_KEY } else { "GIdf7Wg4kvvvoT7jR0xwKLKna8hUR1kvowONbHbPz1E=" }

function Write-Info {
    param([string]$Message)
    Write-Host "[INFO] $Message"
}

function Write-WarnLine {
    param([string]$Message)
    Write-Warning $Message
}

function Write-CompletionBanner {
    $candidatePaths = @()
    if (-not [string]::IsNullOrWhiteSpace($PSScriptRoot)) {
        $candidatePaths += (Join-Path $PSScriptRoot "..\assets\brand\cli-banner.txt")
    }
    $candidatePaths += "assets\brand\cli-banner.txt"

    Write-Host ""
    $bannerPath = $candidatePaths | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
    if ($bannerPath) {
        Get-Content -Path $bannerPath | ForEach-Object { Write-Host $_ }
    } else {
        Write-Host "Maekon"
        Write-Host "local context agent"
    }
    Write-Host ""
}

function Get-NormalizedTag {
    param([string]$InputVersion)
    if ($InputVersion -eq "latest") {
        return "latest"
    }
    if ($InputVersion.StartsWith("v")) {
        return $InputVersion
    }
    return "v$InputVersion"
}

function Invoke-Download {
    param(
        [string]$Url,
        [string]$OutFile
    )
    Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $OutFile -ErrorAction Stop
}

function Assert-Checksum {
    param(
        [string]$ArchivePath,
        [string]$ChecksumPath
    )

    $firstLine = Get-Content -Path $ChecksumPath | Select-Object -First 1
    if ([string]::IsNullOrWhiteSpace($firstLine)) {
        throw "Checksum file is empty: $ChecksumPath"
    }

    $expected = ($firstLine -split '\s+')[0].ToLowerInvariant()
    $actual = (Get-FileHash -Path $ArchivePath -Algorithm SHA256).Hash.ToLowerInvariant()

    if ($expected -ne $actual) {
        throw "Checksum mismatch. expected=$expected actual=$actual"
    }
}

function Test-Signature {
    param(
        [string]$PayloadPath,
        [string]$SignaturePath,
        [string]$PublicKey,
        [switch]$Required
    )

    $pythonCommand = Get-Command python3 -ErrorAction SilentlyContinue
    if (-not $pythonCommand) {
        $pythonCommand = Get-Command python -ErrorAction SilentlyContinue
    }

    if (-not $pythonCommand) {
        if ($Required) {
            throw "Python is required for signature verification."
        }
        Write-WarnLine "Python is not available. Skipping signature verification."
        return
    }

    $scriptPath = Join-Path $env:TEMP ("oneshim-verify-signature-" + [Guid]::NewGuid().ToString("N") + ".py")
    @'
import base64
import sys
from pathlib import Path

payload_path = Path(sys.argv[1])
sig_path = Path(sys.argv[2])
pubkey_b64 = sys.argv[3].split()[0]

try:
    from nacl.exceptions import BadSignatureError
    from nacl.signing import VerifyKey
except Exception as exc:
    print(f"PyNaCl is required for signature verification: {exc}", file=sys.stderr)
    sys.exit(10)

sig_tokens = sig_path.read_text(encoding="utf-8").split()
if not sig_tokens:
    print("Signature file is empty", file=sys.stderr)
    sys.exit(11)

signature = base64.b64decode(sig_tokens[0])
public_key = base64.b64decode(pubkey_b64)

if len(public_key) != 32:
    print(f"Invalid public key length: {len(public_key)} (expected 32)", file=sys.stderr)
    sys.exit(12)

if len(signature) != 64:
    print(f"Invalid signature length: {len(signature)} (expected 64)", file=sys.stderr)
    sys.exit(13)

payload = payload_path.read_bytes()

try:
    VerifyKey(public_key).verify(payload, signature)
except BadSignatureError:
    print("Signature verification failed", file=sys.stderr)
    sys.exit(14)
'@ | Set-Content -Path $scriptPath -Encoding UTF8

    & $pythonCommand.Source $scriptPath $PayloadPath $SignaturePath $PublicKey
    $exitCode = $LASTEXITCODE
    Remove-Item -Path $scriptPath -ErrorAction SilentlyContinue

    if ($exitCode -ne 0) {
        if ($Required) {
            throw "Signature verification failed or PyNaCl is missing."
        }
        Write-WarnLine "Signature verification skipped (PyNaCl missing or verification failed)."
        return
    }

    Write-Info "Ed25519 signature verification passed"
}

function Add-InstallDirToUserPath {
    param([string]$Directory)

    $currentUserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $segments = @()
    if (-not [string]::IsNullOrWhiteSpace($currentUserPath)) {
        $segments = $currentUserPath.Split(";") | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    }

    $normalizedTarget = $Directory.TrimEnd('\').ToLowerInvariant()
    $exists = $segments | Where-Object { $_.TrimEnd('\').ToLowerInvariant() -eq $normalizedTarget }

    if ($exists) {
        return
    }

    $updatedPath = if ([string]::IsNullOrWhiteSpace($currentUserPath)) {
        $Directory
    } else {
        "$currentUserPath;$Directory"
    }

    [Environment]::SetEnvironmentVariable("Path", $updatedPath, "User")
    $sessionSegments = $env:Path.Split(";") | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    $sessionMatch = $sessionSegments | Where-Object { $_.TrimEnd('\').ToLowerInvariant() -eq $normalizedTarget }
    if (-not $sessionMatch) {
        $env:Path = "$env:Path;$Directory"
    }
    Write-Info "Added $Directory to user PATH"
}

# Check WebView2 runtime
$wv2Key = "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BEF-ED47D884521F}"
$wv2 = Get-ItemProperty -Path $wv2Key -ErrorAction SilentlyContinue
if (-not $wv2) {
    Write-WarnLine "WebView2 runtime not found. The NSIS installer will install it automatically."
    Write-WarnLine "If installing from zip, download the bootstrapper from:"
    Write-WarnLine "  https://go.microsoft.com/fwlink/p/?LinkId=2124703"
}

$tag = Get-NormalizedTag -InputVersion $Version
$baseUrl = if ([string]::IsNullOrWhiteSpace($BaseUrl)) {
    if ($tag -eq "latest") {
        "https://github.com/$Repository/releases/latest/download"
    } else {
        "https://github.com/$Repository/releases/download/$tag"
    }
} else {
    $BaseUrl.TrimEnd("/")
}

$tempDir = Join-Path $env:TEMP ("oneshim-install-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tempDir -Force | Out-Null

try {
    $archivePath = Join-Path $tempDir $AssetName
    $checksumPath = "$archivePath.sha256"
    $signaturePath = "$archivePath.sig"
    $extractDir = Join-Path $tempDir "extract"

    Write-Info "Repository: $Repository"
    Write-Info "Version: $tag"
    Write-Info "Asset: $AssetName"
    Write-Info "Install dir: $InstallDir"
    Write-Info "Base URL: $baseUrl"

    Invoke-Download -Url "$baseUrl/$AssetName" -OutFile $archivePath
    Invoke-Download -Url "$baseUrl/$AssetName.sha256" -OutFile $checksumPath

    Write-Info "Verifying SHA-256 checksum"
    Assert-Checksum -ArchivePath $archivePath -ChecksumPath $checksumPath
    Write-Info "Checksum verification passed"

    $signatureDownloaded = $true
    try {
        Invoke-Download -Url "$baseUrl/$AssetName.sig" -OutFile $signaturePath
    } catch {
        $signatureDownloaded = $false
    }

    if ($signatureDownloaded) {
        Test-Signature -PayloadPath $archivePath -SignaturePath $signaturePath -PublicKey $PublicKeyB64 -Required:$RequireSignature
    } elseif ($RequireSignature) {
        throw "Failed to download signature file while -RequireSignature is enabled."
    } else {
        Write-WarnLine "Signature file is not available. Continuing because -RequireSignature is not enabled."
    }

    New-Item -ItemType Directory -Path $extractDir -Force | Out-Null
    Expand-Archive -Path $archivePath -DestinationPath $extractDir -Force

    $binary = Get-ChildItem -Path $extractDir -Filter $BinaryName -File -Recurse | Select-Object -First 1
    if (-not $binary) {
        throw "Could not locate $BinaryName inside archive."
    }
    $sidecar = Get-ChildItem -Path $extractDir -Filter $SidecarName -File -Recurse | Select-Object -First 1

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    $target = Join-Path $InstallDir $BinaryName
    Copy-Item -Path $binary.FullName -Destination $target -Force
    if ($sidecar) {
        $sidecarTarget = Join-Path $InstallDir $SidecarName
        Copy-Item -Path $sidecar.FullName -Destination $sidecarTarget -Force
        Write-Info "Installed sandbox worker: $sidecarTarget"
    } else {
        Write-WarnLine "Sandbox worker sidecar was not found in the archive; sandboxed automation actions may be unavailable."
    }

    Add-InstallDirToUserPath -Directory $InstallDir

    Write-Info "Installed: $target"
    Write-CompletionBanner
    Write-Info "Open a new terminal and run: oneshim"
} finally {
    Remove-Item -Path $tempDir -Recurse -Force -ErrorAction SilentlyContinue
}
