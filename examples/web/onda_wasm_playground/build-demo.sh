#!/usr/bin/env bash
set -euo pipefail

serve=0
skip_build=0

usage() {
  cat <<'EOF'
Usage: examples/web/onda_wasm_playground/build-demo.sh [--serve] [--skip-build]

Requires wasm-pack and npm. The resulting embedded-compiler playground compiles
Onda source in the browser; it does not invoke the native onda CLI.
Use --skip-build after an initial build to refresh UI assets without rebuilding
the Rust compiler. The existing compiler build must match the package version.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --serve)
      serve=1
      shift
      ;;
    --skip-build)
      skip_build=1
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

if [[ "$skip_build" == "0" ]] && ! command -v wasm-pack >/dev/null 2>&1; then
  echo "wasm-pack is required. Install it from https://rustwasm.github.io/wasm-pack/installer/." >&2
  exit 1
fi

if ! binaryen_js="$(node -p "require.resolve('binaryen', { paths: [process.argv[1]] })" "$compiler_package")"; then
  npm ci --prefix "$repo_root"
  binaryen_js="$(node -p "require.resolve('binaryen', { paths: [process.argv[1]] })" "$compiler_package")"
fi

if [[ "$skip_build" == "1" ]]; then
  node -e '
    const root = process.argv[1];
    const { version } = require(`${root}/package.json`);
    const { ondaVersion } = require(`${root}/dist/build.json`);
    if (ondaVersion !== version) {
      throw new Error("Compiler assets are stale; rerun without --skip-build.");
    }
  ' "$compiler_package"
else
  npm run build --prefix "$compiler_package"
fi

rm -rf "$compiler_out" "$webaudio_out"
rm -f "$demo_dir/onda-webaudio.js" "$demo_dir/onda-wasm-processor.js"
mkdir -p "$compiler_out" "$webaudio_out"
cp -R "$compiler_package/src" "$compiler_out/src"
cp -R "$compiler_package/dist" "$compiler_out/dist"
cp "$binaryen_js" "$compiler_out/dist/backend/binaryen.js"
cp "$abi_package/src/index.js" "$compiler_out/src/processor-abi.js"
cp "$abi_package/src/param-control.js" "$compiler_out/src/param-control.js"
cp "$abi_package/src/payload.js" "$compiler_out/src/payload.js"
cp "$abi_package/src/index.js" "$compiler_out/dist/backend/processor-abi.js"
cp "$abi_package/src/param-control.js" "$compiler_out/dist/backend/param-control.js"
cp "$abi_package/src/payload.js" "$compiler_out/dist/backend/payload.js"
sed \
  -e 's/from "#onda-frontend-loader"/from ".\/frontend-browser.js"/' \
  -e 's/from "@onda-lang\/processor-abi"/from ".\/processor-abi.js"/' \
  "$compiler_package/src/index.js" > "$compiler_out/src/index.js"
sed 's/from "binaryen"/from ".\/binaryen.js"/' \
  "$compiler_package/dist/backend/index.js" > "$compiler_out/dist/backend/index.js"
sed 's/from "@onda-lang\/processor-abi"/from ".\/processor-abi.js"/' \
  "$compiler_package/dist/backend/artifact.js" > "$compiler_out/dist/backend/artifact.js"
cp "$webaudio_package/src/worklet.js" "$webaudio_out/worklet.js"
cp "$webaudio_package/src/processor-constants.js" \
  "$webaudio_out/processor-constants.js"
cp "$webaudio_package/src/param-metadata.js" "$webaudio_out/param-metadata.js"
cp "$webaudio_package/src/execution-output-ring.js" \
  "$webaudio_out/execution-output-ring.js"
cp "$repo_root/ui/run/run.html" "$demo_dir/run.html"
cp "$repo_root/ui/number-input.js" "$demo_dir/number-input.js"
cp "$abi_package/src/param-control.js" "$demo_dir/param-control.js"
cp "$abi_package/src/payload.js" "$demo_dir/payload.js"
cp "$abi_package/src/index.js" "$webaudio_out/processor-abi.js"
cp "$abi_package/src/param-control.js" "$webaudio_out/param-control.js"
cp "$abi_package/src/payload.js" "$webaudio_out/payload.js"
sed 's/from "@onda-lang\/processor-abi"/from ".\/processor-abi.js"/' \
  "$webaudio_package/src/index.js" > "$webaudio_out/index.js"
node "$repo_root/scripts/bundle-web-playground.mjs" "$demo_dir/playground.js" \
  "$compiler_out/src/worker.js"

echo "Staged @onda-lang/wasm-compiler in: $compiler_out"
echo "Staged @onda-lang/webaudio in: $webaudio_out"

if [[ "$serve" == "1" ]]; then
  pushd "$demo_dir" >/dev/null
  node ./server.mjs
  popd >/dev/null
fi
