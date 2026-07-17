#!/usr/bin/env bash
set -euo pipefail

serve=0

usage() {
  cat <<'EOF'
Usage: examples/web/sine_wasm_worklet/build-demo.sh [--serve]

Requires wasm-pack and npm. The resulting playground compiles Onda source in
the browser; it does not invoke the native onda CLI.
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
backend_dir="$repo_root/packages/onda_binaryen_web"
compiler_dir="$repo_root/crates/onda_compiler_web"
compiler_out="$demo_dir/onda-compiler-web"
binaryen_js="$backend_dir/node_modules/binaryen/index.js"

if ! command -v wasm-pack >/dev/null 2>&1; then
  echo "wasm-pack is required. Install it from https://rustwasm.github.io/wasm-pack/installer/." >&2
  exit 1
fi

if [[ ! -f "$binaryen_js" ]]; then
  npm install --prefix "$backend_dir"
fi

wasm-pack build "$compiler_dir" \
  --target web \
  --release \
  --out-dir "$compiler_out" \
  --out-name onda_compiler_web

cp "$binaryen_js" "$demo_dir/binaryen.js"
cp "$backend_dir/src/index.js" "$demo_dir/onda-binaryen-web.js"
cp "$backend_dir/src/exact-math.js" "$demo_dir/exact-math.js"
cp "$backend_dir/src/messagepack.js" "$demo_dir/messagepack.js"

echo "Built the in-browser Onda compiler in: $compiler_out"
echo "Staged the Binaryen backend in: $demo_dir"

if [[ "$serve" == "1" ]]; then
  pushd "$demo_dir" >/dev/null
  node ./server.mjs
  popd >/dev/null
fi
