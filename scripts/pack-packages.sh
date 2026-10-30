#!/bin/bash
# Build the release packages with nfpm: wineo_VERSION_amd64.deb and
# wineo-VERSION-1.x86_64.rpm. Both hold the install tree from
# scripts/stage-install.sh; the deb keeps its licenses in
# /usr/share/doc/wineo beside a DEP-5 copyright file and a changelog, with
# gzipped man pages; the rpm keeps them in /usr/share/licenses/wineo.
#
# Usage: scripts/pack-packages.sh VERSION BINDIR OUTDIR
#
# BINDIR holds the built `wineo` and `wineo-gui`. Both packages are written
# to OUTDIR. BINDIR and OUTDIR are relative to the caller's working
# directory. nfpm is the program named by $NFPM, or else the one on PATH.
#
# The static metadata lives in packaging/nfpm.yaml. The runtime package
# lists come from the README through scripts/readme-runtime-packages.sh and
# are written into a copy of that file. The version and the rpm release
# reach nfpm as the environment variables NFPM_VERSION and NFPM_RELEASE,
# which it expands in the file.
#
# Exits 2 on bad usage and 1 on any other error, including an existing
# output file. A failed run removes the packages it made.
set -euo pipefail

usage() {
    echo "usage: ${0##*/} VERSION BINDIR OUTDIR" >&2
    exit 2
}

die() {
    echo "${0##*/}: $*" >&2
    exit 1
}

[[ $# -eq 3 ]] || usage
version=$1
# The version becomes part of a path and of package metadata, so keep it to
# what both deb and rpm accept.
[[ $version =~ ^[0-9][0-9A-Za-z.+~]*$ ]] || usage

# Absolute before any cd or install can change what they mean.
bindir=$2
outdir=$3
[[ $bindir == /* ]] || bindir=$PWD/$bindir
[[ $outdir == /* ]] || outdir=$PWD/$outdir

root=$(cd "$(dirname "$0")/.." && pwd)
deb=$outdir/wineo_${version}_amd64.deb
rpm=$outdir/wineo-$version-1.x86_64.rpm

nfpm=${NFPM:-nfpm}
command -v "$nfpm" >/dev/null || die "nfpm not found (set NFPM or put it on PATH)"

for out in "$deb" "$rpm"; do
    [[ ! -e $out ]] || die "$out already exists"
done

# Assigned first so a failing script stops here. Names go into YAML, so
# only plain package-name characters get through.
read_list() {
    local list
    list=$("$root/scripts/readme-runtime-packages.sh" --list "$1" "$root/README.md") ||
        die "no $1 runtime package list in README.md"
    if [[ -n $(grep -vE '^[0-9A-Za-z][0-9A-Za-z.+-]*$' <<<"$list" || true) ]]; then
        die "unexpected characters in the $1 runtime package list"
    fi
    tr '\n' ' ' <<<"$list"
}
ubuntu=$(read_list ubuntu)
fedora=$(read_list fedora)

tmp=$(mktemp -d)
made=()
# Half-built packages must not be picked up by the upload step.
cleanup() {
    local status=$?
    rm -rf "${tmp:?}"
    if ((status != 0)) && ((${#made[@]} > 0)); then
        rm -f "${made[@]}"
    fi
}
trap cleanup EXIT

config=$tmp/nfpm.yaml
awk -v ubuntu="$ubuntu" -v fedora="$fedora" '
    function emit(list,   n, names, i) {
        n = split(list, names, " ")
        for (i = 1; i <= n; i++) print "      - " names[i]
    }
    $0 == "      - \"@ubuntu-runtime-packages@\"" { emit(ubuntu); next }
    $0 == "      - \"@fedora-runtime-packages@\"" { emit(fedora); next }
    { print }
' "$root/packaging/nfpm.yaml" >"$config"
if grep -q '@[a-z-]*-runtime-packages@' "$config"; then
    die "packaging/nfpm.yaml has a package list placeholder this script doesn't fill"
fi

mkdir -p "$outdir"

# The deb gets its licenses in the doc directory, with a copyright file in
# the machine-readable format Debian expects there.
deb_stage=$tmp/deb
"$root/scripts/stage-install.sh" --license-dir /usr/share/doc/wineo "$deb_stage" "$bindir"
cat >"$deb_stage/usr/share/doc/wineo/copyright" <<'COPYRIGHT'
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: wineo
Source: https://github.com/MeandrousShark/wineo
Comment: The licenses of the bundled fonts are in THIRD_PARTY.md beside this
 file.

Files: *
Copyright: 2026 Michael Hannon
License: GPL-3.0-or-later
 This program is free software: you can redistribute it and/or modify it
 under the terms of the GNU General Public License as published by the Free
 Software Foundation, either version 3 of the License, or (at your option)
 any later version.
 .
 This program is distributed in the hope that it will be useful, but WITHOUT
 ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or
 FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for
 more details.
 .
 On Debian systems, the full text of the GNU General Public License version
 3 can be found in the file /usr/share/common-licenses/GPL-3.
COPYRIGHT
chmod 644 "$deb_stage/usr/share/doc/wineo/copyright"
# Debian policy wants compressed man pages and a changelog.gz. -n keeps
# names and times out of the gzip header so the same input packs the same.
find "$deb_stage/usr/share/man" -type f -name '*.1' -exec gzip -9n {} +
gzip -9n -c "$root/CHANGELOG.md" >"$deb_stage/usr/share/doc/wineo/changelog.gz"
chmod 644 "$deb_stage/usr/share/doc/wineo/changelog.gz"

rpm_stage=$tmp/rpm
"$root/scripts/stage-install.sh" "$rpm_stage" "$bindir"

# nfpm doesn't expand variables in a content's src, so it runs inside each
# staged tree and packaging/nfpm.yaml names the tree as ./usr. The deb has
# no release number, so its file and version stay X.Y.Z.
made+=("$deb")
(cd "$deb_stage" && NFPM_VERSION=$version NFPM_RELEASE='' \
    "$nfpm" package --config "$config" --packager deb --target "$deb")
made+=("$rpm")
(cd "$rpm_stage" && NFPM_VERSION=$version NFPM_RELEASE=1 \
    "$nfpm" package --config "$config" --packager rpm --target "$rpm")
