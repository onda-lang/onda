#!/usr/bin/env bash

set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "usage: $0 <shared-library> <consumer>" >&2
    exit 2
fi

expected=libonda.so
soname="$(
    readelf -d "$1" |
        sed -n 's/.*(SONAME).*\[\([^]]*\)\].*/\1/p'
)"
if [[ "$soname" != "$expected" ]]; then
    echo "Expected '$1' SONAME '$expected'; found '$soname'" >&2
    exit 1
fi

consumer_dependency="$(
    readelf -d "$2" |
        sed -n 's/.*(NEEDED).*\[\([^]]*\)\].*/\1/p' |
        awk '$0 ~ /libonda[.]so$/ { print }'
)"
if [[ "$consumer_dependency" != "$expected" ]]; then
    echo "Expected '$2' dependency '$expected'; found '$consumer_dependency'" >&2
    exit 1
fi
