param(
    [Parameter(Mandatory = $true)]
    [string]$Module
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$output = & llvm-readobj --coff-exports $Module
if ($LASTEXITCODE -ne 0) {
    throw "llvm-readobj failed to inspect '$Module'"
}

$exports = @(
    $output | ForEach-Object {
        if ($_ -match '^\s*Name:\s+(\S+)\s*$') {
            $Matches[1]
        }
    }
)

$expected = "onda_cmake_sdk_smoke"
if ($exports.Count -ne 1 -or $exports[0] -ne $expected) {
    $actual = if ($exports.Count -eq 0) {
        "<none>"
    } else {
        $exports -join ", "
    }
    throw "Expected only '$expected' to be exported from '$Module'; found: $actual"
}
