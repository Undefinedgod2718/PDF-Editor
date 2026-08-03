# Build gate for the bundled Python sidecar. Wired to tauri.conf.json's
# `beforeBuildCommand`, so a non-zero exit here fails `tauri build`.
#
# Why a checker and not just "run prepare-python-embed.ps1": that script builds
# a 467 MB tree from scratch and needs -Force to touch an existing one. Running
# it every build would either fail on the existing directory or reinstall ~370 MB
# of wheels each time. This costs about a second instead.
#
# What it protects against, in the order the mistakes actually happen:
#
#   1. python-embed/ was never built (fresh clone — it is gitignored). Without
#      this gate `tauri build` fails deep inside the bundler with a resource
#      path error, or worse, an earlier version of this project shipped an MSI
#      with no Python at all and every docx/xlsx/markdown export returned a
#      generic 500 "conversion failed".
#
#   2. convert.py drifted. prepare-python-embed.ps1 COPIES python/convert.py
#      into the tree, so editing the source and rebuilding without re-running
#      prep silently ships the stale copy. Nothing about the resulting MSI looks
#      wrong; it just runs old code.
#
#   3. The tree exists but its dependencies are broken (interrupted prep, a
#      half-deleted site-packages). Cheaper to catch here than after a 7-minute
#      release build and an MSI install.
#
# Windows-only by design — this project bundles an MSI and nothing else.
#
# NOTE on the path in tauri.conf.json: tauri runs beforeBuildCommand from the
# REPO ROOT, not from desktop/ — verified by probing (it normalises the cwd
# itself, so it stays the repo root even when you invoke `cargo tauri build`
# from inside desktop/). Hence "desktop/windows/verify-python-embed.ps1" there.
# Get it wrong and the build still fails, but with PowerShell's "-File does not
# exist" instead of anything about the sidecar.

$ErrorActionPreference = 'Stop'

# Everything below resolves off $PSScriptRoot, never the cwd, so running this by
# hand from any directory behaves the same as the build hook.
$repoRoot   = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$embedDir   = Join-Path $repoRoot 'python-embed'
$pythonExe  = Join-Path $embedDir 'python.exe'
$embedPy    = Join-Path $embedDir 'convert.py'
$sourcePy   = Join-Path $repoRoot 'python\convert.py'

$fix = @"
Run the prep script, then build again:

    powershell -ExecutionPolicy Bypass -File desktop\windows\prepare-python-embed.ps1 -Force

(-Force is required when python-embed\ already exists; it rebuilds the tree.)
"@

function Fail($what) {
    Write-Host ''
    Write-Host "python sidecar bundle check FAILED: $what" -ForegroundColor Red
    Write-Host ''
    Write-Host $fix
    Write-Host ''
    exit 1
}

# ---- 1. present at all ----
if (-not (Test-Path $pythonExe)) {
    Fail "no interpreter at $pythonExe. The MSI would install without a Python sidecar, and every docx/xlsx/markdown export would fail at runtime with `"conversion failed`"."
}
if (-not (Test-Path $embedPy)) {
    Fail "no convert.py at $embedPy (the interpreter is there, so the tree is half-built)."
}

# ---- 2. convert.py in sync with source ----
# Hash rather than timestamp: a fresh checkout or a file copy can leave mtimes
# that say "stale" about identical content, and a build that cries wolf gets
# ignored.
if (-not (Test-Path $sourcePy)) {
    Fail "python\convert.py is missing from the repo - cannot verify the bundled copy."
}
$srcHash   = (Get-FileHash $sourcePy -Algorithm SHA256).Hash
$embedHash = (Get-FileHash $embedPy  -Algorithm SHA256).Hash
if ($srcHash -ne $embedHash) {
    Fail "bundled convert.py does not match python\convert.py. The MSI would ship the OLD sidecar script - the build succeeds and the bug only shows up at runtime."
}

# ---- 3. dependencies actually import ----
# No quotes inside -c: PowerShell strips embedded double quotes when handing an
# argument to a native exe (this bit prepare-python-embed.ps1 once already).
& $pythonExe -c 'import fitz, pdf2docx, pdfplumber, openpyxl, markitdown' 2>$null
if ($LASTEXITCODE -ne 0) {
    Fail "the bundled interpreter cannot import its dependencies (exit $LASTEXITCODE) - the tree is present but broken."
}

Write-Host "python sidecar bundle OK ($embedDir)" -ForegroundColor Green
exit 0
