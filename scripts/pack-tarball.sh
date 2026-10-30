#!/bin/bash
# Build the release tarball: wineo-VERSION-x86_64-linux.tar.gz, holding the
# whole install tree under usr/ (from scripts/stage-install.sh) plus
# install.sh, README.md, and LICENSE at the top level.
#
# Usage: scripts/pack-tarball.sh VERSION BINDIR OUTDIR
#
# BINDIR holds the built `wineo` and `wineo-gui`. The staging directory
# OUTDIR/wineo-VERSION-x86_64-linux and the tarball beside it are written to
# OUTDIR. BINDIR and OUTDIR are relative to the caller's working directory.
#
# Exits 2 on bad usage and 1 on any other error, including an existing
# staging directory. A failed run removes the staging directory it made.
set -euo pipefail

usage() {
    echo "usage: ${0##*/} VERSION BINDIR OUTDIR" >&2
    exit 2
}

[[ $# -eq 3 ]] || usage
version=$1
# The version becomes part of a path, so keep it to plain characters.
[[ $version =~ ^[0-9A-Za-z][0-9A-Za-z._+-]*$ ]] || usage

# Absolute before any cd or install can change what they mean.
bindir=$2
outdir=$3
[[ $bindir == /* ]] || bindir=$PWD/$bindir
[[ $outdir == /* ]] || outdir=$PWD/$outdir

root=$(cd "$(dirname "$0")/.." && pwd)
name=wineo-$version-x86_64-linux
staging=$outdir/$name

if [[ -e $staging ]]; then
    echo "${0##*/}: $staging already exists" >&2
    exit 1
fi

mkdir -p "$outdir"
mkdir "$staging"
# Half a staging tree would block the next run and could be packed by hand.
trap 'status=$?; if ((status != 0)); then rm -rf "${staging:?}"; fi' EXIT

"$root/scripts/stage-install.sh" "$staging" "$bindir"
install -m755 "$root/packaging/tarball/install.sh" "$staging/install.sh"
install -m644 "$root/README.md" "$staging/README.md"
install -m644 "$root/LICENSE" "$staging/LICENSE"

tar -C "$outdir" --sort=name --owner=0 --group=0 --numeric-owner \
    -czf "$outdir/$name.tar.gz" "$name"
