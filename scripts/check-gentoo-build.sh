#!/bin/bash
# Build one Gentoo ebuild in a fresh gentoo/stage3 container, scan the overlay
# with pkgcheck, then install the binary package in a second fresh container
# for the same checks the deb, rpm, and AUR packages get.
#
# The overlay is tested from a scratch copy under WORKDIR:
#   release  derives wineo-VERSION.ebuild with gentoo-ebuild.sh (which fetches
#            every crate), using the workspace's version, and builds from
#            `git archive HEAD` placed in DISTDIR as the tag tarball would be.
#   live     builds wineo-9999.ebuild, which clones GitHub's main.
#
# Usage: scripts/check-gentoo-build.sh release|live WORKDIR
#
# WORKDIR must not exist yet, and should be on a real disk, since a build
# outgrows tmpfs. It ends up holding the Portage tree, distfiles, binary
# packages, and the build directory; the files in it belong to your user
# (rootless root), except what Portage's own users write, so remove it with
# `podman unshare rm -rf WORKDIR`. WINEO_GENTOO_CPUS caps the containers' CPUs
# (default 8). The containers run at low priority.
#
# A rootless container can't give Portage its sandboxes, so make.conf turns
# off sandbox, usersandbox, ipc-sandbox, pid-sandbox, network-sandbox,
# mount-sandbox, userpriv, and userfetch (everything runs as the container's
# root, which is your user). It also sets PORTAGE_COMPRESS to nothing, because
# check-installed.sh looks for plain or gzipped files and Portage's default
# would turn the man pages and licenses into .bz2. TAR_OPTIONS keeps tar from
# restoring the crates' owners, whose ids a rootless container can't map.
# The container scripts are single-quoted so the host leaves them alone.
# shellcheck disable=SC2016
set -euo pipefail

usage() {
    echo "usage: ${0##*/} release|live WORKDIR" >&2
    exit 2
}

[[ $# -eq 2 ]] || usage
mode=$1
workdir=$2
[[ $mode == release || $mode == live ]] || usage
image=docker.io/gentoo/stage3:latest
cpus=${WINEO_GENTOO_CPUS:-8}

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(cd "$script_dir/.." && pwd)
version=$(awk -F '"' '
    /^\[workspace\.package\]$/ { in_workspace = 1; next }
    /^\[/ { in_workspace = 0 }
    in_workspace && /^version = / { print $2; exit }
' "$root/Cargo.toml")
if [[ $mode == release ]]; then ebuild_version=$version; else ebuild_version=9999; fi

if [[ -e $workdir ]]; then
    echo "${0##*/}: $workdir already exists" >&2
    exit 1
fi
mkdir -p "$workdir"/{overlay,distfiles,portage-tmp,binpkgs,tree}
workdir=$(cd "$workdir" && pwd)

cp -r "$root/packaging/gentoo/." "$workdir/overlay/"
if [[ $mode == release ]]; then
    git -C "$root" archive --prefix="wineo-$version/" \
        -o "$workdir/distfiles/wineo-$version.gh.tar.gz" HEAD
fi

# Shared by both containers: Portage settings, the tree, and the overlay.
cat >"$workdir/setup.sh" <<'EOF'
set -euo pipefail
cat >>/etc/portage/make.conf <<MAKECONF
FEATURES="-sandbox -usersandbox -ipc-sandbox -pid-sandbox -network-sandbox -mount-sandbox -userpriv -userfetch buildpkg"
MAKEOPTS="-j$CPUS"
EMERGE_DEFAULT_OPTS="--getbinpkg --usepkg"
PORTAGE_COMPRESS=""
TAR_OPTIONS="--no-same-owner"
MAKECONF
cp /usr/share/portage/config/repos.conf /etc/portage/repos.conf/gentoo.conf 2>/dev/null ||
    { mkdir -p /etc/portage/repos.conf && cp /usr/share/portage/config/repos.conf /etc/portage/repos.conf/gentoo.conf; }
printf "[wineo]\nlocation = /overlay\nauto-sync = no\n" >/etc/portage/repos.conf/wineo.conf
# Release ebuilds are ~amd64 and the live one has no keywords.
printf "games-util/wineo ~amd64\n=games-util/wineo-9999 **\n" >/etc/portage/package.accept_keywords/wineo
# Xvfb without the Xorg server and the drivers it would pull in.
echo "x11-base/xorg-server xvfb -xorg -udev -elogind -suid" >/etc/portage/package.use/xvfb
if [[ ! -e /var/db/repos/gentoo/profiles ]]; then
    emerge-webrsync
fi
getuto
EOF

nice -n 15 podman run --rm --cpus "$cpus" \
    --tmpfs /tmp:rw,exec,nosuid,nodev,mode=1777 \
    -e CPUS="$cpus" -e MODE="$mode" -e VERSION="$ebuild_version" \
    -e WINEO_GENTOO_DISTDIR=/var/cache/distfiles \
    -v "$workdir/setup.sh:/setup.sh:ro" -v "$workdir/overlay:/overlay" \
    -v "$workdir/distfiles:/var/cache/distfiles" \
    -v "$workdir/portage-tmp:/var/tmp/portage" \
    -v "$workdir/binpkgs:/var/cache/binpkgs" \
    -v "$workdir/tree:/var/db/repos/gentoo" \
    -v "$root:/src:ro" "$image" bash -euo pipefail -c '
    bash /setup.sh
    emerge -q dev-util/pkgcheck
    package_dir=/overlay/games-util/wineo
    if [[ $MODE == release ]]; then
        # Run from outside the checkout, as a packager would.
        (cd /tmp && /src/scripts/gentoo-ebuild.sh "$VERSION" "$package_dir")
        (cd "$package_dir" && ebuild "wineo-$VERSION.ebuild" manifest)
    fi
    # Any finding fails the check.
    (cd /overlay && pkgcheck scan --repo wineo | tee /tmp/pkgcheck.txt)
    if [[ -s /tmp/pkgcheck.txt ]]; then
        echo "pkgcheck reported problems"
        exit 1
    fi
    mkdir /tmp/portage-logs
    PORTAGE_LOGDIR=/tmp/portage-logs emerge --verbose "=games-util/wineo-$VERSION"
    # wineo'"'"'s own log, since dependencies built here print notices of their
    # own. The crate count is the only one allowed: the plan chose it.
    log=$(echo /tmp/portage-logs/games-util:wineo-*)
    [[ -s $log ]]
    if grep "QA Notice" "$log" | grep -v "very large number of CRATES"; then
        echo "emerge printed QA notices"
        exit 1
    fi
'

# Alone first, so the package's RDEPEND is what pulls the libraries; Xvfb
# comes after so it can't hide a missing one from the version check.
nice -n 15 podman run --rm --cpus "$cpus" \
    --tmpfs /tmp:rw,exec,nosuid,nodev,mode=1777 \
    -e CPUS="$cpus" -e VERSION="$ebuild_version" \
    -v "$workdir/setup.sh:/setup.sh:ro" -v "$workdir/overlay:/overlay:ro" \
    -v "$workdir/portage-tmp:/var/tmp/portage" \
    -v "$workdir/binpkgs:/var/cache/binpkgs" \
    -v "$workdir/tree:/var/db/repos/gentoo" \
    -v "$root/scripts:/scripts:ro" "$image" bash -euo pipefail -c '
    bash /setup.sh
    # Not --usepkgonly: that also forbids the binhost builds of other
    # packages whose USE flags differ here, which breaks dependency solving.
    # Stop now, not after a rebuild, if the first container'"'"'s package is unusable.
    emerge --pretend --nodeps --usepkgonly "=games-util/wineo-$VERSION"
    FEATURES=-buildpkg emerge --verbose "=games-util/wineo-$VERSION"
    # With buildpkg off, only a package installed from a binary has this file.
    [[ -e /var/db/pkg/games-util/wineo-$VERSION/BINPKGMD5 ]]
    wineo --version
    # What wineo-gui dlopens, checked before Xvfb and its Mesa can supply any.
    for lib in libX11-xcb.so.1 libxcb.so.1 libxkbcommon.so.0 libxkbcommon-x11.so.0 \
        libXcursor.so.1 libXi.so.6 libwayland-client.so.0 libwayland-egl.so.1 \
        libEGL.so.1 libvulkan.so.1; do
        ldconfig -p | grep -qF "$lib (" || { echo "missing library: $lib"; exit 1; }
    done
    emerge --quiet x11-misc/xvfb-run x11-apps/xauth
    /scripts/check-gui-starts.sh /usr/bin/wineo-gui
    /scripts/check-installed.sh /usr "/usr/share/doc/wineo-$VERSION"
    # Portage records every installed file in CONTENTS.
    contents=/var/db/pkg/games-util/wineo-$VERSION/CONTENTS
    awk '"'"'$1 == "obj" || $1 == "sym" { print $2 }'"'"' "$contents" >/tmp/files.txt
    echo "$(wc -l </tmp/files.txt) installed files"
    [[ -s /tmp/files.txt ]]
    emerge --unmerge --quiet "=games-util/wineo-$VERSION"
    while read -r path; do
        if [[ -e $path || -L $path ]]; then
            echo "left behind: $path"
            exit 1
        fi
    done </tmp/files.txt
    echo "wineo-$VERSION: installed, ran, and removed cleanly"
'
