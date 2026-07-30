param(
    [Parameter(Mandatory = $true)]
    [string]$Consumer
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$output = & llvm-readobj --coff-imports $Consumer
if ($LASTEXITCODE -ne 0) {
    throw "llvm-readobj failed to inspect '$Consumer'"
}

$imports = @(
    $output | ForEach-Object {
        if ($_ -match '^\s*Name:\s+(\S+)\s*$') {
            $Matches[1]
        }
    }
)

$expected = "onda.dll"
$ondaImports = @($imports | Where-Object { $_ -match '(?i)onda[.]dll$' })
if ($ondaImports.Count -ne 1 -or $ondaImports[0] -cne $expected) {
    $actual = if ($ondaImports.Count -eq 0) {
        "<none>"
    } else {
        $ondaImports -join ", "
    }
    throw "Expected dependency '$expected' in '$Consumer'; found: $actual"
}
