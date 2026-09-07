#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 2 ]]; then
    echo "usage: $0 <maximum-deployment-version> <Mach-O or archive>..." >&2
    exit 2
fi
maximum="$1"
shift
for binary in "$@"; do
    # otool also visits every object in a static archive. Check both the modern
    # LC_BUILD_VERSION and older LC_VERSION_MIN_MACOSX load commands.
    otool -l "$binary" | awk -v maximum="$maximum" -v binary="$binary" '
        function newer(value, floor, a, b, i) {
            split(value, a, ".")
            split(floor, b, ".")
            for (i = 1; i <= 3; ++i) {
                if (a[i] + 0 != b[i] + 0)
                    return a[i] + 0 > b[i] + 0
            }
            return 0
        }
        $1 == "cmd" { legacy = ($2 == "LC_VERSION_MIN_MACOSX") }
        $1 == "minos" || (legacy && $1 == "version") {
            ++count
            if (newer($2, maximum)) {
                print binary ": deployment target " $2 " exceeds " maximum > "/dev/stderr"
                failed = 1
            }
        }
        END {
            if (!count)
                print binary ": no macOS deployment metadata found" > "/dev/stderr"
            exit failed || !count
        }
    '
done
