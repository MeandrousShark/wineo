#!/bin/bash
# Lay out everything a wineo package installs under DESTDIR/usr: both
# binaries, the licenses, the desktop entry and icon, the example configs,
# shell completions, and man pages. Every package format calls this one
# script, so they all install the same tree.
#
# Usage: scripts/stage-install.sh [--license-dir DIR] DESTDIR [BINDIR]
#
# BINDIR holds the built `wineo` and `wineo-gui` (default target/release).
# DESTDIR and BINDIR are relative to the caller's working directory. Source
# files come from the repository this script lives in.
#
# DIR is an absolute path inside the package for LICENSE and THIRD_PARTY.md
# (default /usr/share/licenses/wineo). Distros disagree on where licenses
# go, so the caller picks.
#
# Exits 2 on bad usage and 1 when BINDIR lacks an executable binary, both
# before anything is written.
set -euo pipefail

usage() {
    echo "usage: ${0##*/} [--license-dir DIR] DESTDIR [BINDIR]" >&2
    exit 2
}

license_dir=/usr/share/licenses/wineo
while [[ $# -gt 0 ]]; do
    case $1 in
        --license-dir)
            [[ $# -ge 2 ]] || usage
            license_dir=$2
            shift 2
            ;;
        -*) usage ;;
        *) break ;;
    esac
done
[[ $# -ge 1 && $# -le 2 ]] || usage
[[ $license_dir == /* ]] || usage
# A ".." component would climb out of DESTDIR.
[[ $license_dir/ != */../* ]] || usage

# Absolute before any cd or install can change what they mean.
destdir=$1
bindir=${2:-target/release}
[[ $destdir == /* ]] || destdir=$PWD/$destdir
[[ $bindir == /* ]] || bindir=$PWD/$bindir

for bin in wineo wineo-gui; do
    if [[ ! -f $bindir/$bin || ! -x $bindir/$bin ]]; then
        echo "${0##*/}: $bindir/$bin is missing or not executable" >&2
        exit 1
    fi
done

root=$(cd "$(dirname "$0")/.." && pwd)
prefix=$destdir/usr

# Completions go through a temp dir so install sets their mode the same way
# as every other file.
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

install -Dm755 "$bindir/wineo" "$prefix/bin/wineo"
install -Dm755 "$bindir/wineo-gui" "$prefix/bin/wineo-gui"
install -Dm644 "$root/LICENSE" "$destdir$license_dir/LICENSE"
install -Dm644 "$root/crates/wineo-gui/THIRD_PARTY.md" \
    "$destdir$license_dir/THIRD_PARTY.md"
install -Dm644 "$root/crates/wineo-gui/assets/wineo-gui.desktop" \
    "$prefix/share/applications/wineo-gui.desktop"
install -Dm644 "$root/crates/wineo-gui/assets/wineo-gui.svg" \
    "$prefix/share/icons/hicolor/scalable/apps/wineo-gui.svg"
for example in environment native-environment proxied-environment; do
    install -Dm644 "$root/docs/$example.toml.example" \
        "$prefix/share/doc/wineo/$example.toml.example"
done

for shell in bash zsh fish; do
    "$bindir/wineo" completions "$shell" >"$tmp/$shell"
done
install -Dm644 "$tmp/bash" "$prefix/share/bash-completion/completions/wineo"
install -Dm644 "$tmp/zsh" "$prefix/share/zsh/site-functions/_wineo"
install -Dm644 "$tmp/fish" "$prefix/share/fish/vendor_completions.d/wineo.fish"

"$bindir/wineo" manpages "$prefix/share/man/man1"
