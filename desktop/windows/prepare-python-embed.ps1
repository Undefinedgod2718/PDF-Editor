# Build-time (NOT install-time) step: materialise a self-contained Python
# sidecar runtime that the MSI can ship verbatim.
#
# Why this exists: the desktop MSI had no Python at all, so every docx / xlsx /
# markdown export failed with the generic "conversion failed" 500 (see
# pdf-core/src/sidecar.rs — `SidecarError::Internal` deliberately hides the real
# reason from the client). The Linux deploy path (deploy/linux/deploy.sh) can
# get away with building a venv on the target box because a server has python3;
# a Windows desktop cannot make that bet, so we ship an interpreter.
#
# Output layout is dictated by sidecar.rs's deployment fallbacks — it looks for
# `python/python.exe` and `python/convert.py` relative to the exe's cwd, so both
# land in the SAME directory here and tauri.conf.json maps the whole thing to
# `python/` inside INSTALLDIR:
#
#     <OutDir>/
#       python.exe            <- embeddable interpreter
#       python312._pth        <- patched to enable site-packages
#       Lib/site-packages/    <- markitdown, pdf2docx, pdfplumber, openpyxl, ...
#       convert.py            <- copied from python/convert.py
#
# Run from anywhere; paths resolve off this script's own location.
#
#     powershell -ExecutionPolicy Bypass -File desktop\windows\prepare-python-embed.ps1
#
# Re-running against an existing OutDir requires -Force (it wipes the tree).

[CmdletBinding()]
param(
    # python.org only publishes embeddable zips for releases that still had
    # binary installers. 3.12 went source-only after 3.12.10, so that is the
    # newest 3.12 embeddable that exists — bumping this blindly to match the
    # dev venv (uv ships 3.12.11 from python-build-standalone) will 404.
    [string]$PythonVersion = '3.12.10',

    # Defaults to <repo>/python-embed. Build artifact, not source — gitignored.
    [string]$OutDir,

    [switch]$Force
)

$ErrorActionPreference = 'Stop'
# Invoke-WebRequest in Windows PowerShell 5.1 renders a progress bar that costs
# more wall-clock than the download itself on a ~10 MB file.
$ProgressPreference = 'SilentlyContinue'

function Step($msg) { Write-Host "==> $msg" -ForegroundColor Cyan }
function Note($msg) { Write-Host "    $msg" -ForegroundColor DarkGray }

# ---- paths ----
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$pythonSrc = Join-Path $repoRoot 'python'
$convertPy = Join-Path $pythonSrc 'convert.py'
if (-not (Test-Path $convertPy)) {
    throw "convert.py not found at $convertPy - is $repoRoot really the repo root?"
}
if (-not $OutDir) { $OutDir = Join-Path $repoRoot 'python-embed' }

# Downloads are cached outside OutDir so -Force rebuilds don't re-fetch ~10 MB
# of interpreter every time.
$cacheDir = Join-Path $env:TEMP 'pdf-editor-pyembed-cache'
if (-not (Test-Path $cacheDir)) { New-Item -ItemType Directory -Path $cacheDir -Force | Out-Null }

Step "repo root:   $repoRoot"
Note "out dir:     $OutDir"
Note "cache dir:   $cacheDir"
Note "python:      $PythonVersion (embeddable amd64)"

# ---- 0. guard an existing tree ----
if (Test-Path $OutDir) {
    if (-not $Force) {
        throw "$OutDir already exists. Re-run with -Force to delete and rebuild it."
    }
    Step "removing existing $OutDir (-Force)"
    Remove-Item -Recurse -Force $OutDir
}

# ---- 1. fetch + extract the embeddable interpreter ----
$zipName = "python-$PythonVersion-embed-amd64.zip"
$zipPath = Join-Path $cacheDir $zipName
if (Test-Path $zipPath) {
    Step "using cached $zipName"
} else {
    $url = "https://www.python.org/ftp/python/$PythonVersion/$zipName"
    Step "downloading $url"
    try {
        Invoke-WebRequest -Uri $url -OutFile $zipPath -UseBasicParsing
    } catch {
        # A wrong -PythonVersion is the overwhelmingly likely cause, and the raw
        # 404 doesn't say that.
        if (Test-Path $zipPath) { Remove-Item -Force $zipPath }
        throw "failed to download $url - does python.org publish an embeddable zip for $PythonVersion? (3.12 tops out at 3.12.10). Underlying error: $($_.Exception.Message)"
    }
}
Note ("zip size:    {0:N1} MB" -f ((Get-Item $zipPath).Length / 1MB))

Step "extracting to $OutDir"
Expand-Archive -Path $zipPath -DestinationPath $OutDir -Force

$pythonExe = Join-Path $OutDir 'python.exe'
if (-not (Test-Path $pythonExe)) {
    throw "extraction produced no python.exe in $OutDir - is $zipName really the embeddable (not the installer) zip?"
}

# ---- 2. enable site-packages ----
# The embeddable build ships an isolated sys.path: a `pythonXY._pth` file next
# to python.exe replaces the normal path machinery, and it has `import site`
# commented out. Without patching it, `python -m pip` cannot even bootstrap and
# nothing installed into Lib\site-packages is importable. Glob for the file
# rather than deriving `python312._pth` from $PythonVersion so a future 3.13
# bump doesn't silently no-op here.
$pthFile = Get-ChildItem -Path $OutDir -Filter 'python*._pth' -File | Select-Object -First 1
if (-not $pthFile) {
    throw "no python*._pth in $OutDir - the embeddable layout changed, path setup needs revisiting"
}
Step "patching $($pthFile.Name) to enable site-packages"
$pth = Get-Content $pthFile.FullName
$pth = $pth | ForEach-Object {
    if ($_ -match '^\s*#\s*import\s+site\s*$') { 'import site' } else { $_ }
}
if ($pth -notcontains 'Lib\site-packages') { $pth += 'Lib\site-packages' }
Set-Content -Path $pthFile.FullName -Value $pth -Encoding ascii
Note ("_pth now: " + (($pth | Where-Object { $_ -and $_ -notmatch '^\s*#' }) -join ' | '))

# ---- 3. bootstrap pip ----
# The embeddable distribution has no pip and no ensurepip module, so get-pip.py
# is the only way in.
$getPip = Join-Path $cacheDir 'get-pip.py'
if (Test-Path $getPip) {
    Step 'using cached get-pip.py'
} else {
    Step 'downloading get-pip.py'
    Invoke-WebRequest -Uri 'https://bootstrap.pypa.io/get-pip.py' -OutFile $getPip -UseBasicParsing
}
Step 'installing pip into the embedded interpreter'
& $pythonExe $getPip --no-warn-script-location --quiet
if ($LASTEXITCODE -ne 0) { throw "get-pip.py failed (exit $LASTEXITCODE)" }

# ---- 4. resolve dependency set from the uv lock ----
# uv.lock is the source of truth for the sidecar's dependencies; exporting it
# keeps the shipped interpreter byte-for-byte version-matched to what the dev
# venv and the Linux deploy resolve to, instead of re-resolving at build time
# and quietly shipping different versions than were tested.
$reqFile = Join-Path $cacheDir 'sidecar-requirements.txt'
$uv = Get-Command uv -ErrorAction SilentlyContinue
if ($uv) {
    Step 'exporting locked dependencies via uv'
    Push-Location $pythonSrc
    try {
        # --no-emit-project: the sidecar itself is a virtual root package with no
        # wheel to install; only its dependencies matter here.
        # Out-Null on stdout only: uv echoes the whole hash-pinned requirements
        # file (600+ lines) even when --output-file is given, which buries the
        # rest of this script's output. Errors still surface via stderr.
        & $uv.Source export --frozen --no-emit-project --output-file $reqFile | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "uv export failed (exit $LASTEXITCODE)" }
    } finally {
        Pop-Location
    }
    Note "requirements: $reqFile"
    Step 'installing locked dependencies (this pulls ~370 MB incl. opencv/onnxruntime)'
    & $pythonExe -m pip install --no-warn-script-location --requirement $reqFile
} else {
    # Falling back loses lock parity, so say so loudly rather than pretend the
    # result is reproducible.
    Write-Warning 'uv not found on PATH - falling back to the loose specs in pyproject.toml. Installed versions may differ from uv.lock.'
    Step 'installing dependencies from pyproject specs'
    & $pythonExe -m pip install --no-warn-script-location `
        'markitdown>=0.1.6' 'openpyxl>=3.1.5' 'pdf2docx>=0.5.13' 'pdfplumber>=0.11.10'
}
if ($LASTEXITCODE -ne 0) { throw "pip install failed (exit $LASTEXITCODE)" }

# ---- 5. drop convert.py alongside the interpreter ----
# Same directory on purpose: sidecar.rs resolves `python/python.exe` and
# `python/convert.py` as siblings under INSTALLDIR.
Step 'copying convert.py'
Copy-Item -Path $convertPy -Destination (Join-Path $OutDir 'convert.py') -Force

# ---- 6. prove it actually runs ----
# An embeddable interpreter can install packages fine and still fail to import
# them (bad _pth, missing VC runtime, a wheel that assumes a real prefix), so
# verify with a real conversion rather than trusting pip's exit code.
Step 'smoke test: imports'
# No quotes inside the -c snippet on purpose: PowerShell strips embedded double
# quotes when handing an argument to a native exe, so a `print("...")` here
# reaches python as `print(...` and dies with a SyntaxError that looks like a
# broken interpreter. The exit code alone is the signal we need.
& $pythonExe -c 'import fitz, pdf2docx, pdfplumber, openpyxl, markitdown'
if ($LASTEXITCODE -ne 0) { throw "import smoke test failed (exit $LASTEXITCODE) - the embedded environment is broken" }
Note 'imports OK'

$fixture = Join-Path $repoRoot 'deploy\acceptance\fixtures\chinese.pdf'
if (Test-Path $fixture) {
    Step 'smoke test: markdown conversion'
    $smokeOut = Join-Path $cacheDir 'smoke.md'
    & $pythonExe (Join-Path $OutDir 'convert.py') --mode markdown --input $fixture --output $smokeOut
    if ($LASTEXITCODE -ne 0) { throw "markdown smoke test failed (exit $LASTEXITCODE)" }
    Remove-Item -Force $smokeOut -ErrorAction SilentlyContinue
} else {
    Write-Warning "fixture $fixture missing - skipped the end-to-end conversion check"
}

# ---- done ----
$totalMb = (Get-ChildItem $OutDir -Recurse -File | Measure-Object Length -Sum).Sum / 1MB
Step ("done - {0} ({1:N0} MB)" -f $OutDir, $totalMb)
Note 'next: tauri.conf.json must map this directory into the bundle as python/'
