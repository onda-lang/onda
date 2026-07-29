#!/usr/bin/env bash

set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "usage: $0 <module>" >&2
    exit 2
fi

case "$(uname -s)" in
    Linux)
        expected=onda_cmake_sdk_smoke
        nm_args=(-D --defined-only --format=posix)
        ;;
    Darwin)
        expected=_onda_cmake_sdk_smoke
        nm_args=(-gjU)
        ;;
    *)
        echo "unsupported platform: $(uname -s)" >&2
        exit 2
        ;;
esac

nm "${nm_args[@]}" "$1" | awk -v expected="$expected" '
    $1 == expected {
        found_smoke = 1
        next
    }
    {
        print "Unexpected exported symbol: " $1 > "/dev/stderr"
        found_unexpected = 1
    }
    END {
        if (!found_smoke)
            print "Missing exported symbol: " expected > "/dev/stderr"
        exit !found_smoke || found_unexpected
    }
'
