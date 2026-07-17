param(
    [switch]$Serve
)

$ErrorActionPreference = "Stop"

$demoDir = Resolve-Path $PSScriptRoot
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..\..\..")
$backendDir = Join-Path $repoRoot "packages\onda_binaryen_web"
$compilerDir = Join-Path $repoRoot "crates\onda_compiler_web"
$compilerOut = Join-Path $demoDir "onda-compiler-web"
$binaryenJs = Join-Path $backendDir "node_modules\binaryen\index.js"

if (-not (Get-Command wasm-pack -ErrorAction SilentlyContinue)) {
    throw "wasm-pack is required. Install it from https://rustwasm.github.io/wasm-pack/installer/."
}

if (-not (Test-Path $binaryenJs)) {
    npm install --prefix $backendDir
}

wasm-pack build $compilerDir `
    --target web `
    --release `
    --out-dir $compilerOut `
    --out-name onda_compiler_web

Copy-Item $binaryenJs (Join-Path $demoDir "binaryen.js") -Force
Copy-Item (Join-Path $backendDir "src\index.js") (Join-Path $demoDir "onda-binaryen-web.js") -Force
Copy-Item (Join-Path $backendDir "src\math-kernel.generated.js") (Join-Path $demoDir "math-kernel.generated.js") -Force
Copy-Item (Join-Path $backendDir "src\messagepack.js") (Join-Path $demoDir "messagepack.js") -Force

Write-Host "Built the in-browser Onda compiler in: $compilerOut"
Write-Host "Staged the Binaryen backend in: $demoDir"

if ($Serve) {
    Push-Location $demoDir
    try {
        node .\server.mjs
    } finally {
        Pop-Location
    }
}
