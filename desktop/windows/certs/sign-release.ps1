<#
Signs an ALREADY-BUILT .msi/.exe/.dll with the internal self-signed
code-signing certificate.

build-signed.ps1 covers the normal path: it signs during `cargo tauri build`.
This script covers the case that one cannot — an artifact that already exists
and was produced without a certificate, typically an MSI from CI (the Gitea
runner is Linux-only today, and any future Windows runner will not hold this
machine's private key).

Signs from the certificate store by thumbprint, the same way build-signed.ps1
does, rather than from the .pfx. The pfx route needs its password, which means
either an interactive prompt (unusable from a script or CI step) or the
password sitting somewhere on disk. The cert is already in
Cert:\CurrentUser\My on any machine that ran generate-selfsigned-cert.ps1.

Usage:
    .\sign-release.ps1 -Path ..\..\..\target\release\bundle\msi\*.msi
    .\sign-release.ps1 -Path a.msi,b.exe -Thumbprint ABC123...

Reminder: this signature is only trusted on machines where
import-trusted-root.ps1 has been run. It removes the "unknown publisher"
warning there and nowhere else — it does not buy SmartScreen reputation.
#>

param(
    [Parameter(Mandatory = $true)]
    [string[]]$Path,

    # Defaults to the thumbprint generate-selfsigned-cert.ps1 recorded.
    [string]$Thumbprint,

    [string]$TimestampUrl = 'http://timestamp.digicert.com'
)

$ErrorActionPreference = 'Stop'

if (-not $Thumbprint) {
    $thumbprintPath = Join-Path $PSScriptRoot 'output\thumbprint.txt'
    if (-not (Test-Path $thumbprintPath)) {
        throw "No -Thumbprint given and no $thumbprintPath. Run generate-selfsigned-cert.ps1 first, or pass -Thumbprint explicitly."
    }
    $Thumbprint = (Get-Content $thumbprintPath -Raw).Trim()
}

$cert = Get-ChildItem 'Cert:\CurrentUser\My' | Where-Object { $_.Thumbprint -eq $Thumbprint }
if (-not $cert) {
    throw "Certificate $Thumbprint is not in Cert:\CurrentUser\My. Re-run generate-selfsigned-cert.ps1 on this machine, or import the .pfx here first."
}

$files = Get-ChildItem -Path $Path -ErrorAction Stop
if (-not $files) { throw "No files matched -Path $($Path -join ', ')" }

Write-Host '==> Locating signtool.exe'
# Sort by parsed version, not by string: "10.0.9…" would otherwise sort above
# "10.0.26100.…" and pick an ancient SDK.
$signtool = Get-ChildItem -Path 'C:\Program Files (x86)\Windows Kits\10\bin\*\x64\signtool.exe' -ErrorAction SilentlyContinue |
    Sort-Object { [version]($_.Directory.Parent.Name) } -Descending |
    Select-Object -First 1 -ExpandProperty FullName
if (-not $signtool) {
    throw "signtool.exe not found under Windows Kits 10 — install the Windows SDK (or Visual Studio's 'Desktop development with C++' workload)."
}
Write-Host "    $signtool"

foreach ($file in $files) {
    Write-Host "==> Signing $($file.FullName)"
    & $signtool sign /sha1 $Thumbprint /fd SHA256 /tr $TimestampUrl /td SHA256 $file.FullName
    if ($LASTEXITCODE -ne 0) { throw "signtool sign failed ($LASTEXITCODE) for $($file.FullName)" }

    # /pa checks the Authenticode policy, which includes chaining to a trusted
    # root. On a machine that has not run import-trusted-root.ps1 this fails by
    # design — the signature is still applied and valid, just not trusted here.
    # Warn rather than throw, or signing on a build machine would always "fail".
    & $signtool verify /pa $file.FullName
    if ($LASTEXITCODE -ne 0) {
        Write-Warning "signtool verify /pa failed for $($file.FullName) — expected unless this machine has imported the cert into Trusted Root (import-trusted-root.ps1)."
    }
}

Write-Host ''
Write-Host "Signed $($files.Count) file(s) with $Thumbprint."
