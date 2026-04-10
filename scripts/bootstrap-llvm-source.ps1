param(
    [string]$Version = "21.1.2",
    [ValidateSet("Static", "Shared")]
    [string]$Linkage = "Static"
)

$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$bootstrapRoot = Join-Path $repoRoot "deps/llvm-bootstrap"
$depsRoot = Join-Path $repoRoot ".deps"
$linkageLower = $Linkage.ToLowerInvariant()
$sourceDir = Join-Path $depsRoot ("src/llvm-project-" + $Version)
$buildDir = Join-Path $depsRoot ("build-llvm-" + $Version + "-" + $linkageLower)
$installDir = Join-Path $depsRoot ("llvm-src/" + $Version + "-" + $linkageLower)

if (-not (Test-Path (Join-Path $bootstrapRoot "build_local.ps1"))) {
    throw "deps/llvm-bootstrap is missing. Run 'git submodule update --init --recursive' first."
}

$buildArgs = @{
    LlvmRef = "llvmorg-$Version"
    SourceDir = $sourceDir
    BuildDir = $buildDir
    InstallDir = $installDir
    Linkage = $Linkage
}

if ([System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([System.Runtime.InteropServices.OSPlatform]::Windows)) {
    $buildArgs["MsvcRuntime"] = "MT"
}

& (Join-Path $bootstrapRoot "build_local.ps1") @buildArgs
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

Write-Host "LLVM source build installed at: $installDir"
