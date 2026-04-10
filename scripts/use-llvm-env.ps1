param(
    [ValidateSet("auto", "prebuilt", "source-static", "source-shared", "source")]
    [string]$Flavor = "source-static",
    [string]$Version = "21.1.2"
)

$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$prebuiltPrefix = Join-Path $repoRoot (".deps/llvm/" + $Version)
$sourceStaticPrefix = Join-Path $repoRoot (".deps/llvm-src/" + $Version + "-static")
$sourceSharedPrefix = Join-Path $repoRoot (".deps/llvm-src/" + $Version + "-shared")
$sourceLegacyPrefix = Join-Path $repoRoot (".deps/llvm-src/" + $Version)

function Test-LlvmPrefix([string]$Prefix) {
    return (Test-Path (Join-Path $Prefix "bin/llvm-config.exe"))
}

function Test-SharedLlvmC([string]$Prefix) {
    $libDir = Join-Path $Prefix "lib"
    $binDir = Join-Path $Prefix "bin"

    $hasImportOrStub = (Test-Path (Join-Path $libDir "LLVM-C.lib")) -or
        (Test-Path (Join-Path $libDir "libLLVM-C.so")) -or
        (Test-Path (Join-Path $libDir "libLLVM-C.dylib"))

    $hasRuntime = (Test-Path (Join-Path $binDir "LLVM-C.dll")) -or
        (Test-Path (Join-Path $binDir "libLLVM-C.so")) -or
        (Test-Path (Join-Path $binDir "libLLVM-C.dylib")) -or
        (Test-Path (Join-Path $libDir "libLLVM-C.so")) -or
        (Test-Path (Join-Path $libDir "libLLVM-C.dylib"))

    return $hasImportOrStub -and $hasRuntime
}

$prefix = $null
if ($Flavor -eq "prebuilt") {
    if (Test-LlvmPrefix $prebuiltPrefix) {
        $prefix = $prebuiltPrefix
    }
} elseif ($Flavor -eq "source-static") {
    if (Test-LlvmPrefix $sourceStaticPrefix) {
        $prefix = $sourceStaticPrefix
    }
} elseif ($Flavor -eq "source-shared") {
    if (Test-LlvmPrefix $sourceSharedPrefix) {
        $prefix = $sourceSharedPrefix
    }
} elseif ($Flavor -eq "source") {
    if (Test-LlvmPrefix $sourceStaticPrefix) {
        $prefix = $sourceStaticPrefix
    } elseif (Test-LlvmPrefix $sourceSharedPrefix) {
        $prefix = $sourceSharedPrefix
    } elseif (Test-LlvmPrefix $sourceLegacyPrefix) {
        $prefix = $sourceLegacyPrefix
    }
} else {
    if (Test-LlvmPrefix $sourceStaticPrefix) {
        $prefix = $sourceStaticPrefix
    } elseif (Test-LlvmPrefix $sourceSharedPrefix) {
        $prefix = $sourceSharedPrefix
    } elseif (Test-LlvmPrefix $sourceLegacyPrefix) {
        $prefix = $sourceLegacyPrefix
    } elseif (Test-LlvmPrefix $prebuiltPrefix) {
        $prefix = $prebuiltPrefix
    }
}

if ($null -eq $prefix) {
    throw "LLVM not found for Flavor=$Flavor and Version=$Version. Run scripts/bootstrap-llvm.ps1 or scripts/bootstrap-llvm-source.ps1."
}

$linkMode = $null
if ($Flavor -eq "source-static") {
    $linkMode = "static"
} elseif ($Flavor -eq "source-shared") {
    $linkMode = "shared"
} else {
    if (Test-SharedLlvmC $prefix) {
        $linkMode = "shared"
    } else {
        $linkMode = "static"
    }
}

$env:LLVM_SYS_211_PREFIX = $prefix
$env:ONDA_LLVM_LINK_MODE = $linkMode
$env:PATH = (Join-Path $prefix "bin") + ";" + $env:PATH
Write-Host "LLVM env configured for this shell: $prefix"
Write-Host "ONDA_LLVM_LINK_MODE = $linkMode"
