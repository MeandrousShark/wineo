#!/bin/bash
# Download the pinned cargo-deny release into DIR and check its SHA-256,
# leaving the program at DIR/cargo-deny. CI uses it for the advisory and
# license checks, so the version, URL, and sum are written down only here.
#
# Usage: scripts/fetch-cargo-deny.sh DIR
#
# Exits 2 on bad usage and 1 when the download, the checksum, or the
# unpacking fails. A failed run leaves no cargo-deny behind.
set -euo pipefail

version=0.20.2
name=cargo-deny-$version-x86_64-unknown-linux-musl
asset=$name.tar.gz
url=https://github.com/EmbarkStudios/cargo-deny/releases/download/$version/$asset
sha256=9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f

[[ $# -eq 1 ]] || {
    echo "usage: ${0##*/} DIR" >&2
    exit 2
}
dir=$1

mkdir -p "$dir"
rm -f "$dir/cargo-deny"
tmp=$(mktemp -d)
trap 'rm -rf "${tmp:?}"' EXIT

curl --proto '=https' --tlsv1.2 -sSfL -o "$tmp/$asset" "$url"
(cd "$tmp" && echo "$sha256  $asset" | sha256sum -c -)
tar -xzf "$tmp/$asset" -C "$tmp" "$name/cargo-deny"
install -m755 "$tmp/$name/cargo-deny" "$dir/cargo-deny"
