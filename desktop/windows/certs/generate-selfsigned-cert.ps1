<#
Generates a self-signed code-signing certificate for PDF Editor internal builds.

INTERNAL TEST USE ONLY. This certificate is not issued by a trusted CA — it
will only be trusted by machines that explicitly import its public half into
their Trusted Root store (see import-trusted-root.ps1). Windows SmartScreen
and signature checks on any other machine will still flag the app as
untrusted. Do not use this as a substitute for a real EV/OV certificate on
anything distributed outside your own test machines.

Run this yourself in a normal (non-admin) PowerShell terminal — it will
prompt you interactively for a PFX export password.

Usage:
    .\generate-selfsigned-cert.ps1
#>

$ErrorActionPreference = 'Stop'

$outDir = Join-Path $PSScriptRoot 'output'
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$subject = 'CN=PDF Editor Internal Test, O=Lichang, C=TW'

Write-Host "Creating self-signed code-signing certificate ($subject) ..."

$cert = New-SelfSignedCertificate `
    -Type CodeSigningCert `
    -Subject $subject `
    -FriendlyName 'PDF Editor Internal Code Signing (Self-Signed)' `
    -CertStoreLocation 'Cert:\CurrentUser\My' `
    -KeyUsage DigitalSignature `
    -HashAlgorithm sha256 `
    -NotAfter (Get-Date).AddYears(5)

$thumbprint = $cert.Thumbprint
Write-Host "Certificate created. Thumbprint: $thumbprint"

$pfxPassword = Read-Host -AsSecureString -Prompt 'Set a password to protect the exported .pfx'

$pfxPath = Join-Path $outDir 'pdf-editor-codesign.pfx'
$cerPath = Join-Path $outDir 'pdf-editor-codesign.cer'
$thumbprintPath = Join-Path $outDir 'thumbprint.txt'

Export-PfxCertificate -Cert $cert -FilePath $pfxPath -Password $pfxPassword | Out-Null
Export-Certificate -Cert $cert -FilePath $cerPath | Out-Null
Set-Content -Path $thumbprintPath -Value $thumbprint -Encoding ascii -NoNewline

Write-Host ''
Write-Host "Done. Written to $outDir :"
Write-Host "  pdf-editor-codesign.pfx  - private key + cert, password-protected. NEVER commit this."
Write-Host "  pdf-editor-codesign.cer  - public cert only. Distribute this to test machines."
Write-Host "  thumbprint.txt           - used by build-signed.ps1 to find this cert in the store."
Write-Host ''
Write-Host 'Next steps:'
Write-Host '  - On each test machine, copy pdf-editor-codesign.cer over and run import-trusted-root.ps1 (as admin).'
Write-Host '  - On this build machine, run build-signed.ps1 to build a signed MSI/exe.'
