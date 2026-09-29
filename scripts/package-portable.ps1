param(
    [string]$OutputDirectory = (Join-Path (Split-Path -Parent $PSScriptRoot) 'dist')
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$config = Get-Content -LiteralPath (Join-Path $repoRoot 'desktop/src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json
$version = [string]$config.version
$archiveName = "StoryCut-$version-portable-windows-x64.zip"
$desktopExe = Join-Path $repoRoot 'desktop/src-tauri/target/release/storycut-desktop.exe'
$cliExe = Join-Path $repoRoot 'target/release/storycut.exe'

foreach ($required in @($desktopExe, $cliExe)) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
        throw "Release executable is missing: $required. Build CLI and desktop release first."
    }
}

$files = @(
    @{ Source = $desktopExe; Entry = 'StoryCut.exe' },
    @{ Source = $cliExe; Entry = 'storycut-cli.exe' },
    @{ Source = (Join-Path $repoRoot 'scripts/Start-StoryCut.ps1'); Entry = 'Start-StoryCut.ps1' },
    @{ Source = (Join-Path $repoRoot 'docs/PORTABLE_WINDOWS.md'); Entry = 'README-portable.md' },
    @{ Source = (Join-Path $repoRoot 'docs/CLI.md'); Entry = 'docs/CLI.md' },
    @{ Source = (Join-Path $repoRoot 'docs/MCP.md'); Entry = 'docs/MCP.md' },
    @{ Source = (Join-Path $repoRoot 'docs/STORY_WORKFLOW.md'); Entry = 'docs/STORY_WORKFLOW.md' },
    @{ Source = (Join-Path $repoRoot 'contracts/mcp-tools.json'); Entry = 'contracts/mcp-tools.json' },
    @{ Source = (Join-Path $repoRoot 'docs/IMPLEMENTATION_STATUS.md'); Entry = 'docs/IMPLEMENTATION_STATUS.md' },
    @{ Source = (Join-Path $repoRoot 'docs/LICENSE_INVENTORY.md'); Entry = 'docs/LICENSE_INVENTORY.md' },
    @{ Source = (Join-Path $repoRoot 'LICENSE'); Entry = 'LICENSE' }
)
foreach ($item in $files) {
    if (-not (Test-Path -LiteralPath $item.Source -PathType Leaf)) {
        throw "Package input is missing: $($item.Source)"
    }
}

New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$outputPath = Join-Path $OutputDirectory $archiveName
if (Test-Path -LiteralPath $outputPath) {
    throw "Archive already exists and will not be overwritten: $outputPath"
}
$temporaryPath = Join-Path $OutputDirectory (".$archiveName.stage-" + [guid]::NewGuid().ToString('N'))

Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
$stream = $null
$archive = $null
$hash = $null
try {
    $stream = [System.IO.File]::Open($temporaryPath, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
    $archive = [System.IO.Compression.ZipArchive]::new($stream, [System.IO.Compression.ZipArchiveMode]::Create, $false)
    foreach ($item in $files) {
        [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, $item.Source, $item.Entry, [System.IO.Compression.CompressionLevel]::Optimal) | Out-Null
    }
    $archive.Dispose()
    $archive = $null
    $stream = $null
    $hash = (Get-FileHash -LiteralPath $temporaryPath -Algorithm SHA256).Hash.ToLowerInvariant()
    New-Item -ItemType HardLink -Path $outputPath -Target $temporaryPath | Out-Null
    try {
        Remove-Item -LiteralPath $temporaryPath -ErrorAction Stop
    } catch {
        Write-Warning "Archive was published, but its temporary hard link could not be removed: $temporaryPath"
    }
} catch {
    if ($archive) { $archive.Dispose() }
    elseif ($stream) { $stream.Dispose() }
    Remove-Item -LiteralPath $temporaryPath -ErrorAction SilentlyContinue
    throw
}

Write-Output "Archive: $outputPath"
Write-Output "SHA256: $hash"
