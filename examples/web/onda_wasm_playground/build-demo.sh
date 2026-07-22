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
abi_package="$repo_root/packages/onda_processor_abi"
compiler_out="$demo_dir/onda-wasm-compiler"
webaudio_out="$demo_dir/onda-webaudio"

if ! command -v wasm-pack >/dev/null 2>&1; then
  echo "wasm-pack is required. Install it from https://rustwasm.github.io/wasm-pack/installer/." >&2
  exit 1
fi

if ! binaryen_js="$(node -p "require.resolve('binaryen', { paths: [process.argv[1]] })" "$compiler_package")"; then
  npm ci --prefix "$repo_root"
  binaryen_js="$(node -p "require.resolve('binaryen', { paths: [process.argv[1]] })" "$compiler_package")"
fi

npm run build --prefix "$compiler_package"

rm -rf "$compiler_out" "$webaudio_out"
mkdir -p "$compiler_out" "$webaudio_out"
cp -R "$compiler_package/src" "$compiler_out/src"
cp -R "$compiler_package/dist" "$compiler_out/dist"
cp "$binaryen_js" "$compiler_out/dist/backend/binaryen.js"
cp "$abi_package/src/index.js" "$compiler_out/src/processor-abi.js"
cp "$abi_package/src/index.js" "$compiler_out/dist/backend/processor-abi.js"
sed \
  -e 's/from "#onda-frontend-loader"/from ".\/frontend-browser.js"/' \
  -e 's/from "@onda-lang\/processor-abi"/from ".\/processor-abi.js"/' \
  "$compiler_package/src/index.js" > "$compiler_out/src/index.js"
sed 's/from "binaryen"/from ".\/binaryen.js"/' \
  "$compiler_package/dist/backend/index.js" > "$compiler_out/dist/backend/index.js"
sed 's/from "@onda-lang\/processor-abi"/from ".\/processor-abi.js"/' \
  "$compiler_package/dist/backend/artifact.js" > "$compiler_out/dist/backend/artifact.js"
cp "$webaudio_package/src/worklet.js" "$webaudio_out/worklet.js"
cp "$repo_root/ui/run/run.html" "$demo_dir/run.html"
cp "$abi_package/src/index.js" "$webaudio_out/processor-abi.js"
sed 's/from "@onda-lang\/processor-abi"/from ".\/processor-abi.js"/' \
  "$webaudio_package/src/index.js" > "$webaudio_out/index.js"
node "$repo_root/scripts/bundle-web-playground.mjs" "$demo_dir/playground.js"

echo "Staged @onda-lang/wasm-compiler in: $compiler_out"
echo "Staged @onda-lang/webaudio in: $webaudio_out"

if [[ "$serve" == "1" ]]; then
  pushd "$demo_dir" >/dev/null
  node ./server.mjs
  popd >/dev/null
fi
