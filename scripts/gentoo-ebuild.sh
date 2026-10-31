#!/bin/bash
# Write the release ebuild wineo-VERSION.ebuild. It is the live ebuild
# (wineo-9999.ebuild) with the git-r3 parts swapped for GitHub's tag tarball,
# so the dependency lists live in one place, and then pycargoebuild fills in
# CRATES and the crates' licenses so Portage fetches and checksums every
# crate. The result still needs `pkgdev manifest` (or `ebuild ... manifest`)
# in the overlay before it is committed, which adds the DIST lines.
#
# Usage: scripts/gentoo-ebuild.sh [--skip-crates] VERSION [OUTPUT_DIR]
#
# VERSION must be the workspace version, since the crate list comes from this
# checkout's Cargo.lock. OUTPUT_DIR defaults to the overlay's package
# directory (packaging/gentoo/games-util/wineo). --skip-crates leaves
# CRATES and the crate licenses empty and needs no network; the tests use it.
#
# pycargoebuild comes from PATH if it is there (Gentoo, or the gentoo/stage3
# container with app-portage/pycargoebuild emerged). Otherwise it is
# installed into a throwaway venv from PyPI, pinned below. It needs wget or
# aria2 to fetch the crates. Either way it reads
# Gentoo's license-mapping.conf, downloaded from the gentoo/gentoo mirror
# into the same throwaway directory. Crate tarballs go to
# WINEO_GENTOO_DISTDIR if set (they run to hundreds of MB, so point it at a
# real disk), otherwise to a temporary directory.
#
# Exits 2 on bad usage and 1 when the version doesn't match the workspace,
# the live ebuild no longer has a part this script rewrites, or a tool fails.
set -euo pipefail

pycargoebuild_version=0.17.2
mapping_url=https://raw.githubusercontent.com/gentoo/gentoo/refs/heads/master/metadata/license-mapping.conf

usage() {
    echo "usage: ${0##*/} [--skip-crates] VERSION [OUTPUT_DIR]" >&2
    exit 2
}

skip_crates=0
if [[ ${1:-} == --skip-crates ]]; then
    skip_crates=1
    shift
fi
[[ $# -ge 1 && $# -le 2 ]] || usage
version=$1
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || usage

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
package_dir=$root/packaging/gentoo/games-util/wineo
outdir=${2:-$package_dir}
[[ -d $outdir ]] || usage
live=$package_dir/wineo-9999.ebuild
out=$outdir/wineo-$version.ebuild

workspace_version=$(awk -F '"' '
    /^\[workspace\.package\]$/ { in_workspace = 1; next }
    /^\[/ { in_workspace = 0 }
    in_workspace && /^version = / { print $2; exit }
' "$root/Cargo.toml")
if [[ $workspace_version != "$version" ]]; then
    echo "${0##*/}: the workspace is at $workspace_version, not $version" >&2
    exit 1
fi

tmp=$(mktemp -d)
made_distdir=
cleanup() {
    rm -rf "${tmp:?}"
    if [[ -n $made_distdir ]]; then rm -rf "${made_distdir:?}"; fi
}
trap cleanup EXIT

# The release ebuild differs from the live one in a few places, each checked
# so that editing the live ebuild can't silently break this.
# shellcheck disable=SC2016
awk '
    /^inherit cargo git-r3 xdg$/ {
        print "CRATES=\"\"\n"
        print "inherit cargo xdg"
        found["inherit"] = 1
        next
    }
    /^HOMEPAGE=/ {
        print
        print "SRC_URI=\""
        print "\thttps://github.com/MeandrousShark/wineo/archive/refs/tags/v${PV}.tar.gz -> ${P}.gh.tar.gz"
        print "\t${CARGO_CRATE_URIS}"
        print "\""
        found["homepage"] = 1
        next
    }
    /^EGIT_/ { found["egit"] = 1; next }
    /^LICENSE=/ {
        print
        print "# Dependent crate licenses"
        print "LICENSE+=\"\""
        found["license"] = 1
        next
    }
    /^SLOT=/ {
        print
        print "KEYWORDS=\"~amd64\""
        found["slot"] = 1
        next
    }
    /^src_unpack\(\) \{$/ { skipping = 1; found["unpack"] = 1; next }
    skipping && /^\}$/ { skipping = 0; drop_blank = 1; next }
    skipping { next }
    drop_blank && /^$/ { drop_blank = 0; next }
    { drop_blank = 0; print }
    END {
        split("inherit homepage egit license slot unpack", parts, " ")
        for (i in parts) {
            if (!found[parts[i]]) {
                print "live ebuild has no " parts[i] " line to rewrite" > "/dev/stderr"
                bad = 1
            }
        }
        exit bad
    }
' "$live" >"$tmp/wineo.ebuild" || {
    echo "${0##*/}: could not derive the release ebuild from $live" >&2
    exit 1
}

if [[ $skip_crates -eq 0 ]]; then
    if command -v pycargoebuild >/dev/null; then
        pycargoebuild=pycargoebuild
    else
        echo "${0##*/}: installing pycargoebuild $pycargoebuild_version into a temporary venv" >&2
        python3 -m venv "$tmp/venv"
        "$tmp/venv/bin/pip" install --quiet "pycargoebuild==$pycargoebuild_version"
        pycargoebuild=$tmp/venv/bin/pycargoebuild
    fi

    curl -fsSL -o "$tmp/license-mapping.conf" "$mapping_url"

    distdir=${WINEO_GENTOO_DISTDIR:-}
    if [[ -z $distdir ]]; then
        distdir=$(mktemp -d)
        made_distdir=$distdir
    fi
    mkdir -p "$distdir"

    # It refuses a workspace root, so it gets the two members that are
    # built; they share the root's Cargo.lock. Without -M it would call
    # pkgdev, which only exists on Gentoo. Run from the temp dir so nothing
    # is read from or written to the current one.
    (
        cd "$tmp"
        "$pycargoebuild" --no-config -C -M -i wineo.ebuild \
            -d "$distdir" -l "$tmp/license-mapping.conf" \
            "$root/crates/wineo-cli" "$root/crates/wineo-gui"
    )
fi

cp "$tmp/wineo.ebuild" "$out"
echo "wrote $out"
