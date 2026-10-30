#!/bin/bash
# Write DIR/SHA256SUMS covering the release assets directly in DIR: every
# regular file named *.tar.gz, *.tar.xz, *.deb, or *.rpm. Directories (such as
# the staging tree pack-tarball.sh leaves beside its tarball), symlinks, and
# SHA256SUMS itself are left out. Names are bare, so
# `cd DIR && sha256sum -c SHA256SUMS` verifies the assets in place.
#
# Usage: scripts/write-checksums.sh DIR
#
# Exits 2 on bad usage and 1 on any other error, including an existing
# SHA256SUMS or a DIR with no assets. A failed run removes the file it made.
set -euo pipefail

usage() {
    echo "usage: ${0##*/} DIR" >&2
    exit 2
}

die() {
    echo "${0##*/}: $*" >&2
    exit 1
}

[[ $# -eq 1 ]] || usage
dir=$1
[[ -d $dir ]] || die "$dir is not a directory"
cd "$dir"

# -L too: the redirect below would write through a dangling symlink.
if [[ -e SHA256SUMS || -L SHA256SUMS ]]; then
    die "$PWD/SHA256SUMS already exists"
fi

# C order keeps the file the same on every machine.
export LC_ALL=C
shopt -s nullglob
assets=()
for file in *.tar.gz *.tar.xz *.deb *.rpm; do
    if [[ -f $file && ! -L $file ]]; then
        # sha256sum escapes these in its output, which sha256sum -c would
        # then have to undo; no asset is ever named like this.
        [[ $file != *[\\$'\n']* ]] || die "unsupported character in the name '$file'"
        assets+=("$file")
    fi
done
((${#assets[@]} > 0)) || die "no release assets in $PWD"

trap 'status=$?; if ((status != 0)); then rm -f SHA256SUMS; fi' EXIT
sha256sum -- "${assets[@]}" >SHA256SUMS
