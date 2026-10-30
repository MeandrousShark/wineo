#!/bin/bash
# Checks the metadata of the three AUR recipes: the root PKGBUILD (wineo-git),
# packaging/aur/wineo, and packaging/aur/wineo-bin. check-aur-build.sh is what
# proves they build and run. wineo-gui loads its libraries with dlopen, so
# nothing else notices when these lists drift from the README's runtime list.
#
# Usage: scripts/test-aur.sh
# Exits non-zero if any case behaves wrongly.
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(cd "$script_dir/.." && pwd)
recipes=(PKGBUILD packaging/aur/wineo/PKGBUILD packaging/aur/wineo-bin/PKGBUILD)

failures=0
cases=0

pass() { cases=$((cases + 1)); }
fail() {
    cases=$((cases + 1))
    failures=$((failures + 1))
    echo "FAIL: $*" >&2
}

# The Arch package for each entry in the README's Ubuntu list.
arch_name() {
    case $1 in
        libx11-6 | libx11-xcb1) echo libx11 ;;
        libxcb1) echo libxcb ;;
        libxkbcommon0) echo libxkbcommon ;;
        libxkbcommon-x11-0) echo libxkbcommon-x11 ;;
        libxcursor1) echo libxcursor ;;
        libxi6) echo libxi ;;
        libwayland-client0 | libwayland-egl1) echo wayland ;;
        libegl1) echo libglvnd ;;
        libvulkan1) echo vulkan-icd-loader ;;
        ca-certificates) echo ca-certificates ;;
        *) return 1 ;;
    esac
}

# field RECIPE NAME: one line per entry of NAME in RECIPE.
field() (
    # shellcheck disable=SC1090
    source "$root/$1"
    declare -n value=$2
    printf '%s\n' "${value[@]}"
)

has() { grep -qxF -- "$2" <<<"$1"; }

mapfile -t ubuntu < <("$script_dir/readme-runtime-packages.sh")
if ((${#ubuntu[@]} > 0)); then pass; else fail "the README's runtime list is empty"; fi

desc=$(field PKGBUILD pkgdesc)
for recipe in "${recipes[@]}"; do
    depends=$(field "$recipe" depends)
    for package in "${ubuntu[@]}"; do
        if ! name=$(arch_name "$package"); then
            fail "no Arch name for $package; add it to arch_name"
        elif has "$depends" "$name"; then
            pass
        else
            fail "$recipe does not depend on $name (for $package)"
        fi
    done

    for name in glibc libgcc hicolor-icon-theme; do
        if has "$depends" "$name"; then
            pass
        else
            fail "$recipe does not depend on $name"
        fi
    done

    for line in '# Maintainer: Michael Hannon <mphannon@pm.me>' \
        '# Packaging written with AI assistance and reviewed by the maintainer.' \
        '# SPDX-License-Identifier: 0BSD'; do
        if grep -qxF "$line" "$root/$recipe"; then
            pass
        else
            fail "$recipe is missing: $line"
        fi
    done

    if [[ $(field "$recipe" pkgdesc) == "$desc" ]]; then
        pass
    else
        fail "$recipe has a different pkgdesc"
    fi
done

for recipe in PKGBUILD packaging/aur/wineo-bin/PKGBUILD; do
    if field "$recipe" provides | grep -qx 'wineo=[0-9].*'; then
        pass
    else
        fail "$recipe does not provide wineo=VERSION"
    fi
    if has "$(field "$recipe" conflicts)" wineo; then
        pass
    else
        fail "$recipe does not conflict with wineo"
    fi
done

for package in wineo wineo-bin; do
    recipe=packaging/aur/$package/PKGBUILD
    if [[ $(field "$recipe" pkgname) == "$package" ]]; then
        pass
    else
        fail "$recipe has the wrong pkgname"
    fi
    if grep -qF 'Permission to use, copy, modify, and/or distribute' \
        "$root/packaging/aur/$package/LICENSE"; then
        pass
    else
        fail "packaging/aur/$package has no 0BSD LICENSE"
    fi
    # The filter that keeps the root recipe's pkgver out of commits would
    # turn these real versions into 0.
    filter=$(git -C "$root" check-attr filter -- "$recipe")
    if [[ $filter == *': filter: unspecified' ]]; then
        pass
    else
        fail "$recipe is filtered: $filter"
    fi
done

if has "$(field packaging/aur/wineo-bin/PKGBUILD depends)" xz; then
    fail "wineo-bin depends on xz, but release binaries don't link it"
else
    pass
fi

echo "$((cases - failures)) of $cases cases behaved correctly"
[[ $failures -eq 0 ]]
