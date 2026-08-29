#!/usr/bin/env bash
set -euo pipefail

serve=0

usage() {
  cat <<'EOF'
Usage: examples/web/onda_wasm_aot_sample_player/build-demo.sh [--serve]

Builds the sample player to an executable Wasm artifact before serving the
page. The browser loads the finished artifact and does not contain a compiler.
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
webaudio_dir="$repo_root/packages/onda_webaudio"
abi_dir="$repo_root/packages/onda_processor_abi"
source_file="$repo_root/examples/buffers/sample_player.onda"
mir_file="$demo_dir/sample-player.mir.msgpack"

if ! node -p "require.resolve('binaryen', { paths: [process.argv[1]] })" "$backend_dir" >/dev/null; then
  npm ci --prefix "$repo_root"
fi

cargo run --quiet --release \
  --manifest-path "$repo_root/Cargo.toml" \
  -p onda_compiler_web \
  --example compile_file_to_mir \
  -- "$source_file" "$mir_file" 48000 128

node "$demo_dir/build-artifact.mjs" "$mir_file" "$demo_dir"

cp "$abi_dir/src/index.js" "$demo_dir/artifact.js"
cp "$abi_dir/src/param-control.js" "$demo_dir/param-control.js"
cp "$webaudio_dir/src/index.js" "$demo_dir/onda-webaudio.js"
cp "$webaudio_dir/src/worklet.js" "$demo_dir/onda-wasm-processor.js"
cp "$webaudio_dir/src/execution-output-ring.js" "$demo_dir/execution-output-ring.js"
cp "$repo_root/examples/projects/embedded_room/assets/impulse.wav" "$demo_dir/impulse.wav"
node "$demo_dir/smoke-test.mjs"

echo "Built the precompiled sample-player artifact in: $demo_dir"

if [[ "$serve" == "1" ]]; then
  pushd "$demo_dir" >/dev/null
  node ./server.mjs
  popd >/dev/null
fi
