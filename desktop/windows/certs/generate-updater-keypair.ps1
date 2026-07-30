<#
Generates a Tauri updater signing keypair (minisign-style) for future use.

This does NOT wire up tauri-plugin-updater or touch tauri.conf.json — there's
no update server/endpoint yet. This script only produces and safely stores
the keypair so it exists ahead of time. When an update endpoint is stood up
later, add tauri-plugin-updater as a dependency and paste the printed public
key into tauri.conf.json's plugins.updater.pubkey.

Requires: tauri-cli installed (cargo install tauri-cli --version "^2")

You will be prompted interactively for a password to encrypt the private key
file. Do not skip it (blank password stores the key unencrypted on disk).

Usage:
    .\generate-updater-keypair.ps1
#>

$ErrorActionPreference = 'Stop'

$outDir = Join-Path $PSScriptRoot 'output'
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$keyPath = Join-Path $outDir 'pdf-editor-updater.key'

if (Test-Path $keyPath) {
    throw "$keyPath already exists. Delete it first if you really want to regenerate (this invalidates any future update packages signed with the old key)."
}

Write-Host 'Generating Tauri updater signing keypair ...'
cargo tauri signer generate -w $keyPath
if ($LASTEXITCODE -ne 0) {
    throw "cargo tauri signer generate failed with exit code $LASTEXITCODE"
}

Write-Host ''
Write-Host "Done. Written to $outDir :"
Write-Host "  pdf-editor-updater.key       - private key, password-encrypted. NEVER commit this."
Write-Host "  pdf-editor-updater.key.pub   - public key. Safe to commit / paste into tauri.conf.json later."
Write-Host ''
Write-Host 'Public key:'
Get-Content "$keyPath.pub"
