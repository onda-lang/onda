param(
    [switch]$Check
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$arguments = @()
if ($Check) {
    $arguments += "--check"
}

& node (Join-Path $repoRoot "scripts\sync-package-versions.mjs") @arguments
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
