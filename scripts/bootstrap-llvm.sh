#!/usr/bin/env bash
set -euo pipefail

VERSION="21.1.2"
ASSET=""

usage() {
  cat <<'EOF'
Usage: scripts/bootstrap-llvm.sh [--version <x.y.z>] [--asset <release-asset-name>]

Local builds default to building LLVM from source via deps/llvm-bootstrap.
CI builds may download a prebuilt LLVM package instead.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      VERSION="${2:-}"
      shift 2
      ;;
    --asset)
      ASSET="${2:-}"
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

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/.." && pwd)"

if [[ -z "${CI:-}" ]]; then
  echo "CI not detected; building LLVM from source via deps/llvm-bootstrap."
  exec "$script_dir/bootstrap-llvm-source.sh" --version "$VERSION" --linkage Static
fi

deps_root="$repo_root/.deps"
llvm_root="$deps_root/llvm"
version_root="$llvm_root/$VERSION"
dist_root="$deps_root/dist"

mkdir -p "$deps_root" "$llvm_root" "$dist_root"

if [[ -x "$version_root/bin/llvm-config" || -f "$version_root/bin/llvm-config" ]]; then
  echo "LLVM $VERSION already bootstrapped at $version_root"
  exit 0
fi

if [[ -z "$ASSET" ]]; then
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os/$arch" in
    Linux/x86_64|Linux/amd64)
      ASSET="llvm-$VERSION-linux-x64-static.tar.xz"
      ;;
    Darwin/arm64|Darwin/aarch64)
      ASSET="llvm-$VERSION-macos-arm64-static.tar.xz"
      ;;
    *)
      echo "No default llvm-bootstrap asset mapping for platform '$os/$arch'." >&2
      echo "Pass --asset explicitly." >&2
      exit 1
      ;;
  esac
fi

url="https://github.com/vitreo12/llvm-bootstrap/releases/download/llvm-$VERSION/$ASSET"
archive="$dist_root/$ASSET"
temp_extract_root="$dist_root/extract-$VERSION"

echo "CI detected; downloading $url"
if command -v curl >/dev/null 2>&1; then
  curl -fL "$url" -o "$archive"
elif command -v wget >/dev/null 2>&1; then
  wget -O "$archive" "$url"
else
  echo "Neither curl nor wget is available." >&2
  exit 1
fi

rm -rf "$temp_extract_root"
mkdir -p "$temp_extract_root"

echo "Extracting archive..."
tar -xf "$archive" -C "$temp_extract_root"

if [[ -z "$(find "$temp_extract_root" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
  echo "Extraction failed: archive produced no files" >&2
  exit 1
fi

content_root="$temp_extract_root"
dir_count="$(find "$temp_extract_root" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d '[:space:]')"
entry_count="$(find "$temp_extract_root" -mindepth 1 -maxdepth 1 | wc -l | tr -d '[:space:]')"
if [[ "$entry_count" == "1" && "$dir_count" == "1" ]]; then
  content_root="$(find "$temp_extract_root" -mindepth 1 -maxdepth 1 -type d | head -n 1)"
fi

rm -rf "$version_root"
mkdir -p "$version_root"
shopt -s dotglob nullglob
mv "$content_root"/* "$version_root"/
shopt -u dotglob nullglob

if [[ ! -x "$version_root/bin/llvm-config" && ! -f "$version_root/bin/llvm-config" ]]; then
  echo "llvm-config not found after extraction. Verify --asset and package contents." >&2
  exit 1
fi

echo "LLVM bootstrapped to $version_root"
