# Run at install time (deferred custom action, impersonated as the
# installing user — HKCU/%USERPROFILE% writes need to land in their
# profile, not SYSTEM's). Best-effort only: a missing `claude` CLI or an
# unwritable Cursor config must never fail the MSI install, so every
# step is wrapped and swallows its own errors.
#
# Logs every step to %TEMP%\pdf-editor-mcp-register.log (impersonated, so
# this resolves to the installing user's own temp dir) — deferred custom
# actions have no visible stdout, so this is the only way to diagnose a
# silent failure after the fact.
param(
    [Parameter(Mandatory = $true)]
    [string]$InstallDir
)

$logPath = Join-Path $env:TEMP 'pdf-editor-mcp-register.log'
function Log($msg) {
    "$(Get-Date -Format o)  $msg" | Out-File -FilePath $logPath -Append -Encoding utf8
}

Log "=== register-mcp.ps1 start ==="
Log "InstallDir (raw): $InstallDir"
# Belt and braces against MSI quoting: a trailing backslash immediately before
# the closing quote in the custom action's ExeCommand escapes that quote, and
# the argument arrives with a literal `"` on the end (this really happened —
# see the comment on RegisterMcp in main.wxs). main.wxs now passes
# "[INSTALLDIR]." so it cannot recur, but strip stray quotes anyway rather than
# fail a Test-Path on a path that is only cosmetically wrong.
$InstallDir = $InstallDir.Trim('"').Trim()
Log "InstallDir: $InstallDir"
Log "whoami: $(whoami)"
Log "PATH: $env:Path"

$exePath = Join-Path $InstallDir 'pdf-editor-mcp-server.exe'
if (-not (Test-Path $exePath)) {
    Log "exePath not found: $exePath -- aborting"
    exit 0
}
Log "exePath OK: $exePath"

# ---- Claude Code CLI ----
try {
    $claude = Get-Command claude -ErrorAction SilentlyContinue
    if ($claude) {
        Log "claude CLI found: $($claude.Source)"
        # -s user: registers globally for this Windows user account, not tied
        # to whatever directory happens to be cwd at install time (the
        # default "local" scope is project-scoped — meaningless for a
        # general desktop app install, not a repo checkout).
        # Remove any stale registration first: `claude mcp add` fails when the
        # name already exists, so on a repair/upgrade (esp. one that changed the
        # install dir) the stored exe path would otherwise never update. Mirrors
        # the Cursor branch's remove-then-add. Missing entry → harmless, logged.
        $rmOutput = & $claude.Source mcp remove pdf-editor -s user 2>&1
        Log "claude mcp remove exit=$LASTEXITCODE output: $rmOutput"
        $output = & $claude.Source mcp add pdf-editor -s user -- $exePath 2>&1
        Log "claude mcp add exit=$LASTEXITCODE output: $output"
    } else {
        Log "claude CLI not found on PATH"
    }
} catch {
    Log "claude registration EXCEPTION: $($_.Exception.Message)"
}

# ---- Cursor: merge into ~/.cursor/mcp.json, don't clobber other servers ----
try {
    $cursorDir = Join-Path $env:USERPROFILE '.cursor'
    $cursorConfigPath = Join-Path $cursorDir 'mcp.json'
    Log "cursorConfigPath: $cursorConfigPath (exists: $(Test-Path $cursorConfigPath))"

    if (Test-Path $cursorConfigPath) {
        $config = Get-Content $cursorConfigPath -Raw | ConvertFrom-Json
    } else {
        $config = [pscustomobject]@{}
    }
    if (-not ($config.PSObject.Properties.Name -contains 'mcpServers')) {
        $config | Add-Member -NotePropertyName 'mcpServers' -NotePropertyValue ([pscustomobject]@{})
    }
    if ($config.mcpServers.PSObject.Properties.Name -contains 'pdf-editor') {
        $config.mcpServers.PSObject.Properties.Remove('pdf-editor')
    }
    $entry = [pscustomobject]@{ command = $exePath; args = @() }
    $config.mcpServers | Add-Member -NotePropertyName 'pdf-editor' -NotePropertyValue $entry

    if (-not (Test-Path $cursorDir)) {
        New-Item -ItemType Directory -Path $cursorDir -Force | Out-Null
    }
    $config | ConvertTo-Json -Depth 10 | Set-Content -Path $cursorConfigPath -Encoding utf8
    Log "cursor mcp.json updated OK"
} catch {
    Log "cursor registration EXCEPTION: $($_.Exception.Message)"
}

Log "=== register-mcp.ps1 end ==="
exit 0
