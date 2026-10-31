#!/bin/bash
# Build the PPA, COPR, or OBS recipe for one target from the vendored source
# tarball, with the network off, then install the result into a fresh image
# of the same target and run the checks the Release packages get.
#
# Usage: scripts/check-repo-build.sh TARGET WORKDIR
#
# TARGET is one of:
#   jammy noble resolute stonking              Debian recipe (Ubuntu PPA)
#   fedora43 fedora44 fedora45                 rpm spec (COPR)
#   tumbleweed leap16.0 leap15.6               rpm spec (OBS)
#
# WORKDIR must not exist yet and should be on a real disk, since a cargo
# build outgrows tmpfs. It keeps the source, the build, and the packages
# (WORKDIR/out). Rootless podman runs everything at low priority.
#
# The recipes are tested at the workspace version from a vendored tarball of
# HEAD (scripts/pack-vendored.sh), which needs the network once. To reuse one
# across targets, set WINEO_REPO_TARBALL to a wineo-VERSION-vendored.tar.xz
# made from a commit that holds the recipes. WINEO_REPO_CPUS caps the CPUs
# (default 8). The committed recipes name the next release, so the version is
# rewritten in a scratch copy, as scripts/check-aur-build.sh does.
#
# Three podman runs, so that the build itself has no network:
#   1. the target's image installs the build dependencies, and is committed;
#   2. that image builds with --network=none (a connection is tried first, to
#      show the network is gone);
#   3. a fresh image of the target installs the package alone and runs
#      `wineo --version`, the GUI start under Xvfb, the installed-files
#      check, and a removal that must leave no file behind.
# The container scripts are single-quoted so the host leaves them alone.
# shellcheck disable=SC2016
set -euo pipefail

usage() {
    echo "usage: ${0##*/} TARGET WORKDIR" >&2
    echo "TARGET: jammy noble resolute stonking fedora43 fedora44 fedora45" >&2
    echo "        tumbleweed leap16.0 leap15.6" >&2
    exit 2
}

[[ $# -eq 2 ]] || usage
target=$1
workdir=$2
cpus=${WINEO_REPO_CPUS:-8}

case $target in
    jammy) kind=deb image=docker.io/library/ubuntu:22.04 ;;
    noble) kind=deb image=docker.io/library/ubuntu:24.04 ;;
    resolute) kind=deb image=docker.io/library/ubuntu:26.04 ;;
    stonking) kind=deb image=docker.io/library/ubuntu:26.10 ;;
    fedora43) kind=fedora image=registry.fedoraproject.org/fedora:43 ;;
    fedora44) kind=fedora image=registry.fedoraproject.org/fedora:44 ;;
    fedora45) kind=fedora image=registry.fedoraproject.org/fedora:45 ;;
    tumbleweed) kind=suse image=registry.opensuse.org/opensuse/tumbleweed:latest ;;
    leap16.0) kind=suse image=registry.opensuse.org/opensuse/leap:16.0 ;;
    leap15.6) kind=suse image=registry.opensuse.org/opensuse/leap:15.6 ;;
    *) usage ;;
esac

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(cd "$script_dir/.." && pwd)
version=$(awk -F '"' '
    /^\[workspace\.package\]$/ { in_workspace = 1; next }
    /^\[/ { in_workspace = 0 }
    in_workspace && /^version = / { print $2; exit }
' "$root/Cargo.toml")

die() {
    echo "${0##*/}: $*" >&2
    exit 1
}

[[ ! -e $workdir ]] || die "$workdir already exists"
mkdir -p "$workdir"
workdir=$(cd "$workdir" && pwd)
build=$workdir/build
out=$workdir/out
mkdir -p "$build" "$out"

tarball=${WINEO_REPO_TARBALL:-}
if [[ -z $tarball ]]; then
    "$script_dir/pack-vendored.sh" "$version" HEAD "$workdir/tarball"
    tarball=$workdir/tarball/wineo-$version-vendored.tar.xz
fi
[[ -f $tarball ]] || die "$tarball is not a file"
tarball=$(cd "$(dirname "$tarball")" && pwd)/$(basename "$tarball")
[[ $(basename "$tarball") == "wineo-$version-vendored.tar.xz" ]] ||
    die "the tarball must be named wineo-$version-vendored.tar.xz"

echo "== $target ($kind, $image), wineo $version, $cpus CPUs"
tar -xf "$tarball" -C "$build"
tree=$build/wineo-$version

builder=localhost/wineo-repo-build-$target-$$
deps=wineo-repo-deps-$target-$$
cleanup() {
    podman rm -f "$deps" >/dev/null 2>&1 || true
    podman rmi -f "$builder" >/dev/null 2>&1 || true
}
trap cleanup EXIT

case $kind in
    deb)
        # The version is rewritten for this test only; the series step is the
        # publish script's. The orig tarball is the vendored one under its
        # Debian name.
        # packaging/debian stays as the tarball has it, since dpkg-source
        # refuses a tree that differs from the orig tarball outside debian/.
        "$script_dir/prepare-debian-tree.sh" "$tree" "$target"
        sed -i -E "1 s/^wineo \([0-9][^-]*-/wineo ($version-/" \
            "$tree/debian/changelog"
        cp "$tarball" "$build/wineo_$version.orig.tar.xz"
        head -n 1 "$tree/debian/changelog"
        grep -E '^ (rustc|cargo)' "$tree/debian/control"

        nice -n 15 podman run --name "$deps" --cpus "$cpus" -v "$build:/work" \
            -w /work "$image" bash -euo pipefail -c '
            export DEBIAN_FRONTEND=noninteractive
            apt-get update
            apt-get install -y --no-install-recommends dpkg-dev build-essential
            apt-get build-dep -y --no-install-recommends "./wineo-'"$version"'"
        '
        podman commit -q "$deps" "$builder" >/dev/null
        podman rm "$deps" >/dev/null

        nice -n 15 podman run --rm --network=none --cpus "$cpus" \
            -v "$build:/work" -v "$out:/out" -w /work "$builder" \
            bash -euo pipefail -c '
            if timeout 3 bash -c "exec 3<>/dev/tcp/1.1.1.1/443" 2>/dev/null; then
                echo "the network is still on"
                exit 1
            fi
            echo "network is off: no route out"
            export DEB_BUILD_OPTIONS="parallel='"$cpus"'"
            # What Launchpad does: a source package first, then a binary
            # build from that source package alone.
            (cd wineo-'"$version"' && dpkg-buildpackage -us -uc -S)
            rm -rf unpack
            mkdir unpack
            (cd unpack && dpkg-source -x ../wineo_*.dsc &&
                cd wineo-'"$version"' && dpkg-buildpackage -us -uc -b)
            cp unpack/wineo_*_amd64.deb /out/
        '
        ;;
    fedora | suse)
        spec=$build/wineo.spec
        cp "$tree/packaging/rpm/wineo.spec" "$spec"
        sed -i -E "s/^Version:( +).*/Version:\1$version/" "$spec"
        sed -i -E '/^%changelog/,$ s/ - [0-9][^-]*-1$/ - '"$version"'-1/' "$spec"
        mkdir -p "$build/rpmbuild/SOURCES"
        cp "$tarball" "$build/rpmbuild/SOURCES/"

        if [[ $kind == fedora ]]; then
            nice -n 15 podman run --name "$deps" --cpus "$cpus" -v "$build:/work" \
                -w /work "$image" bash -euo pipefail -c '
                dnf -y install rpm-build redhat-rpm-config dnf-plugins-core
                dnf -y builddep wineo.spec
            '
        else
            nice -n 15 podman run --name "$deps" --cpus "$cpus" -v "$build:/work" \
                -w /work "$image" bash -euo pipefail -c '
                zypper --non-interactive --gpg-auto-import-keys refresh
                zypper --non-interactive install --no-recommends rpm-build tar xz
                mapfile -t needs < <(rpmspec -q --buildrequires wineo.spec)
                printf "build requires: %s\n" "${needs[@]}"
                zypper --non-interactive install --no-recommends "${needs[@]}"
                rustc --version
            '
        fi
        podman commit -q "$deps" "$builder" >/dev/null
        podman rm "$deps" >/dev/null

        nice -n 15 podman run --rm --network=none --cpus "$cpus" \
            -v "$build:/work" -v "$out:/out" -w /work "$builder" \
            bash -euo pipefail -c '
            if timeout 3 bash -c "exec 3<>/dev/tcp/1.1.1.1/443" 2>/dev/null; then
                echo "the network is still on"
                exit 1
            fi
            echo "network is off: no route out"
            rpmbuild -ba --define "_topdir /work/rpmbuild" \
                --define "_smp_mflags -j'"$cpus"'" wineo.spec
            cp rpmbuild/RPMS/x86_64/wineo-*.rpm /out/
            cp rpmbuild/SRPMS/wineo-*.src.rpm /out/
        '
        ;;
esac

ls -l "$out"

# A fresh image: only the package, so its dependencies are what pull the
# libraries in. Xvfb comes after the version check so it can't hide a missing
# library from it. The image's own settings that skip man pages, docs, and
# licenses are cleared, since they would hide missing files.
case $kind in
    deb)
        license_dir=/usr/share/doc/wineo
        install_cmd='
            rm -f /etc/dpkg/dpkg.cfg.d/excludes
            export DEBIAN_FRONTEND=noninteractive
            apt-get update
            apt-get install --no-install-recommends -y /out/wineo_*_amd64.deb
            dpkg -s wineo | grep -E "^(Version|Depends|Recommends|Suggests):"
        '
        xvfb_cmd='apt-get install --no-install-recommends -y xvfb xauth'
        files_cmd='dpkg -L wineo'
        remove_cmd='apt-get remove -y wineo'
        ;;
    fedora)
        license_dir=/usr/share/licenses/wineo
        install_cmd='
            sed -i "/^tsflags/d" /etc/dnf/dnf.conf
            dnf -y install --setopt=install_weak_deps=False /out/wineo-[0-9]*.x86_64.rpm
            rpm -qi wineo | grep -E "^(Version|Release|License) *:"
            rpm -qR wineo | sort
        '
        xvfb_cmd='dnf -y install --setopt=install_weak_deps=False xorg-x11-server-Xvfb xauth'
        files_cmd='rpm -ql wineo'
        remove_cmd='dnf -y remove wineo'
        ;;
    suse)
        license_dir=/usr/share/licenses/wineo
        install_cmd='
            # Tumbleweed keeps its defaults in /usr/etc, so use a config of our own.
            export ZYPP_CONF=/tmp/zypp.conf
            printf "[main]\nrpm.install.excludedocs = no\n" >"$ZYPP_CONF"
            zypper --non-interactive --gpg-auto-import-keys refresh
            zypper --non-interactive install --no-recommends --allow-unsigned-rpm /out/wineo-[0-9]*.x86_64.rpm
            rpm -qi wineo | grep -E "^(Version|Release|License) *:"
            rpm -qR wineo | sort
        '
        xvfb_cmd='zypper --non-interactive install --no-recommends xorg-x11-server-Xvfb xvfb-run xauth'
        files_cmd='rpm -ql wineo'
        remove_cmd='zypper --non-interactive remove wineo'
        ;;
esac

nice -n 15 podman run --rm --cpus "$cpus" -e VERSION="$version" \
    -e LICENSE_DIR="$license_dir" -e KIND="$kind" -e INSTALL_CMD="$install_cmd" \
    -e XVFB_CMD="$xvfb_cmd" -e FILES_CMD="$files_cmd" -e REMOVE_CMD="$remove_cmd" \
    -v "$out:/out:ro" -v "$root/scripts:/scripts:ro" "$image" \
    bash -euo pipefail -c '
    cat /etc/os-release | head -3
    eval "$INSTALL_CMD"
    echo "-- linked libraries"
    ldd /usr/bin/wineo /usr/bin/wineo-gui | tee /tmp/ldd.txt
    if grep -q "not found" /tmp/ldd.txt; then
        echo "a linked library is missing"
        exit 1
    fi
    version_line=$(wineo --version)
    echo "$version_line"
    # The whole line, so 1.1.01 cannot pass for 1.1.0.
    if [[ $version_line != "wineo $VERSION" ]]; then
        echo "expected: wineo $VERSION"
        exit 1
    fi
    eval "$XVFB_CMD"
    /scripts/check-gui-starts.sh /usr/bin/wineo-gui
    /scripts/check-installed.sh /usr "$LICENSE_DIR"
    if [[ $KIND == deb ]]; then test -f /usr/share/doc/wineo/copyright; fi
    eval "$FILES_CMD" | while read -r path; do
        if [[ -f $path || -L $path ]]; then echo "$path"; fi
    done >/tmp/files.txt
    echo "$(wc -l </tmp/files.txt) installed files"
    [[ -s /tmp/files.txt ]]
    eval "$FILES_CMD" | while read -r path; do
        if [[ -d $path ]]; then echo "$path"; fi
    done >/tmp/dirs.txt
    eval "$REMOVE_CMD"
    while read -r path; do
        if [[ -e $path || -L $path ]]; then
            echo "left behind: $path"
            exit 1
        fi
    done </tmp/files.txt
    # Shared directories (completions, hicolor) may stay if another package
    # owns them; they are reported, not failed.
    while read -r path; do
        if [[ -d $path ]] && [[ -z $(ls -A "$path") ]]; then
            echo "empty directory left: $path"
        fi
    done </tmp/dirs.txt
    echo "'"$target"': built offline, installed, ran, and removed cleanly"
'
