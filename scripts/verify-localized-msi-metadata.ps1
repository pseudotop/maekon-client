param(
    [Parameter(Mandatory = $true)]
    [string[]] $MsiPath
)

$ErrorActionPreference = "Stop"

if ($MsiPath.Count -ne 2) {
    throw "Expected exactly two localized MSI files, found $($MsiPath.Count)"
}

$expected = @{
    "en-US" = @{ Language = "1033"; TextPattern = "End-User License Agreement" }
    "ko-KR" = @{ Language = "1042"; TextPattern = "[가-힣]" }
}

$installer = New-Object -ComObject WindowsInstaller.Installer

function Read-MsiRows {
    param(
        [Parameter(Mandatory = $true)] $Database,
        [Parameter(Mandatory = $true)] [string] $Query
    )

    $view = $Database.GetType().InvokeMember(
        "OpenView", "InvokeMethod", $null, $Database, @($Query)
    )
    try {
        $view.GetType().InvokeMember("Execute", "InvokeMethod", $null, $view, $null) | Out-Null
        $rows = @()
        while ($true) {
            $record = $view.GetType().InvokeMember("Fetch", "InvokeMethod", $null, $view, $null)
            if (-not $record) { break }
            $fieldCount = $record.GetType().InvokeMember(
                "FieldCount", "GetProperty", $null, $record, $null
            )
            $values = for ($index = 1; $index -le $fieldCount; $index++) {
                $record.GetType().InvokeMember(
                    "StringData", "GetProperty", $null, $record, @($index)
                )
            }
            $rows += [pscustomobject]@{ Values = [string[]] $values }
        }
        return $rows
    }
    finally {
        $view.GetType().InvokeMember("Close", "InvokeMethod", $null, $view, $null) | Out-Null
    }
}

foreach ($culture in $expected.Keys) {
    $matches = @($MsiPath | Where-Object { (Split-Path $_ -Leaf) -like "*-$culture-*" })
    if ($matches.Count -ne 1) {
        throw "Expected exactly one $culture MSI file, found $($matches.Count)"
    }

    $resolved = (Resolve-Path -LiteralPath $matches[0]).Path
    $database = $installer.GetType().InvokeMember(
        "OpenDatabase", "InvokeMethod", $null, $installer, @($resolved, 0)
    )
    $languageRows = @(Read-MsiRows -Database $database -Query (
        "SELECT ``Value`` FROM ``Property`` WHERE ``Property`` = 'ProductLanguage'"
    ))
    if ($languageRows.Count -ne 1 -or $languageRows[0].Values[0] -ne $expected[$culture].Language) {
        throw "$culture MSI ProductLanguage mismatch: expected $($expected[$culture].Language)"
    }

    $styles = @(Read-MsiRows -Database $database -Query (
        "SELECT ``TextStyle``, ``FaceName``, ``Size`` FROM ``TextStyle``"
    ))
    $styleMap = @{}
    foreach ($style in $styles) {
        $styleMap[$style.Values[0]] = @($style.Values[1], $style.Values[2])
    }
    foreach ($contract in @(
        @("WixUI_Font_Normal", "Segoe UI", "9"),
        @("WixUI_Font_Bigger", "Segoe UI", "12"),
        @("WixUI_Font_Title", "Segoe UI", "10")
    )) {
        $actual = $styleMap[$contract[0]]
        if (-not $actual -or $actual[0] -ne $contract[1] -or $actual[1] -ne $contract[2]) {
            throw "$culture MSI text style mismatch for $($contract[0])"
        }
    }

    $controlRows = @(Read-MsiRows -Database $database -Query (
        "SELECT ``Text`` FROM ``Control`` WHERE ``Dialog_`` = 'LicenseAgreementDlg' " +
        "OR ``Dialog_`` = 'VerifyReadyDlg' OR ``Dialog_`` = 'WelcomeDlg'"
    ))
    $controlText = ($controlRows | ForEach-Object { $_.Values[0] }) -join "`n"
    if ($controlText -notmatch $expected[$culture].TextPattern) {
        throw "$culture MSI does not contain the expected localized wizard text"
    }

    Write-Host "Verified $culture MSI metadata: ProductLanguage, Segoe UI, localized controls"
}
