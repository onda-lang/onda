param(
    [switch]$Serve
)

$ErrorActionPreference = "Stop"

$demoDir = Resolve-Path $PSScriptRoot
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..\..\..")
$backendDir = Join-Path $repoRoot "packages\onda_binaryen_web"
$webAudioDir = Join-Path $repoRoot "packages\onda_webaudio"
$sourceFile = Join-Path $repoRoot "examples\buffers-fft-convolution\sample_player.onda"
$mirFile = Join-Path $demoDir "sample-player.mir.msgpack"
$binaryenJs = Join-Path $backendDir "node_modules\binaryen\index.js"

if (-not (Test-Path $binaryenJs)) {
    npm install --prefix $backendDir
}

cargo run --quiet --release `
    --manifest-path (Join-Path $repoRoot "Cargo.toml") `
    -p onda_compiler_web `
    --example compile_file_to_mir `
    -- $sourceFile $mirFile 48000 128

node (Join-Path $demoDir "build-artifact.mjs") $mirFile $demoDir

Copy-Item (Join-Path $backendDir "src\artifact.js") (Join-Path $demoDir "artifact.js") -Force
Copy-Item (Join-Path $webAudioDir "src\index.js") (Join-Path $demoDir "onda-webaudio.js") -Force
Copy-Item (Join-Path $webAudioDir "src\worklet.js") (Join-Path $demoDir "onda-wasm-processor.js") -Force
Copy-Item `
    (Join-Path $repoRoot "examples\buffers-fft-convolution\impulse.wav") `
    (Join-Path $demoDir "impulse.wav") `
    -Force
node (Join-Path $demoDir "smoke-test.mjs")

Write-Host "Built the precompiled sample-player artifact in: $demoDir"

if ($Serve) {
    Push-Location $demoDir
    try {
        node .\server.mjs
    } finally {
        Pop-Location
    }
}
