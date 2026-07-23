#!/bin/sh

set -eu

fail() {
    printf 'uninstall.sh: %s\n' "$1" >&2
    exit 1
}

if [ "$#" -ne 0 ]; then
    fail "this uninstaller does not accept arguments"
fi

[ "$(uname -s)" = "Linux" ] || fail "this uninstaller is only supported on Linux"
[ -n "${HOME:-}" ] || fail "HOME is not set"
case "$HOME" in
    /*) ;;
    *) fail "HOME must be an absolute path" ;;
esac

install_prefix="$HOME/.local"
data_home="${XDG_DATA_HOME:-$install_prefix/share}"
case "$data_home" in
    /*) ;;
    *) fail "XDG_DATA_HOME must be an absolute path" ;;
esac

target_binary="$install_prefix/bin/onda"
target_desktop="$data_home/applications/onda-run.desktop"
target_icon="$data_home/icons/hicolor/512x512/apps/onda-run.png"

rm -f "$target_binary" "$target_desktop" "$target_icon"

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$(dirname -- "$target_desktop")" >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$data_home/icons/hicolor" >/dev/null 2>&1 || true
fi

printf 'Removed Onda:\n'
printf '  executable: %s\n' "$target_binary"
printf '  desktop entry: %s\n' "$target_desktop"
printf '  icon: %s\n' "$target_icon"
