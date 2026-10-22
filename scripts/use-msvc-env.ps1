# Dot-source this script to make MSVC and Windows SDK tools available in the current shell.
$ErrorActionPreference = "Stop"

$vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path $vswhere)) {
  throw "vswhere.exe not found at $vswhere"
}

$vsInstallPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vsInstallPath) {
  throw "Visual Studio with MSVC tools not found"
}

$devCmd = Join-Path $vsInstallPath "Common7\Tools\VsDevCmd.bat"
if (-not (Test-Path $devCmd)) {
  throw "VsDevCmd.bat not found at $devCmd"
}

$captureEnvScript = Join-Path ([System.IO.Path]::GetTempPath()) ("onda-msvc-" + [guid]::NewGuid().ToString("N") + ".cmd")
@"
@echo off
call "$devCmd" -arch=x64 -host_arch=x64 >nul
if errorlevel 1 exit /b %errorlevel%
set
"@ | Set-Content -Path $captureEnvScript -Encoding ascii

try {
  $msvcEnv = cmd.exe /d /c $captureEnvScript
  if ($LASTEXITCODE -ne 0) {
    throw "Failed to initialize MSVC build environment via VsDevCmd.bat"
  }

  foreach ($line in $msvcEnv) {
    if ($line -match '^(.*?)=(.*)$') {
      [System.Environment]::SetEnvironmentVariable($matches[1], $matches[2])
    }
  }
} finally {
  Remove-Item -LiteralPath $captureEnvScript
}
