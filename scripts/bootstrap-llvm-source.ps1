param(
    [string]$Version = "21.1.2",
    [string]$Config = "Release",
    [string]$PythonExecutable = ""
)

$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$depsRoot = Join-Path $repoRoot ".deps"
$srcRoot = Join-Path $depsRoot "src"
$buildRoot = Join-Path $depsRoot ("build-llvm-" + $Version)
$installRoot = Join-Path $depsRoot ("llvm-src/" + $Version)
$distRoot = Join-Path $depsRoot "dist"

$archive = Join-Path $distRoot ("llvm-project-llvmorg-" + $Version + ".tar.gz")
$url = "https://github.com/llvm/llvm-project/archive/refs/tags/llvmorg-$Version.tar.gz"

New-Item -ItemType Directory -Force -Path $depsRoot | Out-Null
New-Item -ItemType Directory -Force -Path $srcRoot | Out-Null
New-Item -ItemType Directory -Force -Path $distRoot | Out-Null

if (-not (Test-Path $archive)) {
    Write-Host "Downloading LLVM source archive: $url"
    Invoke-WebRequest -Uri $url -OutFile $archive
}

$extractRoot = Join-Path $srcRoot ("llvm-project-" + $Version)
if (-not (Test-Path $extractRoot)) {
    Write-Host "Extracting LLVM sources..."
    tar -xf $archive -C $srcRoot
    $extracted = Get-ChildItem -Path $srcRoot -Directory | Where-Object { $_.Name -like "llvm-project-llvmorg-*" } | Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $extracted) {
        throw "Failed to locate extracted llvm-project source directory"
    }
    Move-Item -Path $extracted.FullName -Destination $extractRoot
}

$llvmSrc = Join-Path $extractRoot "llvm"
if (-not (Test-Path $llvmSrc)) {
    throw "LLVM source tree not found at $llvmSrc"
}

$generator = "Ninja"
$haveNinja = $null -ne (Get-Command ninja -ErrorAction SilentlyContinue)
if (-not $haveNinja) {
    $generator = "Visual Studio 17 2022"
}

if ([string]::IsNullOrWhiteSpace($PythonExecutable)) {
    $pyenvPath = Join-Path $HOME ".pyenv/pyenv-win/versions/3.10.11/python.exe"
    if (Test-Path $pyenvPath) {
        $PythonExecutable = $pyenvPath
    }
}

Write-Host "Configuring LLVM with generator: $generator"

$cmakeArgs = @(
    "-S", $llvmSrc,
    "-B", $buildRoot,
    "-G", $generator,
    "-DCMAKE_INSTALL_PREFIX=$installRoot",
    "-DCMAKE_BUILD_TYPE=$Config",
    "-DLLVM_ENABLE_PROJECTS=",
    "-DLLVM_TARGETS_TO_BUILD=X86",
    "-DLLVM_BUILD_LLVM_C_DYLIB=ON",
    "-DLLVM_LINK_LLVM_DYLIB=ON",
    "-DLLVM_INCLUDE_TESTS=OFF",
    "-DLLVM_INCLUDE_BENCHMARKS=OFF",
    "-DLLVM_INCLUDE_EXAMPLES=OFF",
    "-DLLVM_INCLUDE_DOCS=OFF"
)

if (-not [string]::IsNullOrWhiteSpace($PythonExecutable)) {
    $cmakeArgs += "-DPython3_EXECUTABLE=$PythonExecutable"
}

cmake @cmakeArgs

if ($generator -eq "Ninja") {
    cmake --build $buildRoot --target install --config $Config
} else {
    cmake --build $buildRoot --target INSTALL --config $Config
}

if (-not (Test-Path (Join-Path $installRoot "bin/llvm-config.exe"))) {
    throw "llvm-config.exe not found after source build install"
}

Write-Host "LLVM source build installed at: $installRoot"
Write-Host "Use this in your shell:"
Write-Host "`$env:LLVM_SYS_211_PREFIX = '$installRoot'"
