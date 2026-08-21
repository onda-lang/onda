param(
    [switch]$Serve
)

$ErrorActionPreference = "Stop"

$demoDir = Resolve-Path $PSScriptRoot
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..\..\..")
$compilerPackage = Join-Path $repoRoot "packages\onda_wasm_compiler"
$webAudioPackage = Join-Path $repoRoot "packages\onda_webaudio"
$abiPackage = Join-Path $repoRoot "packages\onda_processor_abi"
$compilerOut = Join-Path $demoDir "onda-wasm-compiler"
$webAudioOut = Join-Path $demoDir "onda-webaudio"

if (-not (Get-Command wasm-pack -ErrorAction SilentlyContinue)) {
    throw "wasm-pack is required. Install it from https://rustwasm.github.io/wasm-pack/installer/."
}

$binaryenJs = node -p "require.resolve('binaryen', { paths: [process.argv[1]] })" $compilerPackage
if ($LASTEXITCODE -ne 0) {
    npm ci --prefix $repoRoot
    $binaryenJs = node -p "require.resolve('binaryen', { paths: [process.argv[1]] })" $compilerPackage
}

npm run build --prefix $compilerPackage

Remove-Item $compilerOut, $webAudioOut -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item (Join-Path $demoDir "onda-webaudio.js"), (Join-Path $demoDir "onda-wasm-processor.js") -Force -ErrorAction SilentlyContinue
New-Item $compilerOut, $webAudioOut -ItemType Directory | Out-Null
Copy-Item (Join-Path $compilerPackage "src") (Join-Path $compilerOut "src") -Recurse
Copy-Item (Join-Path $compilerPackage "dist") (Join-Path $compilerOut "dist") -Recurse
Copy-Item $binaryenJs (Join-Path $compilerOut "dist\backend\binaryen.js") -Force
Copy-Item (Join-Path $abiPackage "src\index.js") (Join-Path $compilerOut "src\processor-abi.js") -Force
Copy-Item (Join-Path $abiPackage "src\index.js") (Join-Path $compilerOut "dist\backend\processor-abi.js") -Force
$compilerSource = Get-Content (Join-Path $compilerPackage "src\index.js") -Raw
$compilerSource.Replace('from "#onda-frontend-loader"', 'from "./frontend-browser.js"').Replace('from "@onda-lang/processor-abi"', 'from "./processor-abi.js"') |
    Set-Content (Join-Path $compilerOut "src\index.js") -NoNewline
$backendSource = Get-Content (Join-Path $compilerPackage "dist\backend\index.js") -Raw
$backendSource.Replace('from "binaryen"', 'from "./binaryen.js"') |
    Set-Content (Join-Path $compilerOut "dist\backend\index.js") -NoNewline
$backendArtifactSource = Get-Content (Join-Path $compilerPackage "dist\backend\artifact.js") -Raw
$backendArtifactSource.Replace('from "@onda-lang/processor-abi"', 'from "./processor-abi.js"') |
    Set-Content (Join-Path $compilerOut "dist\backend\artifact.js") -NoNewline
Copy-Item (Join-Path $webAudioPackage "src\worklet.js") (Join-Path $webAudioOut "worklet.js") -Force
Copy-Item (Join-Path $repoRoot "ui\run\run.html") (Join-Path $demoDir "run.html") -Force
Copy-Item (Join-Path $abiPackage "src\index.js") (Join-Path $webAudioOut "processor-abi.js") -Force
$webAudioSource = Get-Content (Join-Path $webAudioPackage "src\index.js") -Raw
$webAudioSource.Replace('from "@onda-lang/processor-abi"', 'from "./processor-abi.js"') |
    Set-Content (Join-Path $webAudioOut "index.js") -NoNewline
node (Join-Path $repoRoot "scripts\bundle-web-playground.mjs") (Join-Path $demoDir "playground.js")

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
