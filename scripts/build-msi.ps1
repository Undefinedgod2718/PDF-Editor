# Build PDF Editor desktop MSI on Windows (WiX Toolset v3 via Tauri).
# Run from repo root:  .\scripts\build-msi.ps1
#
# Prerequisites:
#   - Node.js, Rust (stable), Visual Studio Build Tools
#   - WiX Toolset v3 (Tauri bundles MSI)
#   - pdfium.dll at server/pdfium.dll
#   - GenSenRoundedTW-R.ttf at server/fonts/
#   - eng.traineddata / chi_tra.traineddata at server/tessdata/ (see .gitignore
#     for the curl download commands)
#   - python-embed/ built via desktop/windows/prepare-python-embed.ps1 — without
#     it the MSI installs with no Python sidecar and every docx/xlsx/markdown
#     export 500s at runtime as "conversion failed" (this shipped once already;
#     `cargo tauri build` below now refuses to proceed without it, but this
#     script fails faster and with a clearer message)
#
# This script runs npm ci + web build itself before the Tauri MSI bundle.

$ErrorActionPreference = "Stop"
$Root = Split-Path $PSScriptRoot -Parent
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

foreach ($Lang in @("eng", "chi_tra")) {
    $Tessdata = Join-Path $Root "server\tessdata\$Lang.traineddata"
    if (-not (Test-Path $Tessdata)) {
        Write-Error "Missing $Tessdata — download it first, e.g.:`n  curl -fSL -o server\tessdata\$Lang.traineddata https://github.com/tesseract-ocr/tessdata/raw/main/$Lang.traineddata"
    }
}

$PythonEmbed = Join-Path $Root "python-embed\python.exe"
if (-not (Test-Path $PythonEmbed)) {
    Write-Error "Missing $PythonEmbed — build it first:`n  .\desktop\windows\prepare-python-embed.ps1"
}

# Bundled into the MSI via desktop/tauri.conf.json resources.
Write-Host "==> cargo build --release (mcp-server -> target-mcp/)"
$env:CARGO_TARGET_DIR = Join-Path $Root "target-mcp"
Push-Location mcp-server
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "mcp-server build failed with exit code $LASTEXITCODE" }
Pop-Location
$McpExe = Join-Path $Root "target-mcp\release\pdf-editor-mcp-server.exe"
if (-not (Test-Path $McpExe)) {
    Write-Error "Missing $McpExe after mcp-server release build."
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
