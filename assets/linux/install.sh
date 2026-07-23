#!/bin/sh

set -eu

fail() {
    printf 'install.sh: %s\n' "$1" >&2
    exit 1
}

if [ "$#" -ne 0 ]; then
    fail "this installer does not accept arguments"
fi

[ "$(uname -s)" = "Linux" ] || fail "this installer is only supported on Linux"
[ -n "${HOME:-}" ] || fail "HOME is not set"
case "$HOME" in
    /*) ;;
    *) fail "HOME must be an absolute path" ;;
esac

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
source_binary="$script_dir/bin/onda"
source_desktop="$script_dir/share/applications/onda-run.desktop"
source_icon="$script_dir/share/icons/hicolor/512x512/apps/onda-run.png"

[ -x "$source_binary" ] || fail "missing executable: $source_binary"
[ -f "$source_desktop" ] || fail "missing desktop entry: $source_desktop"
[ -f "$source_icon" ] || fail "missing application icon: $source_icon"

install_prefix="$HOME/.local"
data_home="${XDG_DATA_HOME:-$install_prefix/share}"
case "$data_home" in
    /*) ;;
    *) fail "XDG_DATA_HOME must be an absolute path" ;;
esac

target_binary="$install_prefix/bin/onda"
target_desktop="$data_home/applications/onda-run.desktop"
target_icon="$data_home/icons/hicolor/512x512/apps/onda-run.png"

case "$target_binary" in
    *'
'*) fail "the installation path must not contain a newline" ;;
esac

desktop_exec_escape() {
    sed \
        -e 's/\\/\\\\\\\\/g' \
        -e 's/"/\\\\"/g' \
        -e 's/`/\\\\`/g' \
        -e 's/\$/\\\\$/g'
}

desktop_string_escape() {
    sed -e 's/\\/\\\\/g'
}

escaped_exec=$(printf '%s' "$target_binary" | desktop_exec_escape)
escaped_try_exec=$(printf '%s' "$target_binary" | desktop_string_escape)

mkdir -p \
    "$(dirname -- "$target_binary")" \
    "$(dirname -- "$target_desktop")" \
    "$(dirname -- "$target_icon")"

install -m 755 "$source_binary" "$target_binary"
install -m 644 "$source_icon" "$target_icon"

temporary_desktop=$(mktemp "$(dirname -- "$target_desktop")/.onda-run.desktop.XXXXXX")
cleanup() {
    rm -f "$temporary_desktop"
}
trap cleanup EXIT HUP INT TERM

while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
        Exec=*) printf 'Exec="%s" run %%f\n' "$escaped_exec" ;;
        TryExec=*) printf 'TryExec=%s\n' "$escaped_try_exec" ;;
        *) printf '%s\n' "$line" ;;
    esac
done < "$source_desktop" > "$temporary_desktop"

install -m 644 "$temporary_desktop" "$target_desktop"

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$(dirname -- "$target_desktop")" >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$data_home/icons/hicolor" >/dev/null 2>&1 || true
fi

printf 'Installed Onda:\n'
printf '  executable: %s\n' "$target_binary"
printf '  desktop entry: %s\n' "$target_desktop"
printf '  icon: %s\n' "$target_icon"
