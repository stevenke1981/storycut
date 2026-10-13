$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot
try {
    # Match CLI/CI builds so Cargo does not relink a running Windows daemon
    # solely because dependency feature unification changed between commands.
    cargo build --release -p storycut-cli -p storycut-daemon --locked -j 1
    if ($LASTEXITCODE -ne 0) { throw 'Workspace daemon build failed.' }
    Push-Location (Join-Path $repoRoot 'desktop')
    try {
        npm run build
        if ($LASTEXITCODE -ne 0) { throw 'Frontend build failed.' }
    } finally { Pop-Location }
} finally { Pop-Location }
