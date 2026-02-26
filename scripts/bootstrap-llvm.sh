#!/usr/bin/env bash
set -euo pipefail

VERSION="21.1.2"
ASSET=""

usage() {
  cat <<'EOF'
Usage: scripts/bootstrap-llvm.sh [--version <x.y.z>] [--asset <release-asset-name>]

Downloads and installs a prebuilt LLVM release into:
  .deps/llvm/<version>

If --asset is omitted, the script chooses a default asset by OS/arch.
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
      ASSET="clang+llvm-$VERSION-x86_64-linux-gnu-ubuntu-22.04.tar.xz"
      ;;
    Linux/aarch64|Linux/arm64)
      ASSET="clang+llvm-$VERSION-aarch64-linux-gnu.tar.xz"
      ;;
    Darwin/x86_64)
      ASSET="clang+llvm-$VERSION-x86_64-apple-darwin.tar.xz"
      ;;
    Darwin/arm64|Darwin/aarch64)
      ASSET="clang+llvm-$VERSION-arm64-apple-darwin.tar.xz"
      ;;
    *)
      echo "No default prebuilt LLVM asset mapping for platform '$os/$arch'." >&2
      echo "Pass --asset explicitly, or use scripts/bootstrap-llvm-source.sh." >&2
      exit 1
      ;;
  esac
fi

url="https://github.com/llvm/llvm-project/releases/download/llvmorg-$VERSION/$ASSET"
archive="$dist_root/$ASSET"
temp_extract_root="$dist_root/extract-$VERSION"

echo "Downloading $url"
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

extracted_dir="$(find "$temp_extract_root" -mindepth 1 -maxdepth 1 -type d | head -n 1 || true)"
if [[ -z "$extracted_dir" ]]; then
  echo "Extraction failed: no directory found in archive" >&2
  exit 1
fi

rm -rf "$version_root"
mkdir -p "$version_root"
shopt -s dotglob nullglob
mv "$extracted_dir"/* "$version_root"/
shopt -u dotglob nullglob

if [[ ! -x "$version_root/bin/llvm-config" && ! -f "$version_root/bin/llvm-config" ]]; then
  echo "llvm-config not found after extraction. Verify --asset and package contents." >&2
  exit 1
fi

echo "LLVM bootstrapped to $version_root"
echo "Set env in your shell if needed:"
echo "  export LLVM_SYS_211_PREFIX=\"$version_root\""
