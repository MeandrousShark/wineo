#!/bin/bash
# Turn the packaging/debian/ in an unpacked source tree into the debian/ for
# one Ubuntu series. One recipe serves every series; this fills in the only
# two things that differ:
#   debian/control    "@RUST@" in Build-Depends becomes the series' Rust
#                     package suffix: "-1.91" on jammy and noble, whose own
#                     rustc is older than the floor, and nothing elsewhere.
#   debian/changelog  the newest entry's UNRELEASED becomes the series and its
#                     version gains "~SERIES1", so an upgrade to the next
#                     Ubuntu release always installs a newer package.
#
# Usage: scripts/prepare-debian-tree.sh [--recipe DIR] TREE SERIES
#
# TREE is the unpacked wineo-VERSION source, which already holds
# packaging/debian. SERIES is jammy, noble, resolute, or stonking; a new
# series is added below once its own Rust is checked against the floor.
# --recipe takes the recipe from DIR instead of TREE/packaging/debian. A
# revision bump after the tag needs it: the bumped recipe can't go into TREE's
# packaging/debian, since dpkg-source refuses a tree that differs from the
# orig tarball outside debian/.
# Exits 2 on bad usage and 1 when debian/ already exists.
set -euo pipefail

usage() {
    echo "usage: ${0##*/} [--recipe DIR] TREE jammy|noble|resolute|stonking" >&2
    exit 2
}

recipe=
if [[ ${1:-} == --recipe ]]; then
    [[ $# -ge 2 && -n $2 ]] || usage
    recipe=$2
    shift 2
fi
[[ $# -eq 2 ]] || usage
tree=$1
series=$2

case $series in
    jammy | noble) rust=-1.91 ;;
    resolute | stonking) rust= ;;
    *) usage ;;
esac

src=${recipe:-$tree/packaging/debian}
if [[ ! -f $src/control || ! -f $src/changelog ]]; then
    echo "${0##*/}: $src is not a Debian recipe" >&2
    exit 1
fi
if [[ -e $tree/debian ]]; then
    echo "${0##*/}: $tree/debian already exists" >&2
    exit 1
fi

cp -a "$src" "$tree/debian"
# Comment lines mention the marker too, so only real fields are changed.
sed -i "/^#/! s/@RUST@/$rust/g" "$tree/debian/control"
sed -i -E "1 s/^(wineo \([^)]+)\) UNRELEASED;/\1~${series}1) $series;/" \
    "$tree/debian/changelog"

if ! head -n 1 "$tree/debian/changelog" | grep -q "~${series}1) $series;"; then
    echo "${0##*/}: the newest changelog entry is not an UNRELEASED wineo entry" >&2
    rm -rf "$tree/debian"
    exit 1
fi
