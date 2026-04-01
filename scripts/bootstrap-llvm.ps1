param(
    [string]$Version = "21.1.2",
    [string]$Asset = ""
)

$ErrorActionPreference = "Stop"

if (-not $env:CI) {
    Write-Host "CI not detected; building LLVM from source via deps/llvm-bootstrap."
    & (Join-Path $PSScriptRoot "bootstrap-llvm-source.ps1") -Version $Version -Linkage Static
    exit $LASTEXITCODE
}

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$depsRoot = Join-Path $repoRoot ".deps"
$llvmRoot = Join-Path $depsRoot "llvm"
$versionRoot = Join-Path $llvmRoot $Version
$distRoot = Join-Path $depsRoot "dist"

$releaseTag = "llvm-$Version"
if ([string]::IsNullOrWhiteSpace($Asset)) {
    $Asset = "llvm-$Version-windows-x64-static.zip"
}
$url = "https://github.com/vitreo12/llvm-bootstrap/releases/download/$releaseTag/$Asset"
$archive = Join-Path $distRoot $Asset
$tempExtractRoot = Join-Path $distRoot ("extract-" + $Version)

New-Item -ItemType Directory -Force -Path $depsRoot | Out-Null
New-Item -ItemType Directory -Force -Path $llvmRoot | Out-Null
New-Item -ItemType Directory -Force -Path $distRoot | Out-Null

if (Test-Path (Join-Path $versionRoot "bin/llvm-config.exe")) {
    Write-Host "LLVM $Version already bootstrapped at $versionRoot"
    exit 0
}

Write-Host "CI detected; downloading prebuilt LLVM package from $url"
Invoke-WebRequest -Uri $url -OutFile $archive

if (Test-Path $tempExtractRoot) {
    Remove-Item -Recurse -Force $tempExtractRoot
}
New-Item -ItemType Directory -Force -Path $tempExtractRoot | Out-Null

Write-Host "Extracting archive..."
tar -xf $archive -C $tempExtractRoot

$topLevelEntries = Get-ChildItem -Path $tempExtractRoot -Force
if (-not $topLevelEntries) {
    throw "Extraction failed: archive produced no files"
}

$contentRoot = $tempExtractRoot
if ($topLevelEntries.Count -eq 1 -and $topLevelEntries[0].PSIsContainer) {
    $contentRoot = $topLevelEntries[0].FullName
}

if (Test-Path $versionRoot) {
    Remove-Item -Recurse -Force $versionRoot
}
New-Item -ItemType Directory -Force -Path $versionRoot | Out-Null

Get-ChildItem -Path $contentRoot -Force | ForEach-Object {
    Move-Item -Path $_.FullName -Destination $versionRoot
}

if (-not (Test-Path (Join-Path $versionRoot "bin/llvm-config.exe"))) {
    throw "llvm-config.exe not found after extraction. Verify asset or LLVM package contents."
}

Write-Host "LLVM bootstrapped to $versionRoot"
