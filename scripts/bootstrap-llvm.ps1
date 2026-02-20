param(
    [string]$Version = "21.1.2"
)

$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$depsRoot = Join-Path $repoRoot ".deps"
$llvmRoot = Join-Path $depsRoot "llvm"
$versionRoot = Join-Path $llvmRoot $Version
$distRoot = Join-Path $depsRoot "dist"

$asset = "clang+llvm-$Version-x86_64-pc-windows-msvc.tar.xz"
$url = "https://github.com/llvm/llvm-project/releases/download/llvmorg-$Version/$asset"
$archive = Join-Path $distRoot $asset
$tempExtractRoot = Join-Path $distRoot ("extract-" + $Version)

New-Item -ItemType Directory -Force -Path $depsRoot | Out-Null
New-Item -ItemType Directory -Force -Path $llvmRoot | Out-Null
New-Item -ItemType Directory -Force -Path $distRoot | Out-Null

if (Test-Path (Join-Path $versionRoot "bin/llvm-config.exe")) {
    Write-Host "LLVM $Version already bootstrapped at $versionRoot"
    exit 0
}

Write-Host "Downloading $url"
Invoke-WebRequest -Uri $url -OutFile $archive

if (Test-Path $tempExtractRoot) {
    Remove-Item -Recurse -Force $tempExtractRoot
}
New-Item -ItemType Directory -Force -Path $tempExtractRoot | Out-Null

Write-Host "Extracting archive..."
tar -xf $archive -C $tempExtractRoot

$extractedDir = Get-ChildItem -Path $tempExtractRoot -Directory | Select-Object -First 1
if (-not $extractedDir) {
    throw "Extraction failed: no directory found in archive"
}

if (Test-Path $versionRoot) {
    Remove-Item -Recurse -Force $versionRoot
}
New-Item -ItemType Directory -Force -Path $versionRoot | Out-Null

Get-ChildItem -Path $extractedDir.FullName -Force | ForEach-Object {
    Move-Item -Path $_.FullName -Destination $versionRoot
}

if (-not (Test-Path (Join-Path $versionRoot "bin/llvm-config.exe"))) {
    throw "llvm-config.exe not found after extraction. Verify asset or LLVM package contents."
}

Write-Host "LLVM bootstrapped to $versionRoot"
Write-Host "Set env in your shell if needed:"
Write-Host "`$env:LLVM_SYS_211_PREFIX = '$versionRoot'"
