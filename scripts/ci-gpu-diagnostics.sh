#!/usr/bin/env bash
# Temporary probes for the Linux onda_ui SIGSEGV. Run tests without a debugger
# and inspect their original core dumps afterward to preserve crash timing.
set -euo pipefail

diagnostics=${ONDA_UI_DIAGNOSTICS_DIR:?Set ONDA_UI_DIAGNOSTICS_DIR}

case "${1:-}" in
  prepare)
    mkdir -p "$diagnostics/cores"
    {
      uname -a
      lscpu
      dpkg-query -W mesa-vulkan-drivers libvulkan1 libllvm21
      vulkaninfo --summary || printf '[DEBUG-gpu-ci] vulkaninfo exited %s\n' "$?"
    } 2>&1 | tee "$diagnostics/environment.log"
    sudo sysctl -w "kernel.core_pattern=$diagnostics/cores/core.%p"
    ;;
  run)
    shift
    if [[ ${1##*/} == onda_ui-* ]]; then
      realpath -- "$1" > "$diagnostics/executable-path.txt"
      printf '[DEBUG-gpu-ci] command:'
      printf ' %q' "$@"
      printf '\n'
      ulimit -c unlimited
    fi
    exec "$@"
    ;;
  collect)
    [[ -f "$diagnostics/executable-path.txt" ]] || exit 0
    executable=$(<"$diagnostics/executable-path.txt")
    shopt -s nullglob
    cores=("$diagnostics"/cores/core.*)
    ((${#cores[@]})) || exit 0
    cp -- "$executable" "$diagnostics/"
    export DEBUGINFOD_URLS=${DEBUGINFOD_URLS-https://debuginfod.ubuntu.com}
    export DEBUGINFOD_MAXTIME=${DEBUGINFOD_MAXTIME:-30}
    for core in "${cores[@]}"; do
      gdb --batch --quiet --nx \
        -iex 'set debuginfod enabled on' \
        -ex 'set pagination off' \
        -ex 'info threads' \
        -ex 'thread apply all bt 50' \
        -ex 'info sharedlibrary' \
        -ex 'info proc mappings' \
        -ex 'info registers' \
        "$executable" "$core" 2>&1 | tee "$diagnostics/${core##*/}.backtrace.txt"
    done
    ;;
  *)
    echo "Usage: $0 prepare | run <test executable> [args...] | collect" >&2
    exit 2
    ;;
esac
