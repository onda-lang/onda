#!/usr/bin/env bash

set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "usage: $0 <dylib> <consumer>" >&2
    exit 2
fi

library="${1##*/}"
expected="@rpath/$library"
dylib_id="$(
    otool -D "$1" |
        awk 'NR == 2 { print $1 }'
)"
if [[ "$dylib_id" != "$expected" ]]; then
    echo "Expected '$1' identity '$expected'; found '$dylib_id'" >&2
    exit 1
fi

consumer_dependency="$(
    otool -L "$2" |
        awk -v library="$library" '{ name = $1; sub(/^.*\//, "", name); if (name == library) print $1 }'
)"
if [[ "$consumer_dependency" != "$expected" ]]; then
    echo "Expected '$2' dependency '$expected'; found '$consumer_dependency'" >&2
    exit 1
fi
