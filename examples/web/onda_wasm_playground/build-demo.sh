#!/usr/bin/env bash
set -euo pipefail

serve=0

usage() {
  cat <<'EOF'
Usage: examples/web/onda_wasm_playground/build-demo.sh [--serve]

Requires wasm-pack and npm. The resulting embedded-compiler playground compiles
Onda source in the browser; it does not invoke the native onda CLI.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --serve)
      serve=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

demo_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$demo_dir/../../.." && pwd)"
compiler_package="$repo_root/packages/onda_wasm_compiler"
webaudio_package="$repo_root/packages/onda_webaudio"
compiler_out="$demo_dir/onda-wasm-compiler"
webaudio_out="$demo_dir/onda-webaudio"
binaryen_js="$compiler_package/node_modules/binaryen/index.js"

if ! command -v wasm-pack >/dev/null 2>&1; then
  echo "wasm-pack is required. Install it from https://rustwasm.github.io/wasm-pack/installer/." >&2
  exit 1
fi

if [[ ! -f "$binaryen_js" ]]; then
  npm ci --prefix "$compiler_package"
fi

npm run build --prefix "$compiler_package"

rm -rf "$compiler_out" "$webaudio_out"
mkdir -p "$compiler_out" "$webaudio_out"
cp -R "$compiler_package/src" "$compiler_out/src"
cp -R "$compiler_package/dist" "$compiler_out/dist"
cp "$binaryen_js" "$compiler_out/dist/backend/binaryen.js"
sed 's/from "#onda-frontend-loader"/from ".\/frontend-browser.js"/' \
  "$compiler_package/src/index.js" > "$compiler_out/src/index.js"
sed 's/from "binaryen"/from ".\/binaryen.js"/' \
  "$compiler_package/dist/backend/index.js" > "$compiler_out/dist/backend/index.js"
cp "$webaudio_package/src/"*.js "$webaudio_out/"

echo "Staged @onda-lang/wasm-compiler in: $compiler_out"
echo "Staged @onda-lang/webaudio in: $webaudio_out"

if [[ "$serve" == "1" ]]; then
  pushd "$demo_dir" >/dev/null
  node ./server.mjs
  popd >/dev/null
fi
