param(
    [switch]$Serve
)

$ErrorActionPreference = "Stop"

$demoDir = Resolve-Path $PSScriptRoot
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..\..\..")
$compilerPackage = Join-Path $repoRoot "packages\onda_wasm_compiler"
$webAudioPackage = Join-Path $repoRoot "packages\onda_webaudio"
$compilerOut = Join-Path $demoDir "onda-wasm-compiler"
$webAudioOut = Join-Path $demoDir "onda-webaudio"
$binaryenJs = Join-Path $compilerPackage "node_modules\binaryen\index.js"

if (-not (Get-Command wasm-pack -ErrorAction SilentlyContinue)) {
    throw "wasm-pack is required. Install it from https://rustwasm.github.io/wasm-pack/installer/."
}

if (-not (Test-Path $binaryenJs)) {
    npm ci --prefix $compilerPackage
}

npm run build --prefix $compilerPackage

Remove-Item $compilerOut, $webAudioOut -Recurse -Force -ErrorAction SilentlyContinue
New-Item $compilerOut, $webAudioOut -ItemType Directory | Out-Null
Copy-Item (Join-Path $compilerPackage "src") (Join-Path $compilerOut "src") -Recurse
Copy-Item (Join-Path $compilerPackage "dist") (Join-Path $compilerOut "dist") -Recurse
Copy-Item $binaryenJs (Join-Path $compilerOut "dist\backend\binaryen.js") -Force
$compilerSource = Get-Content (Join-Path $compilerPackage "src\index.js") -Raw
$compilerSource.Replace('from "#onda-frontend-loader"', 'from "./frontend-browser.js"') |
    Set-Content (Join-Path $compilerOut "src\index.js") -NoNewline
$backendSource = Get-Content (Join-Path $compilerPackage "dist\backend\index.js") -Raw
$backendSource.Replace('from "binaryen"', 'from "./binaryen.js"') |
    Set-Content (Join-Path $compilerOut "dist\backend\index.js") -NoNewline
Copy-Item (Join-Path $webAudioPackage "src\*.js") $webAudioOut -Force

Write-Host "Staged @onda-lang/wasm-compiler in: $compilerOut"
Write-Host "Staged @onda-lang/webaudio in: $webAudioOut"

if ($Serve) {
    Push-Location $demoDir
    try {
        node .\server.mjs
    } finally {
        Pop-Location
    }
}
