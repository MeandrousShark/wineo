#!/bin/bash
# Checks the metadata of the Gentoo overlay under packaging/gentoo: the live
# ebuild and the release ebuild that gentoo-ebuild.sh derives from it (with
# --skip-crates, so there is no network). check-gentoo-build.sh is what proves
# they pass pkgcheck, build, and run. wineo-gui loads its libraries with
# dlopen, so nothing else notices when these lists drift from the README's
# runtime list. Needs no Gentoo tools, but rust-floor.sh needs the crates
# fetched (`cargo fetch --locked`).
#
# Usage: scripts/test-gentoo.sh
# Exits non-zero if any case behaves wrongly.
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(cd "$script_dir/.." && pwd)
overlay=$root/packaging/gentoo
package_dir=$overlay/app-emulation/wineo

tmp=$(mktemp -d)
trap 'rm -rf "${tmp:?}"' EXIT

failures=0
cases=0

pass() { cases=$((cases + 1)); }
fail() {
    cases=$((cases + 1))
    failures=$((failures + 1))
    echo "FAIL: $*" >&2
}

# The Gentoo atom for each entry in the README's Ubuntu list. libX11 builds
# libX11-xcb, dev-libs/wayland builds libwayland-egl, and libxkbcommon[X] is
# libxkbcommon-x11.
gentoo_atom() {
    case $1 in
        libx11-6 | libx11-xcb1) echo x11-libs/libX11 ;;
        libxcb1) echo x11-libs/libxcb ;;
        libxkbcommon0) echo x11-libs/libxkbcommon ;;
        libxkbcommon-x11-0) echo 'x11-libs/libxkbcommon[X]' ;;
        libxcursor1) echo x11-libs/libXcursor ;;
        libxi6) echo x11-libs/libXi ;;
        libwayland-client0 | libwayland-egl1) echo dev-libs/wayland ;;
        libegl1) echo media-libs/libglvnd ;;
        libvulkan1) echo media-libs/vulkan-loader ;;
        *) return 1 ;;
    esac
}

# variable FILE NAME: the quoted value of NAME, one word per line. Only
# handles NAME="...", on one line or several.
variable() {
    awk -v name="$2" '
        !started && index($0, name "=\"") == 1 {
            started = 1
            sub(/^[^"]*"/, "")
        }
        started {
            closed = ($0 ~ /"$/)
            sub(/"$/, "")
            print
            if (closed) exit
        }
    ' "$1" | tr -s '[:space:]' '\n' | sed '/^$/d'
}

has() { grep -qxF -- "$2" <<<"$1"; }

mapfile -t ubuntu < <("$script_dir/readme-runtime-packages.sh")
if ((${#ubuntu[@]} > 0)); then pass; else fail "the README's runtime list is empty"; fi

version=$(awk -F '"' '
    /^\[workspace\.package\]$/ { in_workspace = 1; next }
    /^\[/ { in_workspace = 0 }
    in_workspace && /^version = / { print $2; exit }
' "$root/Cargo.toml")
mkdir "$tmp/release"
if "$script_dir/gentoo-ebuild.sh" --skip-crates "$version" "$tmp/release" >/dev/null; then
    pass
else
    fail "gentoo-ebuild.sh could not write the release ebuild"
fi

floor=$("$script_dir/rust-floor.sh" | cut -d ' ' -f 1 | cut -d . -f 1-2) || floor=
[[ -n $floor ]] || fail "rust-floor.sh printed nothing"

for ebuild in "$package_dir/wineo-9999.ebuild" "$tmp/release/wineo-$version.ebuild"; do
    name=${ebuild##*/}
    if [[ ! -f $ebuild ]]; then
        fail "$name does not exist"
        continue
    fi

    rdepend=$(variable "$ebuild" RDEPEND)
    # Atoms without their USE flags, for the entries that need none.
    # shellcheck disable=SC2001
    plain=$(sed 's/\[.*\]$//' <<<"$rdepend")
    for package in "${ubuntu[@]}"; do
        if ! atom=$(gentoo_atom "$package"); then
            fail "no Gentoo atom for $package; add it to gentoo_atom"
        elif has "$rdepend" "$atom" || { [[ $atom != *'['* ]] && has "$plain" "$atom"; }; then
            pass
        else
            fail "$name does not depend on $atom (for $package)"
        fi
    done

    for atom in app-arch/xz-utils x11-themes/hicolor-icon-theme; do
        if has "$rdepend" "$atom"; then
            pass
        else
            fail "$name does not depend on $atom"
        fi
    done

    for line in '# Packaging written with AI assistance and reviewed by the maintainer.' \
        '# SPDX-License-Identifier: 0BSD' 'EAPI=8'; do
        if grep -qxF "$line" "$ebuild"; then
            pass
        else
            fail "$name is missing: $line"
        fi
    done

    min_ver=$(sed -n 's/^RUST_MIN_VER="\(.*\)"$/\1/p' "$ebuild")
    if [[ -n $floor && $(cut -d . -f 1-2 <<<"$min_ver") == "$floor" ]]; then
        pass
    else
        fail "$name has RUST_MIN_VER \"$min_ver\", but the Rust floor is \"$floor\""
    fi

    if grep -qxF 'LICENSE="GPL-3+"' "$ebuild"; then
        pass
    else
        fail "$name does not license wineo as GPL-3+"
    fi

    # stage-install.sh is the one place that knows the install layout.
    if grep -q 'scripts/stage-install.sh' "$ebuild"; then
        pass
    else
        fail "$name does not install through scripts/stage-install.sh"
    fi
done

live=$package_dir/wineo-9999.ebuild
release=$tmp/release/wineo-$version.ebuild
if [[ -f $live ]] && grep -qx 'inherit cargo desktop git-r3 xdg' "$live" &&
    grep -qx 'EGIT_REPO_URI="https://github.com/MeandrousShark/wineo.git"' "$live" &&
    grep -q 'cargo_live_src_unpack' "$live"; then
    pass
else
    fail "wineo-9999.ebuild is not a git-r3 live ebuild with cargo_live_src_unpack"
fi

# shellcheck disable=SC2016
if [[ -f $release ]] && grep -qx 'inherit cargo desktop xdg' "$release" &&
    grep -qF 'wineo/archive/refs/tags/v${PV}.tar.gz' "$release" &&
    grep -qx 'KEYWORDS="~amd64"' "$release" &&
    ! grep -q 'git-r3\|EGIT_' "$release"; then
    pass
else
    fail "the release ebuild does not fetch GitHub's tag tarball"
fi

for file in profiles/repo_name metadata/layout.conf; do
    if [[ -s $overlay/$file ]]; then pass; else fail "$file is missing"; fi
done
if [[ $(cat "$overlay/profiles/repo_name") == wineo ]]; then
    pass
else
    fail "profiles/repo_name is not wineo"
fi
if grep -qx 'masters = gentoo' "$overlay/metadata/layout.conf"; then
    pass
else
    fail "layout.conf does not name gentoo as master"
fi
if grep -q '<email>mphannon@pm.me</email>' "$package_dir/metadata.xml"; then
    pass
else
    fail "metadata.xml does not name the maintainer"
fi
if grep -qF 'Permission to use, copy, modify, and/or distribute' "$overlay/LICENSE"; then
    pass
else
    fail "the overlay has no 0BSD LICENSE"
fi

echo "$((cases - failures)) of $cases cases behaved correctly"
[[ $failures -eq 0 ]]
