$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot
try {
    cargo build --release -p storycut-daemon --locked -j 1
    if ($LASTEXITCODE -ne 0) { throw 'Workspace daemon build failed.' }
    Push-Location (Join-Path $repoRoot 'desktop')
    try {
        npm run build
        if ($LASTEXITCODE -ne 0) { throw 'Frontend build failed.' }
    } finally { Pop-Location }
} finally { Pop-Location }
