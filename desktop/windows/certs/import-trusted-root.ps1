<#
Imports the PDF Editor internal self-signed code-signing certificate's public
half into this machine's Trusted Root Certification Authorities store.

Run this ONLY on internal test machines you control, never on a machine you
don't trust or one intended to represent a real end-user environment — adding
a cert to Trusted Root means Windows will trust ANY signature made with the
matching private key, not just PDF Editor's installer.

Requires: an elevated (Run as Administrator) PowerShell terminal, and a copy
of pdf-editor-codesign.cer (produced by generate-selfsigned-cert.ps1 on the
build machine) sitting next to this script or passed via -CerPath.

Usage:
    .\import-trusted-root.ps1
    .\import-trusted-root.ps1 -CerPath C:\path\to\pdf-editor-codesign.cer
#>

param(
    [string]$CerPath = (Join-Path $PSScriptRoot 'output\pdf-editor-codesign.cer')
)

$ErrorActionPreference = 'Stop'

$currentPrincipal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $currentPrincipal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this script as Administrator (right-click PowerShell -> Run as administrator).'
}

if (-not (Test-Path $CerPath)) {
    throw "Certificate file not found: $CerPath. Copy pdf-editor-codesign.cer from the build machine first, or pass -CerPath."
}

Write-Host "Importing $CerPath into Cert:\LocalMachine\Root ..."
Import-Certificate -FilePath $CerPath -CertStoreLocation 'Cert:\LocalMachine\Root' | Out-Null

Write-Host 'Done. This machine will now trust signatures made with the PDF Editor internal test certificate.'
