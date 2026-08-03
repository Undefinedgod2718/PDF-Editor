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

# .NET directly, not Get-FileHash: on GitHub's windows-latest runner, the outer
# `shell: pwsh` step spawns `powershell.exe` (Windows PowerShell 5.1, what
# tauri's beforeBuildCommand actually invokes) as a nested child process that
# inherits pwsh's $env:PSModulePath. That path doesn't include Windows
# PowerShell's own module directory, so 5.1's command-not-found autoload never
# finds Microsoft.PowerShell.Utility and Get-FileHash fails with "not
# recognized" — nothing to do with this script's logic, and `Import-Module` by
# name hits the same broken path resolution, so it isn't a fix either. Loading
# System.Security.Cryptography via .NET sidesteps the module system entirely.
# Reproduces only through that nested-shell path; a plain local
# `powershell -File ...` run never hits it, which is what makes this worth a
# comment instead of a silent one-liner swap.
function Get-Sha256Hex([string]$Path) {
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $stream = [System.IO.File]::OpenRead($Path)
        try {
            -join ($sha.ComputeHash($stream) | ForEach-Object { $_.ToString('x2') })
        } finally { $stream.Dispose() }
    } finally { $sha.Dispose() }
}

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
$srcHash   = Get-Sha256Hex $sourcePy
$embedHash = Get-Sha256Hex $embedPy
if ($srcHash -ne $embedHash) {
    Fail "bundled convert.py does not match python\convert.py. The MSI would ship the OLD sidecar script - the build succeeds and the bug only shows up at runtime."
}

# ---- 3. dependencies actually import ----
# No quotes inside -c: PowerShell strips embedded double quotes when handing an
# argument to a native exe (this bit prepare-python-embed.ps1 once already).
#
# $ErrorActionPreference is downgraded for just this call because Windows
# PowerShell 5.1 — specifically 5.1, not pwsh 7 — promotes ANY stderr line from
# a native process into a terminating NativeCommandError when the preference is
# 'Stop', regardless of the process's actual exit code. `2>$null` does not save
# it: the promotion happens before the redirect target matters. This is exactly
# what tripped GitHub's windows-msi runner: onnxruntime prints a benign
# UserWarning ("Unsupported Windows version (2025server)...") to stderr on
# import, on that OS specifically. The interpreter's own exit code is what
# actually answers "did the import work", so that's what's checked below —
# stderr noise from a healthy process must not fail the gate.
$prevEap = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
& $pythonExe -c 'import fitz, pdf2docx, pdfplumber, openpyxl, markitdown' 2>$null
$importExit = $LASTEXITCODE
$ErrorActionPreference = $prevEap
if ($importExit -ne 0) {
    Fail "the bundled interpreter cannot import its dependencies (exit $importExit) - the tree is present but broken."
}

Write-Host "python sidecar bundle OK ($embedDir)" -ForegroundColor Green
exit 0
