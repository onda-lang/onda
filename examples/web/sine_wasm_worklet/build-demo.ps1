param(
    [int]$SampleRate = 48000,
    [int]$BlockSize = 128,
    [switch]$Serve
)

$ErrorActionPreference = "Stop"

$demoDir = Resolve-Path $PSScriptRoot
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..\..\..")
$sourceFile = Join-Path $demoDir "sine_wasm.onda"
$objectFile = Join-Path $demoDir "sine_wasm.o"
$metaFile = Join-Path $demoDir "sine_wasm.onda.json"
$wasmFile = Join-Path $demoDir "sine_wasm.wasm"

function Invoke-Onda {
    param(
        [Parameter(ValueFromRemainingArguments = $true)]
        [string[]]$Args
    )

    $packagedOnda = Join-Path $repoRoot "bin\onda.exe"
    $releaseOnda = Join-Path $repoRoot "target\release\onda.exe"

    if (Test-Path $packagedOnda) {
        & $packagedOnda @Args
        return
    }

    $pathOnda = Get-Command onda.exe -ErrorAction SilentlyContinue
    if ($pathOnda) {
        & $pathOnda.Source @Args
        return
    }

    if (Test-Path $releaseOnda) {
        & $releaseOnda @Args
        return
    }

    if (-not (Test-Path (Join-Path $repoRoot "Cargo.toml"))) {
        throw "onda not found in bin/ or PATH, and this demo is not running from a source checkout with Cargo.toml."
    }

    $useLlvmEnv = Join-Path $repoRoot "scripts\use-llvm-env.ps1"
    if (Test-Path $useLlvmEnv) {
        . $useLlvmEnv -Flavor auto -Version "21.1.2"
    }

    cargo build --release -p onda_cli
    & $releaseOnda @Args
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
    Invoke-Onda compile $sourceFile --emit obj --target wasm32-unknown-unknown --sample-rate $SampleRate --block $BlockSize --output $objectFile --meta-out $metaFile

    $wasmLd = Get-WasmLd
    & $wasmLd $objectFile `
        --no-entry `
        --export=onda_init `
        --export=onda_process `
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
