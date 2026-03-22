param(
    [int]$SampleRate = 48000,
    [int]$BlockSize = 128,
    [switch]$Serve
)

$ErrorActionPreference = "Stop"

$demoDir = Resolve-Path $PSScriptRoot
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..\..\..")
$sourceFile = Join-Path $repoRoot "examples\sine_wasm.omni"
$objectFile = Join-Path $demoDir "sine_wasm.o"
$metaFile = Join-Path $demoDir "sine_wasm.omni.json"
$wasmFile = Join-Path $demoDir "sine_wasm.wasm"

function Set-LlvmEnv {
    if ($env:LLVM_SYS_211_PREFIX -and (Test-Path (Join-Path $env:LLVM_SYS_211_PREFIX "bin\llvm-config.exe"))) {
        $prefixBin = Join-Path $env:LLVM_SYS_211_PREFIX "bin"
        if (-not (($env:PATH -split ";") -contains $prefixBin)) {
            $env:PATH = $prefixBin + ";" + $env:PATH
        }
        if (-not $env:OMNI_LLVM_LINK_MODE) {
            $env:OMNI_LLVM_LINK_MODE = if (Test-Path (Join-Path $env:LLVM_SYS_211_PREFIX "bin\LLVM-C.dll")) { "shared" } else { "static" }
        }
        Write-Host "LLVM env configured for this shell: $env:LLVM_SYS_211_PREFIX"
        Write-Host "OMNI_LLVM_LINK_MODE = $env:OMNI_LLVM_LINK_MODE"
        return
    }

    $fallbackPrefixes = @(
        (Join-Path $repoRoot ".deps\llvm-src\21.1.2-static"),
        (Join-Path $repoRoot ".deps\llvm-src\21.1.2-shared"),
        (Join-Path $repoRoot ".deps\build-llvm-21.1.2-static\Release"),
        (Join-Path $repoRoot ".deps\build-llvm-21.1.2-shared\Release"),
        (Join-Path $repoRoot ".deps\llvm\21.1.2")
    )

    foreach ($prefix in $fallbackPrefixes) {
        if (-not (Test-Path (Join-Path $prefix "bin\llvm-config.exe"))) {
            continue
        }

        $env:LLVM_SYS_211_PREFIX = $prefix
        $env:OMNI_LLVM_LINK_MODE = if (Test-Path (Join-Path $prefix "bin\LLVM-C.dll")) { "shared" } else { "static" }
        $prefixBin = Join-Path $prefix "bin"
        if (-not (($env:PATH -split ";") -contains $prefixBin)) {
            $env:PATH = $prefixBin + ";" + $env:PATH
        }
        Write-Host "LLVM env configured for this shell: $prefix"
        Write-Host "OMNI_LLVM_LINK_MODE = $env:OMNI_LLVM_LINK_MODE"
        return
    }

    throw "LLVM not found. Run scripts/bootstrap-llvm-source.ps1 or scripts/bootstrap-llvm.ps1 first."
}

function Get-WasmLd {
    $sysroot = (& rustc --print sysroot).Trim()
    $candidate = Join-Path $sysroot "lib\rustlib\x86_64-pc-windows-msvc\bin\gcc-ld\wasm-ld.exe"
    if (Test-Path $candidate) {
        return $candidate
    }

    $command = Get-Command wasm-ld.exe -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    throw "wasm-ld.exe not found. Install the Rust toolchain or add wasm-ld to PATH."
}

Push-Location $repoRoot
try {
    Set-LlvmEnv

    cargo run -p omni_cli -- compile $sourceFile --emit obj --target wasm32-unknown-unknown --sample-rate $SampleRate --block $BlockSize --output $objectFile --meta-out $metaFile

    $wasmLd = Get-WasmLd
    & $wasmLd $objectFile `
        --no-entry `
        --export=omni_init `
        --export=omni_process `
        --export=__heap_base `
        --export-memory `
        --initial-memory=131072 `
        --no-growable-memory `
        -o $wasmFile

    Write-Host "Wrote object: $objectFile"
    Write-Host "Wrote metadata: $metaFile"
    Write-Host "Wrote wasm: $wasmFile"

    if ($Serve) {
        Push-Location $demoDir
        try {
            node .\server.mjs
        } finally {
            Pop-Location
        }
    }
} finally {
    Pop-Location
}
