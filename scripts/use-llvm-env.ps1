$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$prebuiltPrefix = Join-Path $repoRoot ".deps/llvm/21.1.2"
$sourcePrefix = Join-Path $repoRoot ".deps/llvm-src/21.1.2"

$prefix = $null
if (Test-Path (Join-Path $prebuiltPrefix "bin/llvm-config.exe")) {
    $prefix = $prebuiltPrefix
} elseif (Test-Path (Join-Path $sourcePrefix "bin/llvm-config.exe")) {
    $prefix = $sourcePrefix
}

if ($null -eq $prefix) {
    throw "LLVM not found. Run scripts/bootstrap-llvm.ps1 first."
}

$env:LLVM_SYS_211_PREFIX = $prefix
$env:PATH = (Join-Path $prefix "bin") + ";" + $env:PATH
Write-Host "LLVM env configured for this shell: $prefix"
