<#
Builds the PDF Editor desktop app (exe + MSI) with the internal self-signed
code-signing certificate applied, without hardcoding the (per-machine)
certificate thumbprint into the tracked tauri.conf.json.

Requires:
  - generate-selfsigned-cert.ps1 already run on this machine (cert must be
    present in Cert:\CurrentUser\My, thumbprint recorded in output\thumbprint.txt)
  - tauri-cli installed (cargo install tauri-cli --version "^2")

Usage:
    .\build-signed.ps1
    .\build-signed.ps1 -- --target x86_64-pc-windows-msvc   # extra args passed through to `tauri build`
#>

param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$ExtraArgs
)

$ErrorActionPreference = 'Stop'

$thumbprintPath = Join-Path $PSScriptRoot 'output\thumbprint.txt'
if (-not (Test-Path $thumbprintPath)) {
    throw "No thumbprint found at $thumbprintPath. Run generate-selfsigned-cert.ps1 first."
}
$thumbprint = (Get-Content $thumbprintPath -Raw).Trim()

$certExists = Get-ChildItem 'Cert:\CurrentUser\My' | Where-Object { $_.Thumbprint -eq $thumbprint }
if (-not $certExists) {
    throw "Certificate with thumbprint $thumbprint not found in Cert:\CurrentUser\My. Re-run generate-selfsigned-cert.ps1 on this machine, or import the .pfx here first."
}

$configOverride = @{
    bundle = @{
        windows = @{
            certificateThumbprint = $thumbprint
            digestAlgorithm       = 'sha256'
            timestampUrl          = 'http://timestamp.digicert.com'
        }
    }
} | ConvertTo-Json -Depth 5

# 傳 inline JSON 字串給 --config 時，PowerShell 轉給 native exe (cargo.exe) 的參數
# 組譯規則會把字串裡的雙引號吃掉，導致 tauri 那邊收到壞掉的 JSON（缺引號）。寫成暫存
# 檔用路徑傳，繞開這條組譯路徑。
$configOverridePath = Join-Path $PSScriptRoot 'output\build-signed.config.json'
Set-Content -Path $configOverridePath -Value $configOverride -Encoding utf8

$desktopDir = Resolve-Path (Join-Path $PSScriptRoot '..\..')

Write-Host "Building signed app (thumbprint $thumbprint) in $desktopDir ..."
Push-Location $desktopDir
try {
    cargo tauri build --config $configOverridePath @ExtraArgs
    if ($LASTEXITCODE -ne 0) {
        throw "cargo tauri build failed with exit code $LASTEXITCODE"
    }
}
finally {
    Pop-Location
}

Write-Host ''
Write-Host 'Build complete. The bundled .exe and .msi under target\release\bundle\ should now carry the internal test signature.'
Write-Host 'Reminder: this signature is only trusted on machines where import-trusted-root.ps1 has been run.'
