#!/bin/bash
# Download the pinned nfpm release into DIR and check its SHA-256, leaving
# the program at DIR/nfpm. CI uses it in the release build and in the
# package tests, so the version, URL, and sum are written down only here.
#
# Usage: scripts/fetch-nfpm.sh DIR
#
# Exits 2 on bad usage and 1 when the download, the checksum, or the
# unpacking fails. A failed run leaves no nfpm behind.
set -euo pipefail

version=2.48.0
asset=nfpm_${version}_Linux_x86_64.tar.gz
url=https://github.com/goreleaser/nfpm/releases/download/v$version/$asset
sha256=6d7f9a28b2f397934403950fcbba83f598b08d02edbe79a0815b3b4897341750

[[ $# -eq 1 ]] || {
    echo "usage: ${0##*/} DIR" >&2
    exit 2
}
dir=$1

mkdir -p "$dir"
tmp=$(mktemp -d)
trap 'rm -rf "${tmp:?}"' EXIT

curl --proto '=https' --tlsv1.2 -sSfL -o "$tmp/$asset" "$url"
(cd "$tmp" && echo "$sha256  $asset" | sha256sum -c -)
tar -xzf "$tmp/$asset" -C "$tmp" nfpm
install -m755 "$tmp/nfpm" "$dir/nfpm"
