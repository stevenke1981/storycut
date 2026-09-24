param(
    [string]$Workspace = (Join-Path (Get-Location) 'storycut-workspace')
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
New-Item -ItemType Directory -Force -Path $Workspace | Out-Null
Push-Location $repoRoot
try {
    cargo build -p storycut-cli
    if ($LASTEXITCODE -ne 0) { throw 'Rust build failed.' }
    Push-Location (Join-Path $repoRoot 'desktop')
    try {
        npm ci
        if ($LASTEXITCODE -ne 0) { throw 'Frontend dependency install failed.' }
        npm run tauri dev
        if ($LASTEXITCODE -ne 0) { throw 'Tauri development launch failed.' }
    } finally { Pop-Location }
} finally { Pop-Location }
