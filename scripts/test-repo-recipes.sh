#!/bin/bash
# Checks the repository recipes without building them: the Debian source
# package in packaging/debian and the spec in packaging/rpm. They are tested
# against each other, nfpm.yaml, the README's runtime lists, and deny.toml.
# check-repo-build.sh is what proves they build and run. wineo-gui loads its
# libraries with dlopen, so nothing else notices when these lists drift from
# the README.
#
# Usage: scripts/test-repo-recipes.sh
# Needs no dpkg-buildpackage or rpmbuild. Exits non-zero if any case behaves
# wrongly.
# The ${...} in single quotes are Debian substitution variables, not shell.
# shellcheck disable=SC2016
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(cd "$script_dir/.." && pwd)
control=$root/packaging/debian/control
rules=$root/packaging/debian/rules
changelog=$root/packaging/debian/changelog
copyright=$root/packaging/debian/copyright
spec=$root/packaging/rpm/wineo.spec
nfpm=$root/packaging/nfpm.yaml

failures=0
cases=0

pass() { cases=$((cases + 1)); }
fail() {
    cases=$((cases + 1))
    failures=$((failures + 1))
    echo "FAIL: $*" >&2
}
check() {
    local what=$1
    shift
    if "$@"; then pass; else fail "$what"; fi
}

has() { grep -qxF -- "$2" <<<"$1"; }
has_line() { grep -qxF -- "$2" "$1"; }

# The openSUSE package for each entry in the README's Fedora list. Their
# library packages carry the soname in the name, so these were looked up in
# Tumbleweed, Leap 16.0, and Leap 15.6 (see check-repo-build.sh).
suse_name() {
    case $1 in
        libX11) echo libX11-6 ;;
        libX11-xcb) echo libX11-xcb1 ;;
        libxcb) echo libxcb1 ;;
        libxkbcommon) echo libxkbcommon0 ;;
        libxkbcommon-x11) echo libxkbcommon-x11-0 ;;
        libXcursor) echo libXcursor1 ;;
        libXi) echo libXi6 ;;
        libwayland-client) echo libwayland-client0 ;;
        libwayland-egl) echo libwayland-egl1 ;;
        libglvnd-egl) echo libglvnd ;;
        vulkan-loader) echo libvulkan1 ;;
        ca-certificates) echo ca-certificates-mozilla ;;
        *) return 1 ;;
    esac
}

# control_field FIELD: the entries of a binary-package field, one per line,
# without comments or trailing commas.
control_field() {
    awk -v field="$1" '
        /^Package:/ { in_binary = 1 }
        /^#/ { next }
        in_binary && $0 ~ "^" field ":" { grab = 1; sub("^" field ":", ""); }
        grab && /^[A-Za-z]/ && $0 !~ "^" field ":" { grab = 0 }
        grab { print }
    ' "$control" | tr ',' '\n' | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' |
        grep -v '^$' || true
}

# build_depends: the same for the source stanza's Build-Depends.
build_depends() {
    awk '
        /^Package:/ { exit }
        /^#/ { next }
        /^Build-Depends:/ { grab = 1; sub("^Build-Depends:", ""); }
        grab && /^[A-Za-z]/ && !/^Build-Depends:/ { grab = 0 }
        grab { print }
    ' "$control" | tr ',' '\n' | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' |
        grep -v '^$' || true
}

# nfpm_list SECTION KEY: the entries of a deb or rpm list in nfpm.yaml.
nfpm_list() {
    awk -v section="$1" -v key="$2" '
        /^overrides:/ { in_over = 1; next }
        in_over && /^  [a-z]+:/ { current = $1; sub(":", "", current); next }
        in_over && current == section && $0 ~ "^    " key ":" { grab = 1; next }
        in_over && grab && /^      - / { sub("^      - ", ""); gsub(/"/, ""); print; next }
        in_over && grab { grab = 0 }
    ' "$nfpm"
}

# spec_requires BRANCH: the Requires lines of the spec's openSUSE ("suse") or
# other ("fedora") branch, as package names.
spec_requires() {
    awk -v branch="$1" '
        /^%if 0%\{\?suse_version\}/ { state = "suse"; next }
        /^%else/ && state == "suse" { state = "fedora"; next }
        /^%endif/ { state = ""; next }
        /^Requires:/ && (state == branch) { print $2 }
    ' "$spec"
}

spec_field() { sed -n -E "s/^$1:[[:space:]]+//p" "$spec" | head -n 1; }

mapfile -t ubuntu < <("$script_dir/readme-runtime-packages.sh")
mapfile -t fedora < <("$script_dir/readme-runtime-packages.sh" --list fedora)
check "the README's Ubuntu list is empty" test "${#ubuntu[@]}" -gt 0
check "the README's Fedora list is empty" test "${#fedora[@]}" -gt 0

# Disclosure and licence lines.
disclosure='# Packaging written with AI assistance and reviewed by the maintainer.'
for file in "$control" "$rules" "$spec"; do
    check "${file#"$root"/} is missing the disclosure line" has_line "$file" "$disclosure"
    check "${file#"$root"/} is missing the 0BSD SPDX line" \
        has_line "$file" '# SPDX-License-Identifier: 0BSD'
done
check "packaging/debian/rules is not executable" test -x "$rules"
check "packaging/debian/rules has no shebang" \
    test "$(head -n 1 "$rules")" = '#!/usr/bin/make -f'
check "debian/copyright has no 0BSD stanza for debian/*" \
    awk '/^Files: debian\/\*/ { f = 1; next } f && /^License: 0BSD$/ { ok = 1 } /^$/ { f = 0 } END { exit !ok }' "$copyright"
check "debian/copyright lacks the 0BSD text" \
    grep -qF 'Permission to use, copy, modify, and/or distribute' "$copyright"
check "debian/copyright does not license wineo as GPL-3.0-or-later" \
    awk '/^Files: \*$/ { f = 1; next } f && /^License: GPL-3.0-or-later$/ { ok = 1 } /^$/ { f = 0 } END { exit !ok }' "$copyright"
check "debian/source/format is not 3.0 (quilt)" \
    test "$(cat "$root/packaging/debian/source/format")" = '3.0 (quilt)'

# Names and versions.
maintainer='Michael Hannon <mphannon@pm.me>'
check "the control Maintainer is wrong" has_line "$control" "Maintainer: $maintainer"
check "the spec Packager is wrong" test "$(spec_field Packager)" = "$maintainer"
check "the control source is not wineo" has_line "$control" 'Source: wineo'
check "the control package is not wineo" has_line "$control" 'Package: wineo'
check "the spec name is not wineo" test "$(spec_field Name)" = wineo

spec_version=$(spec_field Version)
spec_release=$(spec_field Release)
deb_version=$(sed -n -E '1 s/^wineo \(([^)]+)\) .*/\1/p' "$changelog")
check "the spec Version is not X.Y.Z: $spec_version" \
    grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$' <<<"$spec_version"
check "the changelog is not 'wineo (X.Y.Z-1ppaN) UNRELEASED;': $deb_version" \
    grep -qE '^wineo \([0-9]+\.[0-9]+\.[0-9]+-[0-9]+ppa[0-9]+\) UNRELEASED; ' "$changelog"
check "the changelog and spec versions differ: $deb_version, $spec_version" \
    test "${deb_version%%-*}" = "$spec_version"
check "the spec Release is not N%{?dist}: $spec_release" \
    grep -qE '^[1-9][0-9]*%\{\?dist\}$' <<<"$spec_release"
spec_changelog=$(sed -n '/^%changelog/ { n; p; q; }' "$spec")
changelog_ok() { [[ $spec_changelog == "* "*" $maintainer - $spec_version-${spec_release%%\%*}" ]]; }
check "the spec changelog does not end in $spec_version-${spec_release%%\%*}: $spec_changelog" changelog_ok
check "the spec does not build with thin LTO" has_line "$spec" 'export CARGO_PROFILE_RELEASE_LTO=thin'
check "the spec does not build with 16 codegen units" \
    has_line "$spec" 'export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16'
check "the spec source is not the vendored release tarball" \
    test "$(spec_field Source0)" = '%{url}/releases/download/v%{version}/%{name}-%{version}-vendored.tar.xz'
check "the spec does not unpack wineo-VERSION" has_line "$spec" '%autosetup -n %{name}-%{version}'
check "the spec is not x86_64 only" test "$(spec_field ExclusiveArch)" = x86_64
check "the control is not amd64 only" has_line "$control" 'Architecture: amd64'

# One summary and one description everywhere.
summary=$(awk '/^description: \|$/ { getline; sub("^  ", ""); print; exit }' "$nfpm")
check "the nfpm summary is empty" test -n "$summary"
check "the control summary differs from nfpm.yaml's" \
    has_line "$control" "Description: $summary"
check "the spec summary differs from nfpm.yaml's" test "$(spec_field Summary)" = "$summary"
# nfpm.yaml's description paragraph, as lines without their indent.
nfpm_text=$(awk '/^description: \|$/ { f = 1; next } f && /^  / { sub("^  ", ""); print; next } f { exit }' "$nfpm" | tail -n +2)
control_text=$(awk '/^Description:/ { f = 1; next } f && /^ / { sub("^ ", ""); print; next } f { exit }' "$control")
spec_text=$(awk '/^%description/ { f = 1; next } f && /^%/ { exit } f && NF { print }' "$spec")
check "the control description differs from nfpm.yaml's" test "$control_text" = "$nfpm_text"
check "the spec description differs from nfpm.yaml's" test "$spec_text" = "$nfpm_text"

# Runtime dependencies.
deb_depends=$(control_field Depends)
for package in "${ubuntu[@]}"; do
    check "the control does not depend on $package" has "$deb_depends" "$package"
done
for substvar in '${shlibs:Depends}' '${misc:Depends}'; do
    check "the control Depends lacks $substvar" has "$deb_depends" "$substvar"
done
# Depends holds the substitutions and the README list, nothing else.
check "the control Depends has entries beyond the README's list" \
    test "$(wc -l <<<"$deb_depends")" -eq $((${#ubuntu[@]} + 2))
check "the control Recommends differs from nfpm.yaml's deb list" \
    test "$(control_field Recommends)" = "$(nfpm_list deb recommends)"
check "the control Suggests differs from nfpm.yaml's deb list" \
    test "$(control_field Suggests)" = "$(nfpm_list deb suggests)"

mapfile -t fedora_requires < <(spec_requires fedora)
mapfile -t suse_requires < <(spec_requires suse)
for package in "${fedora[@]}"; do
    check "the spec does not require $package" has "$(printf '%s\n' "${fedora_requires[@]}")" "$package"
    if name=$(suse_name "$package"); then
        check "the openSUSE branch does not require $name (for $package)" \
            has "$(printf '%s\n' "${suse_requires[@]}")" "$name"
    else
        fail "no openSUSE name for $package; add it to suse_name"
    fi
done
check "the spec's Fedora branch has entries beyond the README's list" \
    test "${#fedora_requires[@]}" -eq "${#fedora[@]}"
check "the spec's openSUSE branch has entries beyond the README's list" \
    test "${#suse_requires[@]}" -eq "${#fedora[@]}"
spec_all=$(grep -E '^Requires:|^Recommends:' "$spec" | awk '{ print $2 }')
check "the spec lacks hicolor-icon-theme" has "$spec_all" hicolor-icon-theme
rpm_recommends=$(awk '/^Recommends:/ { print $2 }' "$spec" | sort)
check "the spec Recommends differ from nfpm.yaml's rpm list" \
    test "$rpm_recommends" = "$(nfpm_list rpm recommends | sort)"

# The Rust floor is written in both and must agree. rust-floor.sh holds the
# truth and is run at release time (step 13); here they only have to match.
deb_floor=$(build_depends | sed -n -E 's/^rustc@RUST@ \(>= ([0-9.]+)\)$/\1/p')
rpm_floor=$(sed -n -E 's/^BuildRequires:[[:space:]]+rust >= ([0-9.]+)$/\1/p' "$spec")
check "the control has no rustc floor" test -n "$deb_floor"
check "the control and spec Rust floors differ: $deb_floor, $rpm_floor" test "$deb_floor" = "$rpm_floor"
check "the control has no cargo floor of $deb_floor" \
    has "$(build_depends)" "cargo@RUST@ (>= $deb_floor)"
check "the spec has no cargo floor of $deb_floor" \
    has_line "$spec" "BuildRequires:  cargo >= $deb_floor"
check "the control does not need liblzma-dev and pkg-config" \
    test "$(build_depends | grep -cxE 'liblzma-dev|pkg-config')" -eq 2
check "the spec does not need liblzma" has_line "$spec" 'BuildRequires:  pkgconfig(liblzma)'

# Builds are offline, run no tests, and install through stage-install.sh.
check "debian/rules does not build with --frozen" grep -q -- '--release --frozen' "$rules"
check "the spec does not build with --frozen" grep -q -- '--release --frozen' "$spec"
check "debian/rules has no empty override_dh_auto_test" \
    awk '/^override_dh_auto_test:[[:space:]]*$/ { f = 1; next } f && /^\t/ { bad = 1 } f && /^[^\t#]/ { exit } END { exit !(f && !bad) }' "$rules"
check "the spec has a %check" test "$(grep -c '^%check' "$spec")" -eq 0
check "debian/rules does not install with stage-install.sh" \
    grep -q 'scripts/stage-install.sh --license-dir /usr/share/doc/wineo' "$rules"
check "the spec does not install with stage-install.sh" \
    grep -q 'scripts/stage-install.sh --license-dir %{_datadir}/licenses/%{name}' "$spec"
check "the spec keeps debuginfo on" grep -q '^%global debug_package %{nil}' "$spec"

# The spec's License is deny.toml's list, so the audit and the package cannot
# disagree. The audit can't see the fonts embedded in wineo-gui, and deny.toml
# has BSL-1.0 for Windows-only crates, which no package compiles.
mapfile -t allowed < <(sed -n '/^allow = \[/,/^\]/ { s/^[[:space:]]*"\([^"]*\)",$/\1/p }' "$root/deny.toml")
check "deny.toml has no allow list" test "${#allowed[@]}" -gt 0
read -r -a license_words <<<"$(spec_field License)"
expected_licenses=$({
    printf '%s\n' "${allowed[@]}" | grep -vxF BSL-1.0
    echo OFL-1.1
} | sort)
spec_licenses=$(printf '%s\n' "${license_words[@]}" | grep -vxF AND | sort)
check "the spec License is not deny.toml's list with OFL-1.1 and without BSL-1.0" \
    test "$spec_licenses" = "$expected_licenses"
check "the spec License lacks GPL-3.0-or-later" \
    grep -qw GPL-3.0-or-later <<<"${license_words[*]}"

# prepare-debian-tree.sh: the only per-series substitution.
tmp=$(mktemp -d)
trap 'rm -rf "${tmp:?}"' EXIT
prepare() { # SERIES: a fresh tree with the recipe, prepared for SERIES
    local tree=$tmp/$1
    mkdir -p "$tree/packaging"
    cp -r "$root/packaging/debian" "$tree/packaging/debian"
    "$script_dir/prepare-debian-tree.sh" "$tree" "$1" >/dev/null 2>&1
}
for series in jammy noble resolute stonking; do
    if prepare "$series"; then
        pass
    else
        fail "prepare-debian-tree.sh failed for $series"
        continue
    fi
    tree=$tmp/$series
    check "$series changelog does not start with the series and ~${series}1" \
        grep -qE "^wineo \\([0-9.]+-1ppa[0-9]+~${series}1\\) $series; " "$tree/debian/changelog"
    markers=$(grep -v '^#' "$tree/debian/control" | grep -c '@RUST@' || true)
    check "$series control still has a marker" test "$markers" -eq 0
    case $series in
        jammy | noble) want=-1.91 ;;
        *) want= ;;
    esac
    check "$series control does not name rustc$want" \
        has_line "$tree/debian/control" " rustc$want (>= $deb_floor),"
    check "$series control does not name cargo$want" \
        has_line "$tree/debian/control" " cargo$want (>= $deb_floor),"
    extra=$(diff -rq "$tree/debian" "$tree/packaging/debian" | grep -vE '/(control|changelog) and ' || true)
    check "$series debian/ differs from the recipe beyond control and changelog: $extra" test -z "$extra"
done
rc=0
"$script_dir/prepare-debian-tree.sh" "$tmp/jammy" noble >/dev/null 2>&1 || rc=$?
check "prepare-debian-tree.sh overwrote an existing debian/ (exit $rc)" test "$rc" -eq 1
rc=0
"$script_dir/prepare-debian-tree.sh" "$tmp/jammy" focal >/dev/null 2>&1 || rc=$?
check "prepare-debian-tree.sh did not exit 2 for an unknown series (exit $rc)" test "$rc" -eq 2

# --recipe: a bumped recipe from outside the tree, whose packaging/debian must
# stay byte for byte what the orig tarball holds.
tree=$tmp/recipe-tree
bumped=$tmp/bumped
mkdir -p "$tree/packaging"
cp -r "$root/packaging/debian" "$tree/packaging/debian"
cp -r "$root/packaging/debian" "$bumped"
sed -i -E '1 s/-1ppa[0-9]+\)/-1ppa9)/' "$bumped/changelog"
before=$(cd "$tree/packaging" && find debian -type f -exec sha256sum {} + | sort)
rc=0
"$script_dir/prepare-debian-tree.sh" --recipe "$bumped" "$tree" jammy >/dev/null 2>&1 || rc=$?
check "prepare-debian-tree.sh --recipe failed (exit $rc)" test "$rc" -eq 0
check "--recipe did not take the changelog from the recipe" \
    grep -qE '^wineo \([0-9.]+-1ppa9~jammy1\) jammy; ' "$tree/debian/changelog"
check "--recipe did not fill in the series' Rust" \
    has_line "$tree/debian/control" " rustc-1.91 (>= $deb_floor),"
after=$(cd "$tree/packaging" && find debian -type f -exec sha256sum {} + | sort)
check "--recipe changed the tree's packaging/debian" test "$before" = "$after"
check "--recipe changed the recipe it was given" \
    grep -q '^wineo ([0-9.]*-1ppa9) UNRELEASED;' "$bumped/changelog"
rc=0
"$script_dir/prepare-debian-tree.sh" --recipe "$tmp/no-such-recipe" "$tmp/noble" jammy >/dev/null 2>&1 || rc=$?
check "--recipe with no recipe there did not exit 1 (exit $rc)" test "$rc" -eq 1
rc=0
"$script_dir/prepare-debian-tree.sh" --recipe >/dev/null 2>&1 || rc=$?
check "--recipe with no DIR did not exit 2 (exit $rc)" test "$rc" -eq 2

echo "$((cases - failures)) of $cases cases behaved correctly"
[[ $failures -eq 0 ]]
