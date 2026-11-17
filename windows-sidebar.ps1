param(
    [ValidateSet('toggle', 'toggle-all')]
    [string]$Action = 'toggle',
    [string]$Window
)

$ErrorActionPreference = 'Stop'
$binary = Join-Path $PSScriptRoot 'bin\tmux-agent-sidebar.exe'
if ($Action -eq 'toggle-all') {
    & $binary toggle-all
} else {
    if (-not $Window) {
        $Window = (& psmux display-message -p '#{session_name}:#{window_id}').Trim()
        if ($LASTEXITCODE -ne 0 -or -not $Window) { throw 'Cannot resolve the current psmux window.' }
    }
    $panePath = (& psmux display-message -t $Window -p '#{pane_current_path}').Trim()
    if ($LASTEXITCODE -ne 0 -or -not $panePath) { throw 'Cannot resolve the current psmux directory.' }
    & $binary toggle $Window $panePath
}
if ($LASTEXITCODE -ne 0) { throw "Sidebar failed with exit code $LASTEXITCODE." }
