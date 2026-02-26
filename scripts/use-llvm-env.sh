#!/usr/bin/env bash
set -euo pipefail

FLAVOR="source-static"
VERSION="21.1.2"

usage() {
  cat <<'EOF'
Usage: source scripts/use-llvm-env.sh [--flavor <auto|prebuilt|source-static|source-shared|source>] [--version <x.y.z>]

Configures LLVM environment variables for the current shell:
  LLVM_SYS_211_PREFIX
  OMNI_LLVM_LINK_MODE
  PATH (prepends <prefix>/bin)
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --flavor)
      FLAVOR="${2:-}"
      shift 2
      ;;
    --version)
      VERSION="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      return 0 2>/dev/null || exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      return 1 2>/dev/null || exit 1
      ;;
  esac
done

case "$FLAVOR" in
  auto|prebuilt|source-static|source-shared|source) ;;
  *)
    echo "Invalid --flavor '$FLAVOR'" >&2
    usage >&2
    return 1 2>/dev/null || exit 1
    ;;
esac

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/.." && pwd)"
prebuilt_prefix="$repo_root/.deps/llvm/$VERSION"
source_static_prefix="$repo_root/.deps/llvm-src/$VERSION-static"
source_shared_prefix="$repo_root/.deps/llvm-src/$VERSION-shared"
source_legacy_prefix="$repo_root/.deps/llvm-src/$VERSION"

test_llvm_prefix() {
  local prefix="$1"
  [[ -x "$prefix/bin/llvm-config" || -f "$prefix/bin/llvm-config" ]]
}

test_shared_llvm_c() {
  local prefix="$1"
  local lib_dir="$prefix/lib"
  local bin_dir="$prefix/bin"
  local has_import_or_stub=0
  local has_runtime=0

  [[ -f "$lib_dir/libLLVM-C.so" || -f "$lib_dir/libLLVM-C.dylib" ]] && has_import_or_stub=1
  [[ -f "$bin_dir/libLLVM-C.so" || -f "$bin_dir/libLLVM-C.dylib" || -f "$lib_dir/libLLVM-C.so" || -f "$lib_dir/libLLVM-C.dylib" ]] && has_runtime=1

  [[ $has_import_or_stub -eq 1 && $has_runtime -eq 1 ]]
}

prefix=""
if [[ "$FLAVOR" == "prebuilt" ]]; then
  test_llvm_prefix "$prebuilt_prefix" && prefix="$prebuilt_prefix"
elif [[ "$FLAVOR" == "source-static" ]]; then
  test_llvm_prefix "$source_static_prefix" && prefix="$source_static_prefix"
elif [[ "$FLAVOR" == "source-shared" ]]; then
  test_llvm_prefix "$source_shared_prefix" && prefix="$source_shared_prefix"
elif [[ "$FLAVOR" == "source" ]]; then
  if test_llvm_prefix "$source_static_prefix"; then
    prefix="$source_static_prefix"
  elif test_llvm_prefix "$source_shared_prefix"; then
    prefix="$source_shared_prefix"
  elif test_llvm_prefix "$source_legacy_prefix"; then
    prefix="$source_legacy_prefix"
  fi
else
  if test_llvm_prefix "$source_static_prefix"; then
    prefix="$source_static_prefix"
  elif test_llvm_prefix "$prebuilt_prefix"; then
    prefix="$prebuilt_prefix"
  elif test_llvm_prefix "$source_shared_prefix"; then
    prefix="$source_shared_prefix"
  elif test_llvm_prefix "$source_legacy_prefix"; then
    prefix="$source_legacy_prefix"
  fi
fi

if [[ -z "$prefix" ]]; then
  echo "LLVM not found for flavor=$FLAVOR version=$VERSION." >&2
  echo "Run scripts/bootstrap-llvm.sh or scripts/bootstrap-llvm-source.sh first." >&2
  return 1 2>/dev/null || exit 1
fi

link_mode=""
if [[ "$FLAVOR" == "source-static" ]]; then
  link_mode="static"
elif [[ "$FLAVOR" == "source-shared" || "$FLAVOR" == "prebuilt" ]]; then
  link_mode="shared"
else
  if test_shared_llvm_c "$prefix"; then
    link_mode="shared"
  else
    link_mode="static"
  fi
fi

export LLVM_SYS_211_PREFIX="$prefix"
export OMNI_LLVM_LINK_MODE="$link_mode"
export PATH="$prefix/bin:$PATH"

echo "LLVM env configured for this shell: $prefix"
echo "OMNI_LLVM_LINK_MODE = $link_mode"

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  echo "Note: run with 'source scripts/use-llvm-env.sh ...' to persist env vars in your current shell." >&2
fi
