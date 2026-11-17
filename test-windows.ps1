$ErrorActionPreference = 'Stop'
$env:TMUX_AGENT_SIDEBAR_TEST = '1'
if ((Test-Path '.venv') -and (-not $env:VIRTUAL_ENV)) { .venv\Scripts\Activate.ps1 }
cargo test --offline @args
exit $LASTEXITCODE
