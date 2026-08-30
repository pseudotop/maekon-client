$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$clientRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$verifier = Join-Path $PSScriptRoot "verify-windows-authenticode.ps1"
$policyPath = Join-Path $clientRoot "supply-chain\windows-authenticode-policy.json"
$temporaryRoot = Join-Path ([System.IO.Path]::GetTempPath()) (
    "maekon-authenticode-test-" + [Guid]::NewGuid().ToString("N")
)

function Expect-Failure {
    param(
        [Parameter(Mandatory = $true)]
        [scriptblock] $Action,
        [Parameter(Mandatory = $true)]
        [string] $Reason
    )

    $failed = $false
    try {
        & $Action
    } catch {
        $failed = $true
    }
    if (-not $failed) {
        throw "Expected failure was not observed: $Reason"
    }
}

New-Item -ItemType Directory -Path $temporaryRoot -Force | Out-Null
try {
    $policy = Get-Content -LiteralPath $policyPath -Raw | ConvertFrom-Json
    if ($policy.schema_version -ne "maekon.windows-authenticode-policy.v1") {
        throw "Unexpected Windows Authenticode policy schema"
    }
    if ($policy.enforcement_state -ne "prepared_not_active") {
        throw "Signing policy must remain explicit about its inactive identity blocker"
    }
    if (
        $policy.github.oidc_subject -ne
        "repo:pseudotop/maekon-client:environment:release-signing"
    ) {
        throw "OIDC subject must be bound to the protected public release environment"
    }
    if (@($policy.required_artifacts).Count -ne 5) {
        throw "Signing policy must cover two embedded executables and three installers"
    }

    $unsigned = Join-Path $temporaryRoot "unsigned.dll"
    Add-Type `
        -TypeDefinition 'public static class UnsignedFixture {}' `
        -Language CSharp `
        -OutputAssembly $unsigned `
        -OutputType Library
    $reportReceipt = Join-Path $temporaryRoot "report.json"
    & $verifier -Path $unsigned -Mode Report -ReceiptPath $reportReceipt
    $report = Get-Content -LiteralPath $reportReceipt -Raw | ConvertFrom-Json
    if ($report.verification_passed -ne $true -or $report.all_artifacts_signed -ne $false) {
        throw "Report mode must record unsigned state without claiming all artifacts are signed"
    }

    Expect-Failure -Reason "required mode accepted an unsigned executable" -Action {
        & $verifier `
            -Path $unsigned `
            -Mode Required `
            -ExpectedPublisherSubject "CN=Maekon Test" `
            -RequireTimestamp
    }
    Expect-Failure -Reason "required mode accepted an empty publisher contract" -Action {
        & $verifier -Path $unsigned -Mode Required
    }

    $gitCommand = Get-Command git.exe -ErrorAction Stop
    $signedSource = $gitCommand.Source
    $signed = Get-AuthenticodeSignature -LiteralPath $signedSource
    if ($signed.Status -ne "Valid" -or -not $signed.TimeStamperCertificate) {
        throw "Embedded signed-file positive control is unavailable"
    }
    & $verifier `
        -Path $signedSource `
        -Mode Required `
        -ExpectedPublisherSubject $signed.SignerCertificate.Subject `
        -RequireTimestamp
    Expect-Failure -Reason "required mode accepted the wrong publisher" -Action {
        & $verifier `
            -Path $signedSource `
            -Mode Required `
            -ExpectedPublisherSubject "CN=Unexpected Publisher" `
            -RequireTimestamp
    }

    $mutated = Join-Path $temporaryRoot "signed-mutated.exe"
    Copy-Item -LiteralPath $signedSource -Destination $mutated
    $stream = [System.IO.File]::Open($mutated, [System.IO.FileMode]::Append)
    try {
        $stream.WriteByte(0)
    } finally {
        $stream.Dispose()
    }
    Expect-Failure -Reason "modified signed executable passed verification" -Action {
        & $verifier `
            -Path $mutated `
            -Mode Required `
            -ExpectedPublisherSubject $signed.SignerCertificate.Subject `
            -RequireTimestamp
    }

    Write-Host "Windows Authenticode readiness tests passed"
} finally {
    Remove-Item -LiteralPath $temporaryRoot -Recurse -Force -ErrorAction SilentlyContinue
}
