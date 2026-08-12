$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot

& cargo run --quiet `
    --manifest-path (Join-Path $repoRoot "Cargo.toml") `
    -p onda_lsp `
    --example generate_stdlib_docs
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
