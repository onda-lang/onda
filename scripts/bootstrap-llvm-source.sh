#!/usr/bin/env bash
set -euo pipefail

VERSION="21.1.2"
LINKAGE="Static"

usage() {
  cat <<'EOF'
Usage: scripts/bootstrap-llvm-source.sh [options]

Options:
  --version <x.y.z>                 LLVM release version (default: 21.1.2)
  --linkage <Static|Shared>         Install flavor (default: Static)
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      VERSION="${2:-}"
      shift 2
      ;;
    --linkage)
      LINKAGE="${2:-}"
      shift 2
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

case "$LINKAGE" in
  Static|Shared) ;;
  *)
    echo "Invalid --linkage '$LINKAGE' (expected Static or Shared)" >&2
    exit 1
    ;;
esac

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/.." && pwd)"
bootstrap_root="$repo_root/deps/llvm-bootstrap"
deps_root="$repo_root/.deps"
linkage_lower="$(printf '%s' "$LINKAGE" | tr '[:upper:]' '[:lower:]')"
source_dir="$deps_root/src/llvm-project-$VERSION"
build_dir="$deps_root/build-llvm-$VERSION-$linkage_lower"
install_dir="$deps_root/llvm-src/$VERSION-$linkage_lower"

if [[ ! -f "$bootstrap_root/build_local.sh" ]]; then
  echo "deps/llvm-bootstrap is missing. Run 'git submodule update --init --recursive' first." >&2
  exit 1
fi

bash "$bootstrap_root/build_local.sh" \
  --llvm-ref "llvmorg-$VERSION" \
  --source-dir "$source_dir" \
  --build-dir "$build_dir" \
  --install-dir "$install_dir" \
  --linkage "$LINKAGE"

echo "LLVM source build installed at: $install_dir"
