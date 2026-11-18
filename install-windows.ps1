$ErrorActionPreference = 'Stop'
$utf8 = [System.Text.UTF8Encoding]::new($false)
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$binaryDirectory = Join-Path $PSScriptRoot 'bin'
[System.IO.Directory]::CreateDirectory($binaryDirectory) > $null
$binary = Join-Path $binaryDirectory 'tmux-agent-sidebar.exe'
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'target\release\tmux-agent-sidebar.exe') -Destination $binary

foreach ($agent in @('claude', 'codex')) {
    $settingsPath = if ($agent -eq 'claude') {
        Join-Path $env:USERPROFILE '.claude\settings.json'
    } else {
        Join-Path $env:USERPROFILE '.codex\hooks.json'
    }
    $settings = Get-Content -LiteralPath $settingsPath -Encoding UTF8 -Raw | ConvertFrom-Json
    $snippet = & $binary setup $agent | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw "Cannot generate $agent hooks." }
    foreach ($event in $snippet.hooks.PSObject.Properties) {
        $existing = $settings.hooks.PSObject.Properties[$event.Name]
        if ($existing) {
            $kept = @($existing.Value | Where-Object {
                -not (@($_.hooks | Where-Object { $_.command -match 'tmux-agent-sidebar[\\/]bin[\\/]tmux-agent-sidebar.exe' }).Count)
            })
            $settings.hooks.($event.Name) = @($kept) + @($event.Value)
        } else {
            $settings.hooks | Add-Member -NotePropertyName $event.Name -NotePropertyValue @($event.Value)
        }
    }
    Copy-Item -LiteralPath $settingsPath -Destination "$settingsPath.sidebar-backup-$stamp"
    [System.IO.File]::WriteAllText($settingsPath, ($settings | ConvertTo-Json -Depth 100) + "`n", $utf8)
    Write-Output "Updated $settingsPath; backup: $settingsPath.sidebar-backup-$stamp"
}

$configuration = Join-Path $PSScriptRoot 'windows-sidebar.conf'
$launcher = (Join-Path $PSScriptRoot 'windows-sidebar.ps1').Replace('\', '/')
$lines = @(
    'set -g @sidebar_width 42'
    'set -g @sidebar_position left'
    'set -g @sidebar_bottom_height 0'
    'set -g @sidebar_filter all'
    'set -g @sidebar_repo_filter all'
    "bind e run-shell 'powershell.exe -NoProfile -ExecutionPolicy Bypass -File $launcher toggle -Window #{session_name}:#{window_id}'"
    "bind E run-shell 'powershell.exe -NoProfile -ExecutionPolicy Bypass -File $launcher toggle-all'"
)
[System.IO.File]::WriteAllLines($configuration, $lines, $utf8)
$psmuxConfig = Join-Path $env:USERPROFILE '.psmux.conf'
$sourceLine = 'source-file ' + $configuration.Replace('\', '/')
$current = Get-Content -LiteralPath $psmuxConfig -Encoding UTF8 -Raw
if (-not $current.Contains($sourceLine)) {
    Copy-Item -LiteralPath $psmuxConfig -Destination "$psmuxConfig.sidebar-backup-$stamp"
    [System.IO.File]::WriteAllText($psmuxConfig, $current.TrimEnd() + "`n`n" + $sourceLine + "`n", $utf8)
}
$sessions = & psmux list-sessions -F '#{session_name}'
if ($LASTEXITCODE -ne 0) { throw 'Cannot list psmux sessions.' }
foreach ($session in $sessions) {
    $env:PSMUX_TARGET_SESSION = $session
    Remove-Item Env:PSMUX_TARGET_SESSION_FULL -ErrorAction SilentlyContinue
    Remove-Item Env:PSMUX_TARGET_FULL -ErrorAction SilentlyContinue
    & psmux source-file $configuration
    if ($LASTEXITCODE -ne 0) { throw "Cannot configure psmux session $session." }
}
Write-Output 'Installed: prefix + e toggles this window; prefix + E toggles all windows.'
