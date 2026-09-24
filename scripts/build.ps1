param(
    [ValidateSet('cli', 'desktop', 'portable', 'all')]
    [string]$Target = 'all'
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot
try {
    if ($Target -in @('cli', 'portable', 'all')) {
        cargo build --release -p storycut-cli --locked
        if ($LASTEXITCODE -ne 0) { throw 'CLI build failed.' }
    }
    if ($Target -in @('desktop', 'portable', 'all')) {
        Push-Location (Join-Path $repoRoot 'desktop')
        try {
            npm ci
            if ($LASTEXITCODE -ne 0) { throw 'Frontend dependency install failed.' }
            if ($Target -eq 'portable') {
                npm run tauri build -- --no-bundle
            } else {
                npm run tauri build
            }
            if ($LASTEXITCODE -ne 0) { throw 'Tauri build failed.' }
        } finally { Pop-Location }
    }
    if ($Target -eq 'portable') {
        & (Join-Path $repoRoot 'scripts/package-portable.ps1')
    }
} finally { Pop-Location }
