param(
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$packageJsonPath = Join-Path $repoRoot 'package.json'
$packageJson = Get-Content $packageJsonPath -Raw | ConvertFrom-Json
$version = $packageJson.version

if ([string]::IsNullOrWhiteSpace($version)) {
    throw 'package.json is missing a version.'
}

$bundleName = "lama-blanket-v$version-windows-x64"
$distRoot = Join-Path $repoRoot 'dist'
$bundleRoot = Join-Path $distRoot $bundleName
$zipPath = Join-Path $distRoot "$bundleName.zip"
$releaseRoot = Join-Path $repoRoot 'target\release'
$appExe = Join-Path $releaseRoot 'lama-blanket.exe'

if (-not $SkipBuild) {
    Push-Location $repoRoot
    try {
        cargo build --release
        if ($LASTEXITCODE -ne 0) {
            throw 'cargo build --release failed.'
        }
    }
    finally {
        Pop-Location
    }
}

if (-not (Test-Path $appExe)) {
    throw "Release executable not found at $appExe"
}

if (Test-Path $bundleRoot) {
    Remove-Item $bundleRoot -Recurse -Force
}
if (Test-Path $zipPath) {
    Remove-Item $zipPath -Force
}

New-Item -ItemType Directory -Path $bundleRoot | Out-Null

$filesToCopy = @(
    'README.md',
    'LICENSE',
    'CHANGELOG.md',
    'icon.webp'
)

Copy-Item $appExe $bundleRoot

foreach ($relativePath in $filesToCopy) {
    $sourcePath = Join-Path $repoRoot $relativePath
    if (Test-Path $sourcePath) {
        Copy-Item $sourcePath $bundleRoot
    }
}

Compress-Archive -Path $bundleRoot -DestinationPath $zipPath -Force

Write-Host "Release bundle created at $bundleRoot"
Write-Host "Release zip created at $zipPath"
