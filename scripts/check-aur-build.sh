#!/bin/bash
# Build one AUR recipe in a fresh Arch container, run namcap on it, and
# install the result in a second fresh container for the same checks the
# deb and rpm get. A stand-in for extra-x86_64-build, which needs root.
#
# The recipe is tested from a scratch copy using the workspace's version:
#   wineo      builds from `git archive HEAD`.
#   wineo-bin  repacks a dry run's tarball, with the sum from its SHA256SUMS.
#   wineo-git  clones main from GitHub and runs its check().
#
# Usage: scripts/check-aur-build.sh PACKAGE WORKDIR [ARTIFACTS]
#
# WORKDIR must not exist yet, and should be on a real disk, since a cargo
# build can outgrow tmpfs. ARTIFACTS is needed for wineo-bin and holds the
# dry run's tarball and SHA256SUMS. WINEO_AUR_CPUS caps the container's CPUs
# (default 8). The containers run at low priority.
# The container scripts are single-quoted so the host leaves them alone.
# shellcheck disable=SC2016
set -euo pipefail

usage() {
    echo "usage: ${0##*/} wineo|wineo-bin|wineo-git WORKDIR [ARTIFACTS]" >&2
    exit 2
}

[[ $# -eq 2 || $# -eq 3 ]] || usage
package=$1
workdir=$2
artifacts=${3:-}
image=docker.io/archlinux/archlinux:base-devel
cpus=${WINEO_AUR_CPUS:-8}

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(cd "$script_dir/.." && pwd)
version=$(awk -F '"' '
    /^\[workspace\.package\]$/ { in_workspace = 1; next }
    /^\[/ { in_workspace = 0 }
    in_workspace && /^version = / { print $2; exit }
' "$root/Cargo.toml")

if [[ -e $workdir ]]; then
    echo "${0##*/}: $workdir already exists" >&2
    exit 1
fi
mkdir -p "$workdir/build"
workdir=$(cd "$workdir" && pwd)
build=$workdir/build

case $package in
    wineo)
        cp "$root/packaging/aur/wineo/PKGBUILD" "$build/PKGBUILD"
        sed -i "s/^pkgver=.*/pkgver=$version/" "$build/PKGBUILD"
        git -C "$root" archive --prefix="wineo-$version/" \
            -o "$build/wineo-$version.tar.gz" HEAD
        ;;
    wineo-bin)
        [[ -n $artifacts ]] || usage
        tarball=wineo-$version-x86_64-linux.tar.gz
        sum=$(awk -v name="$tarball" '$2 == name { print $1 }' "$artifacts/SHA256SUMS")
        [[ -n $sum ]] || {
            echo "${0##*/}: $tarball is not in SHA256SUMS" >&2
            exit 1
        }
        cp "$root/packaging/aur/wineo-bin/PKGBUILD" "$build/PKGBUILD"
        sed -i -e "s/^pkgver=.*/pkgver=$version/" \
            -e "s/^sha256sums=.*/sha256sums=('$sum')/" "$build/PKGBUILD"
        cp "$artifacts/$tarball" "$build/"
        ;;
    wineo-git)
        cp "$root/PKGBUILD" "$build/PKGBUILD"
        ;;
    *) usage ;;
esac

uid=$(id -u)
gid=$(id -g)

# As root the container updates and installs; only the build drops to the
# host user, who owns the bind mount. /tmp is a real tmpfs as on an Arch
# system, because tests need birth times, which rootless podman's
# fuse-overlayfs doesn't report, and they run scripts from there.
nice -n 15 podman run --rm --userns=keep-id --user 0 --cpus "$cpus" \
    --tmpfs /tmp:rw,exec,nosuid,nodev,mode=1777 \
    -e BUILD_UID="$uid" -e BUILD_GID="$gid" \
    -v "$build:/build" -w /build "$image" bash -euo pipefail -c '
    as_builder() {
        setpriv --reuid="$BUILD_UID" --regid="$BUILD_GID" --clear-groups \
            env HOME=/build/home PKGDEST=/build/out "$@"
    }
    mkdir -p /build/home /build/out
    chown "$BUILD_UID:$BUILD_GID" /build/home /build/out
    pacman -Syu --noconfirm
    # Assigned first so a failing makepkg stops the script.
    srcinfo=$(as_builder makepkg --printsrcinfo)
    mapfile -t needs < <(sed -n -E \
        "s/^\s*(depends|makedepends|checkdepends) = //p" <<<"$srcinfo")
    pacman -S --noconfirm --needed "${needs[@]}"
    as_builder makepkg --noconfirm --nodeps
    pacman -S --noconfirm --needed namcap
    # Warnings stay in the log: namcap can'"'"'t see dlopen, so it calls every
    # graphics library unneeded. Errors fail the check.
    namcap PKGBUILD /build/out/*.pkg.tar.zst | tee /tmp/namcap.txt
    if grep -q " E: " /tmp/namcap.txt; then
        echo "namcap reported errors"
        exit 1
    fi
'

# Alone first, so the package's depends are what pulls the libraries; Xvfb
# comes after so it can't hide a missing one from the version check.
nice -n 15 podman run --rm --cpus "$cpus" \
    -e PACKAGE="$package" \
    -v "$build/out:/out:ro" -v "$root/scripts:/scripts:ro" "$image" \
    bash -euo pipefail -c '
    # Arch images skip man pages and docs, which would hide missing ones.
    sed -i "/^NoExtract/d" /etc/pacman.conf
    pacman -Syu --noconfirm
    pacman -U --noconfirm /out/*.pkg.tar.zst
    wineo --version
    pacman -S --noconfirm --needed xorg-server-xvfb xorg-xauth
    /scripts/check-gui-starts.sh /usr/bin/wineo-gui
    /scripts/check-installed.sh /usr "/usr/share/licenses/$PACKAGE"
    pacman -Qql "$PACKAGE" | while read -r path; do
        if [[ -f $path || -L $path ]]; then echo "$path"; fi
    done >/tmp/files.txt
    echo "$(wc -l </tmp/files.txt) installed files"
    [[ -s /tmp/files.txt ]]
    pacman -R --noconfirm "$PACKAGE"
    while read -r path; do
        if [[ -e $path || -L $path ]]; then
            echo "left behind: $path"
            exit 1
        fi
    done </tmp/files.txt
    echo "$PACKAGE: installed, ran, and removed cleanly"
'
