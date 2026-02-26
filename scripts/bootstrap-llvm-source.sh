#!/usr/bin/env bash
set -euo pipefail

VERSION="21.1.2"
CONFIG="Release"
PYTHON_EXECUTABLE=""
LINKAGE="Static"
GENERATOR="Auto"

usage() {
  cat <<'EOF'
Usage: scripts/bootstrap-llvm-source.sh [options]

Options:
  --version <x.y.z>                 LLVM release version (default: 21.1.2)
  --config <Debug|Release|...>      CMake build config (default: Release)
  --python-executable <path>        Optional Python executable for CMake
  --linkage <Static|Shared>         Install flavor (default: Static)
  --generator <Auto|Ninja|UnixMakefiles>
                                    CMake generator selection (default: Auto)
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      VERSION="${2:-}"
      shift 2
      ;;
    --config)
      CONFIG="${2:-}"
      shift 2
      ;;
    --python-executable)
      PYTHON_EXECUTABLE="${2:-}"
      shift 2
      ;;
    --linkage)
      LINKAGE="${2:-}"
      shift 2
      ;;
    --generator)
      GENERATOR="${2:-}"
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

case "$GENERATOR" in
  Auto|Ninja|UnixMakefiles) ;;
  *)
    echo "Invalid --generator '$GENERATOR' (expected Auto, Ninja, or UnixMakefiles)" >&2
    exit 1
    ;;
esac

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/.." && pwd)"
deps_root="$repo_root/.deps"
src_root="$deps_root/src"
linkage_lower="$(echo "$LINKAGE" | tr '[:upper:]' '[:lower:]')"
build_root="$deps_root/build-llvm-$VERSION-$linkage_lower"
install_root="$deps_root/llvm-src/$VERSION-$linkage_lower"
dist_root="$deps_root/dist"

archive="$dist_root/llvm-project-llvmorg-$VERSION.tar.gz"
url="https://github.com/llvm/llvm-project/archive/refs/tags/llvmorg-$VERSION.tar.gz"

mkdir -p "$deps_root" "$src_root" "$dist_root"

if [[ ! -f "$archive" ]]; then
  echo "Downloading LLVM source archive: $url"
  if command -v curl >/dev/null 2>&1; then
    curl -fL "$url" -o "$archive"
  elif command -v wget >/dev/null 2>&1; then
    wget -O "$archive" "$url"
  else
    echo "Neither curl nor wget is available." >&2
    exit 1
  fi
fi

extract_root="$src_root/llvm-project-$VERSION"
if [[ ! -d "$extract_root" ]]; then
  echo "Extracting LLVM sources..."
  tar -xf "$archive" -C "$src_root"
  extracted="$(find "$src_root" -mindepth 1 -maxdepth 1 -type d -name 'llvm-project-llvmorg-*' | sort | tail -n 1 || true)"
  if [[ -z "$extracted" ]]; then
    echo "Failed to locate extracted llvm-project source directory" >&2
    exit 1
  fi
  mv "$extracted" "$extract_root"
fi

llvm_src="$extract_root/llvm"
if [[ ! -d "$llvm_src" ]]; then
  echo "LLVM source tree not found at $llvm_src" >&2
  exit 1
fi

have_ninja=0
command -v ninja >/dev/null 2>&1 && have_ninja=1

resolved_generator=""
if [[ "$GENERATOR" == "Ninja" ]]; then
  resolved_generator="Ninja"
elif [[ "$GENERATOR" == "UnixMakefiles" ]]; then
  resolved_generator="Unix Makefiles"
else
  if [[ $have_ninja -eq 1 ]]; then
    resolved_generator="Ninja"
  else
    resolved_generator="Unix Makefiles"
  fi
fi

if [[ "$resolved_generator" == "Ninja" && $have_ninja -ne 1 ]]; then
  echo "Generator Ninja selected but 'ninja' was not found in PATH." >&2
  exit 1
fi

echo "Configuring LLVM with generator: $resolved_generator"
echo "Requested LLVM linkage: $LINKAGE"

if [[ -f "$build_root/CMakeCache.txt" ]]; then
  cache_generator="$(sed -n 's/^CMAKE_GENERATOR:INTERNAL=//p' "$build_root/CMakeCache.txt" | head -n 1 || true)"
  if [[ -n "$cache_generator" && "$cache_generator" != "$resolved_generator" ]]; then
    echo "Recreating build dir due to generator change ($cache_generator -> $resolved_generator)"
    rm -rf "$build_root"
  fi
fi

cmake_args=(
  -S "$llvm_src"
  -B "$build_root"
  -G "$resolved_generator"
  "-DCMAKE_INSTALL_PREFIX=$install_root"
  "-DCMAKE_BUILD_TYPE=$CONFIG"
  -DLLVM_ENABLE_PROJECTS=
  -DLLVM_TARGETS_TO_BUILD=X86
  -DLLVM_INCLUDE_TESTS=OFF
  -DLLVM_INCLUDE_BENCHMARKS=OFF
  -DLLVM_INCLUDE_EXAMPLES=OFF
  -DLLVM_INCLUDE_DOCS=OFF
)

if [[ "$LINKAGE" == "Static" ]]; then
  cmake_args+=(
    -DBUILD_SHARED_LIBS=OFF
    -DLLVM_BUILD_LLVM_DYLIB=OFF
    -DLLVM_BUILD_LLVM_C_DYLIB=OFF
    -DLLVM_LINK_LLVM_DYLIB=OFF
  )
else
  cmake_args+=(
    -DBUILD_SHARED_LIBS=OFF
    -DLLVM_BUILD_LLVM_DYLIB=ON
    -DLLVM_BUILD_LLVM_C_DYLIB=ON
    -DLLVM_LINK_LLVM_DYLIB=ON
  )
fi

if [[ -n "$PYTHON_EXECUTABLE" ]]; then
  cmake_args+=("-DPython3_EXECUTABLE=$PYTHON_EXECUTABLE")
fi

cmake "${cmake_args[@]}"
cmake --build "$build_root" --target install --config "$CONFIG"

if [[ ! -x "$install_root/bin/llvm-config" && ! -f "$install_root/bin/llvm-config" ]]; then
  echo "llvm-config not found after source build install" >&2
  exit 1
fi

if [[ "$LINKAGE" == "Static" && ! -f "$install_root/lib/libLLVMCore.a" ]]; then
  echo "Static LLVM core library not found at $install_root/lib/libLLVMCore.a" >&2
  exit 1
fi

echo "LLVM source build installed at: $install_root"
echo "Use this in your shell:"
echo "  export LLVM_SYS_211_PREFIX=\"$install_root\""
