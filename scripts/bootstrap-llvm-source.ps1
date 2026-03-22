param(
    [string]$Version = "21.1.2",
    [string]$Config = "Release",
    [string]$PythonExecutable = "",
    [ValidateSet("Static", "Shared")]
    [string]$Linkage = "Static",
    [string]$Targets = "X86",
    [ValidateSet("Auto", "Ninja", "VS2022")]
    [string]$Generator = "Auto"
)

$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$depsRoot = Join-Path $repoRoot ".deps"
$srcRoot = Join-Path $depsRoot "src"
$linkageLower = $Linkage.ToLowerInvariant()
$buildRoot = Join-Path $depsRoot ("build-llvm-" + $Version + "-" + $linkageLower)
$installRoot = Join-Path $depsRoot ("llvm-src/" + $Version + "-" + $linkageLower)
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
    tar -xf $archive -C $srcRoot 2>$null  # 2>$null hides the symlink creation errors from the console
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

function Test-CMakeGeneratorAvailable([string]$GeneratorName) {
    $help = & cmake --help 2>$null
    if ($LASTEXITCODE -ne 0 -or $null -eq $help) {
        return $false
    }
    return [bool]($help -match [regex]::Escape($GeneratorName))
}

$haveNinja = $null -ne (Get-Command ninja -ErrorAction SilentlyContinue)
$haveCompilerForNinja = ($null -ne (Get-Command cl -ErrorAction SilentlyContinue)) -or ($null -ne (Get-Command clang -ErrorAction SilentlyContinue)) -or ($null -ne (Get-Command clang-cl -ErrorAction SilentlyContinue))
$haveVs2022Generator = Test-CMakeGeneratorAvailable "Visual Studio 17 2022"

$resolvedGenerator = $null
if ($Generator -eq "Ninja") {
    $resolvedGenerator = "Ninja"
} elseif ($Generator -eq "VS2022") {
    $resolvedGenerator = "Visual Studio 17 2022"
} else {
    if ($haveNinja -and $haveCompilerForNinja) {
        $resolvedGenerator = "Ninja"
    } elseif ($haveVs2022Generator) {
        $resolvedGenerator = "Visual Studio 17 2022"
    } elseif ($haveNinja) {
        $resolvedGenerator = "Ninja"
    } else {
        throw "No suitable CMake generator found. Install Visual Studio 2022 Build Tools (Desktop C++) or provide a C/C++ compiler in PATH."
    }
}

if ($resolvedGenerator -eq "Ninja" -and -not $haveCompilerForNinja) {
    throw "Generator Ninja selected but no C/C++ compiler found (cl/clang/clang-cl). Open a VS Developer shell or run with -Generator VS2022."
}

if ($resolvedGenerator -eq "Visual Studio 17 2022" -and -not $haveVs2022Generator) {
    throw "CMake does not report generator 'Visual Studio 17 2022'. Install Visual Studio 2022 Build Tools or use -Generator Ninja with compiler configured."
}

if ([string]::IsNullOrWhiteSpace($PythonExecutable)) {
    $pyenvPath = Join-Path $HOME ".pyenv/pyenv-win/versions/3.10.11/python.exe"
    if (Test-Path $pyenvPath) {
        $PythonExecutable = $pyenvPath
    }
}

if ([string]::IsNullOrWhiteSpace($Targets)) {
    throw "Targets must not be empty. Use a semicolon-separated LLVM target list such as 'X86;AArch64;ARM;WebAssembly'."
}

Write-Host "Configuring LLVM with generator: $resolvedGenerator"
Write-Host "Requested LLVM linkage: $Linkage"
Write-Host "Requested LLVM targets: $Targets"

$cmakeArgs = @(
    "-S", $llvmSrc,
    "-B", $buildRoot,
    "-G", $resolvedGenerator,
    "-DCMAKE_INSTALL_PREFIX=$installRoot",
    "-DCMAKE_BUILD_TYPE=$Config",
    "-DLLVM_ENABLE_PROJECTS=",
    "-DLLVM_TARGETS_TO_BUILD=$Targets",
    "-DLLVM_INCLUDE_TESTS=OFF",
    "-DLLVM_INCLUDE_BENCHMARKS=OFF",
    "-DLLVM_INCLUDE_EXAMPLES=OFF",
    "-DLLVM_INCLUDE_DOCS=OFF"
)

if ($Linkage -eq "Static") {
    $cmakeArgs += @(
        "-DBUILD_SHARED_LIBS=OFF",
        "-DLLVM_BUILD_LLVM_DYLIB=OFF",
        "-DLLVM_BUILD_LLVM_C_DYLIB=OFF",
        "-DLLVM_LINK_LLVM_DYLIB=OFF"
    )
} else {
    $cmakeArgs += @(
        "-DBUILD_SHARED_LIBS=OFF",
        "-DLLVM_BUILD_LLVM_DYLIB=ON",
        "-DLLVM_BUILD_LLVM_C_DYLIB=ON",
        "-DLLVM_LINK_LLVM_DYLIB=ON"
    )
}

if (-not [string]::IsNullOrWhiteSpace($PythonExecutable)) {
    $cmakeArgs += "-DPython3_EXECUTABLE=$PythonExecutable"
}

$cachePath = Join-Path $buildRoot "CMakeCache.txt"
if (Test-Path $cachePath) {
    $cacheGeneratorLine = Get-Content $cachePath | Where-Object { $_ -like "CMAKE_GENERATOR:INTERNAL=*" } | Select-Object -First 1
    if ($null -ne $cacheGeneratorLine) {
        $cacheGenerator = $cacheGeneratorLine.Substring("CMAKE_GENERATOR:INTERNAL=".Length)
        if ($cacheGenerator -ne $resolvedGenerator) {
            Write-Host "Recreating build dir due to generator change ($cacheGenerator -> $resolvedGenerator)"
            Remove-Item -Recurse -Force $buildRoot
        }
    }
}

cmake @cmakeArgs

if ($resolvedGenerator -eq "Ninja") {
    cmake --build $buildRoot --target install --config $Config
} else {
    cmake --build $buildRoot --target INSTALL --config $Config
}

if (-not (Test-Path (Join-Path $installRoot "bin/llvm-config.exe"))) {
    throw "llvm-config.exe not found after source build install"
}

if ($Linkage -eq "Static") {
    $coreLib = Join-Path $installRoot "lib/LLVMCore.lib"
    if (-not (Test-Path $coreLib)) {
        throw "Static LLVM core library not found at $coreLib"
    }
}

Write-Host "LLVM source build installed at: $installRoot"
Write-Host "Use this in your shell:"
Write-Host "`$env:LLVM_SYS_211_PREFIX = '$installRoot'"
