#!/usr/bin/env bash
set -euo pipefail

sample_rate=48000
block_size=128
serve=0

usage() {
  cat <<'EOF'
Usage: examples/web/sine_wasm_worklet/build-demo.sh [--sample-rate <hz>] [--block-size <frames>] [--serve]
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --sample-rate)
      sample_rate="${2:-}"
      shift 2
      ;;
    --block-size)
      block_size="${2:-}"
      shift 2
      ;;
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

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/../../.." && pwd)"
source_file="$script_dir/sine_wasm.onda"
object_file="$script_dir/sine_wasm.o"
meta_file="$script_dir/sine_wasm.onda.json"
wasm_file="$script_dir/sine_wasm.wasm"

invoke_onda() {
  local packaged_onda="$repo_root/bin/onda"
  local release_onda="$repo_root/target/release/onda"

  if [[ -x "$packaged_onda" ]]; then
    "$packaged_onda" "$@"
    return
  fi

  if command -v onda >/dev/null 2>&1; then
    "$(command -v onda)" "$@"
    return
  fi

  if [[ -x "$release_onda" ]]; then
    "$release_onda" "$@"
    return
  fi

  if [[ ! -f "$repo_root/Cargo.toml" ]]; then
    echo "onda not found in bin/ or PATH, and this demo is not running from a source checkout with Cargo.toml." >&2
    exit 1
  fi

  if [[ -f "$repo_root/scripts/use-llvm-env.sh" ]]; then
    # shellcheck disable=SC1091
    source "$repo_root/scripts/use-llvm-env.sh" --flavor auto --version 21.1.2
  fi

  cargo build --release -p onda_cli
  "$release_onda" "$@"
}

get_wasm_ld() {
  local sysroot candidate
  sysroot="$(rustc --print sysroot)"

  while IFS= read -r candidate; do
    if [[ -x "$candidate" ]]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done < <(find "$sysroot/lib/rustlib" -path '*/bin/gcc-ld/wasm-ld' -type f 2>/dev/null)

  if command -v wasm-ld >/dev/null 2>&1; then
    command -v wasm-ld
    return 0
  fi

  echo "wasm-ld not found. Install the Rust toolchain or add wasm-ld to PATH." >&2
  exit 1
}

pushd "$repo_root" >/dev/null
invoke_onda compile "$source_file" \
  --emit obj \
  --target wasm32-unknown-unknown \
  --sample-rate "$sample_rate" \
  --block "$block_size" \
  --output "$object_file" \
  --meta-out "$meta_file"
popd >/dev/null

wasm_ld="$(get_wasm_ld)"
"$wasm_ld" "$object_file" \
  --no-entry \
  --export=onda_init \
  --export=onda_process \
  --export=__heap_base \
  --export-memory \
  --initial-memory=131072 \
  --no-growable-memory \
  -o "$wasm_file"

echo "Wrote object: $object_file"
echo "Wrote metadata: $meta_file"
echo "Wrote wasm: $wasm_file"

if [[ "$serve" == "1" ]]; then
  pushd "$script_dir" >/dev/null
  node ./server.mjs
  popd >/dev/null
fi
