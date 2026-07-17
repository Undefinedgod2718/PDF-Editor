# Build PDF Editor desktop MSI on Windows (WiX Toolset v3 via Tauri).
# Run from repo root:  .\deploy\windows\build-msi.ps1
#
# Prerequisites:
#   - Node.js, Rust (stable), Visual Studio Build Tools
#   - WiX Toolset v3 (Tauri bundles MSI)
#   - web/dist built; pdfium.dll at server/pdfium.dll
#   - GenSenRoundedTW-R.ttf at server/fonts/ (see deploy/windows/deploy.sh)

$ErrorActionPreference = "Stop"
$Root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
Set-Location $Root

Write-Host "==> npm ci + build (web/)"
Push-Location web
npm ci
if ($LASTEXITCODE -ne 0) { throw "npm ci failed with exit code $LASTEXITCODE" }
npm run build
if ($LASTEXITCODE -ne 0) { throw "npm run build failed with exit code $LASTEXITCODE" }
Pop-Location

$Pdfium = Join-Path $Root "server\pdfium.dll"
if (-not (Test-Path $Pdfium)) {
    Write-Error "Missing $Pdfium — copy pdfium.dll into server/ before building MSI."
}

$Font = Join-Path $Root "server\fonts\GenSenRoundedTW-R.ttf"
if (-not (Test-Path $Font)) {
    Write-Error "Missing $Font — copy the CJK font into server/fonts/ before building MSI."
}

Write-Host "==> cargo tauri build --bundles msi (desktop/)"
$env:CARGO_TARGET_DIR = Join-Path $Root "desktop\target"
Push-Location desktop
cargo tauri --version *> $null
if ($LASTEXITCODE -ne 0) {
    Write-Host "Installing tauri-cli..."
    cargo install tauri-cli --version "^2"
    if ($LASTEXITCODE -ne 0) { throw "tauri-cli install failed with exit code $LASTEXITCODE" }
}
cargo tauri build --bundles msi
if ($LASTEXITCODE -ne 0) { throw "MSI build failed with exit code $LASTEXITCODE" }
Pop-Location

$MsiDir = Join-Path $Root "desktop\target\release\bundle\msi"
Write-Host ""
Write-Host "Done. MSI output:"
Get-ChildItem -Path $MsiDir -Filter "*.msi" -ErrorAction SilentlyContinue | ForEach-Object { $_.FullName }
if (-not (Test-Path $MsiDir)) {
    Write-Warning "Expected bundle dir not found: $MsiDir"
}
