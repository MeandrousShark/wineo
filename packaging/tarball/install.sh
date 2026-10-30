#!/bin/bash
# Install or remove the wineo files this tarball ships. Everything under the
# tarball's usr/ goes to the same path under PREFIX, so usr/bin/wineo becomes
# PREFIX/bin/wineo. No root is needed for a PREFIX you own.
#
# Usage: install.sh [--uninstall] [PREFIX]
#
# PREFIX defaults to $HOME/.local; a relative PREFIX is relative to the
# caller's working directory. The tree is found from this script's own
# location, so it works from any directory.
#
# Uninstalling removes exactly the files this tarball ships, then the
# share/doc/wineo and share/licenses/wineo directories if they are empty.
# Anything else under PREFIX is left alone.
#
# Exits 2 on bad usage before anything is written, and 1 on any other error.
set -euo pipefail

name=${0##*/}

usage() {
    echo "usage: $name [--uninstall] [PREFIX]" >&2
    exit 2
}

fail() {
    echo "$name: $*" >&2
    exit 1
}

# PREFIX without trailing slashes. It is empty for `/`, so "$base/bin" still
# reads right.
base_of() {
    local path=$1
    while [[ $path == */ ]]; do path=${path%/}; done
    printf '%s' "$path"
}

# The Exec value for the desktop entry, quoted by the Desktop Entry spec: a
# path with a reserved character is wrapped in double quotes, with ", `, $
# and \ escaped inside. A literal % is %% because % starts a field code.
# Backslashes are also doubled, because the file's string escaping runs
# before the quoting rule.
exec_value() {
    local path=${1//%/%%}
    local reserved=$' \t"\'\\<>~|&;$*?#()`'
    if [[ $path != *["$reserved"]* ]]; then
        printf '%s' "$path"
        return
    fi
    local bs=\\ out=\" char i
    for ((i = 0; i < ${#path}; i++)); do
        char=${path:i:1}
        case $char in
            \\) out+=$bs$bs$bs$bs ;;
            '"' | '`' | '$') out+=$bs$bs$char ;;
            *) out+=$char ;;
        esac
    done
    printf '%s"' "$out"
}

# Sourcing this file only defines the functions above, which the tests use.
[[ ${BASH_SOURCE[0]} == "$0" ]] || return 0

uninstall=0
while [[ $# -gt 0 ]]; do
    case $1 in
        --uninstall)
            uninstall=1
            shift
            ;;
        -*) usage ;;
        *) break ;;
    esac
done
[[ $# -le 1 ]] || usage

if [[ $# -eq 1 ]]; then
    prefix=$1
    [[ -n $prefix ]] || usage
elif [[ -n ${HOME:-} ]]; then
    prefix=$HOME/.local
else
    echo "$name: HOME is not set, so give a PREFIX" >&2
    exit 2
fi
# A control character, a newline above all, would make the desktop entry's
# Exec line invalid.
if [[ $prefix == *[[:cntrl:]]* ]]; then
    echo "$name: PREFIX may not contain control characters" >&2
    exit 2
fi
[[ $prefix == /* ]] || prefix=$PWD/$prefix

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
tree=$here/usr
[[ -d $tree ]] || fail "no usr/ directory next to $name"
mapfile -t files < <(cd "$tree" && find . -type f -printf '%P\n' | sort)

# Resolved physically when PREFIX exists, so `.` and `..` are taken the way
# the filesystem takes them, even after a symlink, and never end up in the
# desktop entry. A PREFIX that doesn't exist yet can't clash with anything.
if [[ ! -d $prefix ]] && ((!uninstall)); then
    mkdir -p "$prefix"
fi
if [[ -d $prefix ]]; then
    # The trailing x keeps $(...) from stripping newlines the real path ends
    # in, which would name a different directory.
    prefix=$(cd -P -- "$prefix" && pwd -P && echo x)
    prefix=${prefix%$'\n'x}
fi
# Checked again, since a symlink in PREFIX can lead to a name with one.
if [[ $prefix == *[[:cntrl:]]* ]]; then
    echo "$name: PREFIX leads to a directory whose name has control characters" >&2
    exit 2
fi
base=$(base_of "$prefix")
shown=${base:-/}

if ((uninstall)); then
    removed=0
    for rel in "${files[@]}"; do
        # Only files and symlinks can be ours; a directory here is not.
        if [[ -f $base/$rel || -L $base/$rel ]]; then
            rm -f -- "$base/$rel"
            removed=$((removed + 1))
        fi
    done
    rmdir "$base/share/doc/wineo" "$base/share/licenses/wineo" 2>/dev/null || true
    echo "Removed $removed files from $shown."
    exit 0
fi

# Nothing is written unless every target is free or a file or symlink to
# replace. install would put a file inside a directory sitting at the target
# (or behind a symlink to one) and still report success.
for rel in "${files[@]}"; do
    target=$base/$rel
    if [[ -d $target ]] || { [[ -e $target || -L $target ]] && [[ ! -f $target && ! -L $target ]]; }; then
        fail "$target exists and is not a file, so nothing was installed"
    fi
done

# Mode comes from the executable bit alone, so the umask the tarball was
# extracted under can't change what gets installed.
for rel in "${files[@]}"; do
    mode=644
    [[ -x $tree/$rel ]] && mode=755
    install -Dm"$mode" "$tree/$rel" "$base/$rel"
done

desktop=$base/share/applications/wineo-gui.desktop
if [[ -f $desktop ]]; then
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    line=$(exec_value "$base/bin/wineo-gui")
    while IFS= read -r text || [[ -n $text ]]; do
        if [[ $text == Exec=* ]]; then
            printf 'Exec=%s\n' "$line"
        else
            printf '%s\n' "$text"
        fi
    done <"$desktop" >"$tmp/desktop"
    install -m644 "$tmp/desktop" "$desktop"
fi

echo "Installed ${#files[@]} files to $shown."
case ":$PATH:" in
    *":$base/bin:"*) ;;
    *) echo "Add $shown/bin to your PATH to run wineo." ;;
esac
if [[ $base != /usr && $base != /usr/local ]]; then
    echo "For zsh completions, add $shown/share/zsh/site-functions to your fpath."
fi
