#!/bin/bash
# Build the vendored source tarball: wineo-VERSION-vendored.tar.xz. It holds
# the tracked files at a git ref under wineo-VERSION/, plus the output of
# `cargo vendor --locked` in wineo-VERSION/vendor/ and the source replacement
# config that command prints in wineo-VERSION/.cargo/config.toml, so
# `cargo build --frozen` in the unpacked tree needs no network. Distro
# packagers build from this.
#
# Usage: scripts/pack-vendored.sh VERSION REF OUTDIR
#
# REF is any commit-ish in the repository this script lives in: a tag, a
# commit, or a branch name like main. OUTDIR is relative to the caller's
# working directory. The work happens in a temporary directory, never in the
# caller's checkout. Needs git, cargo, tar, and xz, plus network access for
# cargo vendor.
#
# The tar is sorted and owned by root, with every time set to REF's commit
# time. The xz output is not promised to be byte-identical across xz
# versions.
#
# Exits 2 on bad usage and 1 on any other error, including an existing
# output file. A failed run removes the tarball it made.
set -euo pipefail

usage() {
    echo "usage: ${0##*/} VERSION REF OUTDIR" >&2
    exit 2
}

die() {
    echo "${0##*/}: $*" >&2
    exit 1
}

[[ $# -eq 3 ]] || usage
version=$1
ref=$2
# The version becomes part of a path, so keep it to plain characters. A ref
# starting with a dash would be read as an option by git.
[[ $version =~ ^[0-9A-Za-z][0-9A-Za-z._+-]*$ ]] || usage
[[ -n $ref && $ref != -* ]] || usage

# Absolute before any cd can change what it means.
outdir=$3
[[ $outdir == /* ]] || outdir=$PWD/$outdir

root=$(cd "$(dirname "$0")/.." && pwd)
name=wineo-$version
out=$outdir/$name-vendored.tar.xz

for tool in git cargo tar xz; do
    command -v "$tool" >/dev/null || die "$tool not found"
done

# -L too: the redirect below would write through a dangling symlink.
if [[ -e $out || -L $out ]]; then
    die "$out already exists"
fi

commit=$(git -C "$root" rev-parse --verify --quiet "$ref^{commit}") || die "no commit named '$ref' in $root"
stamp=$(git -C "$root" log -1 --format=%ct "$commit")

tmp=$(mktemp -d)
made=()
# A half-written tarball must not be picked up by the upload step.
cleanup() {
    local status=$?
    rm -rf "${tmp:?}"
    if ((status != 0)) && ((${#made[@]} > 0)); then
        rm -f "${made[@]}"
    fi
}
trap cleanup EXIT

# git archive takes only tracked files, so build output and local notes in
# the checkout can't leak into the tarball.
git -C "$root" archive --format=tar --prefix="$name/" "$commit" | tar -x -C "$tmp"
tree=$tmp/$name

# Overwriting either would silently change what the build uses.
for existing in .cargo vendor; do
    if [[ -e $tree/$existing || -L $tree/$existing ]]; then
        die "$ref already tracks $existing, which this script would overwrite"
    fi
done

# cargo vendor prints the config on stdout and its progress on stderr, so the
# config is captured exactly as printed. No -q: that silences the config too.
mkdir "$tree/.cargo"
(cd "$tree" && cargo vendor --locked vendor >.cargo/config.toml) || die "cargo vendor failed"
# With nothing to vendor cargo prints no config, and the tarball would
# promise an offline build it can't deliver.
[[ -s $tree/.cargo/config.toml ]] || die "cargo vendor printed no source replacement config"

mkdir -p "$outdir"
made+=("$out")
tar -C "$tmp" --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$stamp" \
    -cf - "$name" | xz -9 -T0 >"$out"
