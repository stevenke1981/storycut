$ErrorActionPreference = 'Stop'
$appPath = Join-Path $PSScriptRoot 'StoryCut.exe'
if (-not (Test-Path -LiteralPath $appPath -PathType Leaf)) {
    throw "StoryCut.exe is missing beside this script: $appPath"
}

if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue) -or -not (Get-Command ffprobe -ErrorAction SilentlyContinue)) {
    Write-Warning 'FFmpeg and ffprobe are not on PATH. Editing can open, but media probing, preview and export need both. See README-portable.md.'
}

Start-Process -FilePath $appPath -WorkingDirectory $PSScriptRoot
