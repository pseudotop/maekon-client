[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string[]] $Path,

    [ValidateSet("Report", "Required")]
    [string] $Mode = "Report",

    [string] $ExpectedPublisherSubject = "",

    [switch] $RequireTimestamp,

    [switch] $RequireSignTool,

    [string] $ReceiptPath = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if ($Mode -eq "Required" -and [string]::IsNullOrWhiteSpace($ExpectedPublisherSubject)) {
    throw "Required mode needs an exact ExpectedPublisherSubject value"
}

$timestampRequired = $RequireTimestamp -or $Mode -eq "Required"
$signToolRequired = $RequireSignTool -or $Mode -eq "Required"

function Find-SignTool {
    $command = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    $kitsRoot = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"
    if (-not (Test-Path -LiteralPath $kitsRoot)) {
        return $null
    }

    return Get-ChildItem -LiteralPath $kitsRoot -Filter signtool.exe -Recurse -File `
        -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match '[\\/]x64[\\/]signtool\.exe$' } |
        Sort-Object FullName -Descending |
        Select-Object -First 1 -ExpandProperty FullName
}

$resolvedPaths = @(
    $Path |
        ForEach-Object { (Resolve-Path -LiteralPath $_).Path } |
        Sort-Object -Unique
)
if ($resolvedPaths.Count -eq 0) {
    throw "No Authenticode artifacts were provided"
}

$signTool = Find-SignTool
if ($signToolRequired -and -not $signTool) {
    throw "signtool.exe is required but was not found"
}

$failures = [System.Collections.Generic.List[string]]::new()
$entries = [System.Collections.Generic.List[object]]::new()

foreach ($artifactPath in $resolvedPaths) {
    $signature = Get-AuthenticodeSignature -LiteralPath $artifactPath
    $publisher = if ($signature.SignerCertificate) {
        $signature.SignerCertificate.Subject
    } else {
        ""
    }
    $timestampSubject = if ($signature.TimeStamperCertificate) {
        $signature.TimeStamperCertificate.Subject
    } else {
        ""
    }
    $artifactFailures = [System.Collections.Generic.List[string]]::new()
    $signToolStatus = "not-run"

    switch ($signature.Status.ToString()) {
        "Valid" {
            if (
                -not [string]::IsNullOrWhiteSpace($ExpectedPublisherSubject) -and
                $publisher -cne $ExpectedPublisherSubject
            ) {
                $artifactFailures.Add(
                    "publisher mismatch: expected '$ExpectedPublisherSubject', got '$publisher'"
                )
            }
            if ($timestampRequired -and -not $signature.TimeStamperCertificate) {
                $artifactFailures.Add("RFC 3161 timestamp certificate is missing")
            }
            if ($signTool) {
                & $signTool verify /pa /all /v $artifactPath | Out-Host
                if ($LASTEXITCODE -ne 0) {
                    $signToolStatus = "failed"
                    $artifactFailures.Add("signtool verify failed with exit $LASTEXITCODE")
                } else {
                    $signToolStatus = "passed"
                }
            }
        }
        "NotSigned" {
            if ($Mode -eq "Required") {
                $artifactFailures.Add("artifact is not signed")
            }
        }
        default {
            $artifactFailures.Add("signature status is $($signature.Status)")
        }
    }

    foreach ($failure in $artifactFailures) {
        $failures.Add("$(Split-Path $artifactPath -Leaf): $failure")
    }

    $entries.Add([ordered]@{
        artifact = Split-Path $artifactPath -Leaf
        sha256 = (Get-FileHash -LiteralPath $artifactPath -Algorithm SHA256).Hash.ToLowerInvariant()
        status = $signature.Status.ToString()
        publisher_subject = $publisher
        timestamp_subject = $timestampSubject
        signtool = $signToolStatus
        passed = $artifactFailures.Count -eq 0
    })
}

$receipt = [ordered]@{
    schema_version = "maekon.windows-authenticode-receipt.v1"
    generated_at_utc = [DateTimeOffset]::UtcNow.ToString("o")
    mode = $Mode
    expected_publisher_subject = $ExpectedPublisherSubject
    require_timestamp = [bool] $timestampRequired
    require_signtool = [bool] $signToolRequired
    all_artifacts_signed = @($entries | Where-Object { $_.status -ne "Valid" }).Count -eq 0
    verification_passed = $failures.Count -eq 0
    artifacts = @($entries)
    failures = @($failures)
}

if (-not [string]::IsNullOrWhiteSpace($ReceiptPath)) {
    $receiptParent = Split-Path -Parent $ReceiptPath
    if ($receiptParent) {
        New-Item -ItemType Directory -Path $receiptParent -Force | Out-Null
    }
    # Windows PowerShell 5.1 does not expose the utf8NoBOM Set-Content encoding.
    # Use the .NET encoder so the receipt is byte-identical on 5.1 and PowerShell 7.
    $receiptJson = $receipt | ConvertTo-Json -Depth 6
    $utf8NoBom = [System.Text.UTF8Encoding]::new($false)
    [System.IO.File]::WriteAllText($ReceiptPath, $receiptJson, $utf8NoBom)
}

foreach ($entry in $entries) {
    Write-Host "Authenticode $($entry.status): $($entry.artifact)"
}

if ($failures.Count -gt 0) {
    throw "Authenticode verification failed: $($failures -join '; ')"
}

Write-Host "Authenticode verification completed in $Mode mode for $($entries.Count) artifact(s)"
