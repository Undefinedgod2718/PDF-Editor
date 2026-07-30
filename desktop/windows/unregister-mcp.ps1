# Run at uninstall time (deferred custom action, impersonated as the
# uninstalling user — the entries we remove live in HKCU/%USERPROFILE%,
# not SYSTEM's profile). Counterpart to register-mcp.ps1: removes the
# `pdf-editor` MCP registration so uninstalling doesn't leave clients
# (Claude Code CLI / Cursor) pointing at a now-deleted exe.
#
# Best-effort only: a missing `claude` CLI, an absent entry, or an
# unwritable Cursor config must never fail the MSI uninstall, so every
# step is wrapped and swallows its own errors. Removing an entry that is
# not there is a no-op.
#
# Shares the register step's log file (%TEMP%\pdf-editor-mcp-register.log,
# impersonated -> the uninstalling user's own temp dir) — deferred custom
# actions have no visible stdout, so this is the only way to diagnose a
# silent failure after the fact.
param(
    [Parameter(Mandatory = $false)]
    [string]$InstallDir
)

$logPath = Join-Path $env:TEMP 'pdf-editor-mcp-register.log'
function Log($msg) {
    "$(Get-Date -Format o)  $msg" | Out-File -FilePath $logPath -Append -Encoding utf8
}

Log "=== unregister-mcp.ps1 start ==="
Log "InstallDir: $InstallDir"
Log "whoami: $(whoami)"

# ---- Claude Code CLI ----
try {
    $claude = Get-Command claude -ErrorAction SilentlyContinue
    if ($claude) {
        Log "claude CLI found: $($claude.Source)"
        # -s user: matches the scope register-mcp.ps1 added it under. Removing a
        # missing entry is harmless (non-zero exit is logged, not fatal).
        $rmOutput = & $claude.Source mcp remove pdf-editor -s user 2>&1
        Log "claude mcp remove exit=$LASTEXITCODE output: $rmOutput"
    } else {
        Log "claude CLI not found on PATH"
    }
} catch {
    Log "claude unregister EXCEPTION: $($_.Exception.Message)"
}

# ---- Cursor: drop only our key from ~/.cursor/mcp.json, keep other servers ----
try {
    $cursorConfigPath = Join-Path (Join-Path $env:USERPROFILE '.cursor') 'mcp.json'
    Log "cursorConfigPath: $cursorConfigPath (exists: $(Test-Path $cursorConfigPath))"

    if (Test-Path $cursorConfigPath) {
        $config = Get-Content $cursorConfigPath -Raw | ConvertFrom-Json
        if (($config.PSObject.Properties.Name -contains 'mcpServers') -and
            ($config.mcpServers.PSObject.Properties.Name -contains 'pdf-editor')) {
            $config.mcpServers.PSObject.Properties.Remove('pdf-editor')
            $config | ConvertTo-Json -Depth 10 | Set-Content -Path $cursorConfigPath -Encoding utf8
            Log "cursor mcp.json: pdf-editor entry removed"
        } else {
            Log "cursor mcp.json: no pdf-editor entry, nothing to remove"
        }
    } else {
        Log "cursor mcp.json not present, nothing to remove"
    }
} catch {
    Log "cursor unregister EXCEPTION: $($_.Exception.Message)"
}

Log "=== unregister-mcp.ps1 end ==="
exit 0
