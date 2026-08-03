[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$PayloadRoot,

    [string]$VisualStudioInstallation
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$requiredRuntimeFiles = @(
    "vcruntime140.dll",
    "vcruntime140_1.dll"
)

function Resolve-VisualStudioInstallation {
    param([string]$ExplicitInstallation)

    if ($ExplicitInstallation) {
        return (Resolve-Path -LiteralPath $ExplicitInstallation).Path
    }

    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (!(Test-Path -LiteralPath $vswhere -PathType Leaf)) {
        throw "vswhere.exe was not found at $vswhere"
    }

    $installation = & $vswhere -latest -products * `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -property installationPath
    if ($LASTEXITCODE -ne 0 -or !$installation) {
        throw "Visual Studio C++ tools are required to stage the VC runtime"
    }

    return (Resolve-Path -LiteralPath $installation).Path
}

function Resolve-LatestRetailCrtDirectory {
    param([string]$Installation)

    $redistRoot = Join-Path $Installation "VC\Redist\MSVC"
    if (!(Test-Path -LiteralPath $redistRoot -PathType Container)) {
        throw "Visual C++ redistributable root was not found: $redistRoot"
    }

    $versionDirectories = Get-ChildItem -LiteralPath $redistRoot -Directory |
        Where-Object { $_.Name -match '^\d+\.\d+\.\d+$' } |
        Sort-Object { [version]$_.Name } -Descending

    foreach ($versionDirectory in $versionDirectories) {
        $x64Root = Join-Path $versionDirectory.FullName "x64"
        if (!(Test-Path -LiteralPath $x64Root -PathType Container)) {
            continue
        }
        $crtDirectory = Get-ChildItem -LiteralPath $x64Root -Directory -Filter "Microsoft.VC*.CRT" -ErrorAction SilentlyContinue |
            Sort-Object Name -Descending |
            Select-Object -First 1
        if ($crtDirectory) {
            return $crtDirectory
        }
    }

    throw "No retail x64 Microsoft.VC*.CRT directory was found under $redistRoot"
}

$payloadDirectory = New-Item -ItemType Directory -Force -Path $PayloadRoot
$installationPath = Resolve-VisualStudioInstallation -ExplicitInstallation $VisualStudioInstallation
$crtDirectory = Resolve-LatestRetailCrtDirectory -Installation $installationPath

$staged = @()
foreach ($fileName in $requiredRuntimeFiles) {
    $source = Join-Path $crtDirectory.FullName $fileName
    if (!(Test-Path -LiteralPath $source -PathType Leaf)) {
        throw "Required retail VC runtime file was not found: $source"
    }

    $signature = Get-AuthenticodeSignature -LiteralPath $source
    if ($signature.Status -ne "Valid" -or !$signature.SignerCertificate) {
        throw "VC runtime file does not have a valid Authenticode signature: $source"
    }
    if ($signature.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation') {
        throw "VC runtime file is not signed by Microsoft Corporation: $source"
    }

    $destination = Join-Path $payloadDirectory.FullName $fileName
    Copy-Item -LiteralPath $source -Destination $destination -Force
    $hash = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant()
    $staged += [ordered]@{
        name = $fileName
        version = (Get-Item -LiteralPath $destination).VersionInfo.FileVersion
        sha256 = $hash
    }
}

[ordered]@{
    result = "pass"
    architecture = "x64"
    redistributable_version = $crtDirectory.Parent.Parent.Name
    files = $staged
} | ConvertTo-Json -Depth 4
