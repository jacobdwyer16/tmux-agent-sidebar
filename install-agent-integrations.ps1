$ErrorActionPreference = 'Stop'
$utf8 = [Text.UTF8Encoding]::new($false)
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$extension = Join-Path $env:USERPROFILE '.pi\agent\extensions\tmux-agent-sidebar.ts'
if (Test-Path -LiteralPath $extension) {
    Copy-Item -LiteralPath $extension -Destination "$extension.sidebar-backup-$stamp"
}
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'integrations\pi.ts') -Destination $extension

$settingsPath = Join-Path $env:USERPROFILE '.gemini\antigravity-cli\settings.json'
$settings = Get-Content -LiteralPath $settingsPath -Encoding UTF8 -Raw | ConvertFrom-Json
$binary = (Join-Path $PSScriptRoot 'bin\tmux-agent-sidebar.exe').Replace('\', '/')
$command = "$binary report antigravity"
if ($settings.PSObject.Properties['statusLine'] -and $settings.statusLine.command -and $settings.statusLine.command -ne $command) {
    throw 'An existing Antigravity status-line command needs composition before installing the sidebar integration.'
}
Copy-Item -LiteralPath $settingsPath -Destination "$settingsPath.sidebar-backup-$stamp"
$statusLine = [pscustomobject]@{type='command';command=$command;stack_with_default=$true;enabled=$true}
if ($settings.PSObject.Properties['statusLine']) {
    $settings.statusLine = $statusLine
} else {
    $settings | Add-Member -NotePropertyName statusLine -NotePropertyValue $statusLine
}
[IO.File]::WriteAllText($settingsPath, ($settings | ConvertTo-Json -Depth 100) + "`n", $utf8)
Write-Output "Installed Pi extension: $extension"
Write-Output "Configured Antigravity status reporter; backup: $settingsPath.sidebar-backup-$stamp"
