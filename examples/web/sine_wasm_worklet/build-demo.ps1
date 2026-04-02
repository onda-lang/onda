param(
    [int]$SampleRate = 48000,
    [int]$BlockSize = 128,
    [switch]$Serve
)

$ErrorActionPreference = "Stop"

$demoDir = Resolve-Path $PSScriptRoot
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..\..\..")
$sourceFile = Join-Path $demoDir "sine_wasm.omni"
$objectFile = Join-Path $demoDir "sine_wasm.o"
$metaFile = Join-Path $demoDir "sine_wasm.omni.json"
$wasmFile = Join-Path $demoDir "sine_wasm.wasm"

function Invoke-Omni {
    param(
        [Parameter(ValueFromRemainingArguments = $true)]
        [string[]]$Args
    )

    $packagedOmni = Join-Path $repoRoot "bin\omni.exe"
    $releaseOmni = Join-Path $repoRoot "target\release\omni.exe"

    if (Test-Path $packagedOmni) {
        & $packagedOmni @Args
        return
    }

    $pathOmni = Get-Command omni.exe -ErrorAction SilentlyContinue
    if ($pathOmni) {
        & $pathOmni.Source @Args
        return
    }

    if (Test-Path $releaseOmni) {
        & $releaseOmni @Args
        return
    }

    if (-not (Test-Path (Join-Path $repoRoot "Cargo.toml"))) {
        throw "omni not found in bin/ or PATH, and this demo is not running from a source checkout with Cargo.toml."
    }

    $useLlvmEnv = Join-Path $repoRoot "scripts\use-llvm-env.ps1"
    if (Test-Path $useLlvmEnv) {
        . $useLlvmEnv -Flavor auto -Version "21.1.2"
    }

    cargo build --release -p omni_cli
    & $releaseOmni @Args
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
    Invoke-Omni compile $sourceFile --emit obj --target wasm32-unknown-unknown --sample-rate $SampleRate --block $BlockSize --output $objectFile --meta-out $metaFile

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
