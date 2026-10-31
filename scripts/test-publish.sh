#!/bin/bash
# Tests publish-packages.sh against a small real git repository, with stubs on
# PATH for gh, curl, podman, makepkg, pycargoebuild, gpg, and sleep, and
# --dest-dir fakes for the destinations, so nothing touches a service and
# nothing sleeps. git, tar, cargo, gentoo-ebuild.sh, and prepare-debian-tree.sh
# are the real ones. Each failure case checks the reason as well as the exit
# status, and that nothing was uploaded past the point where it should stop.
#
# The podman stub runs every container's script for real, in a directory
# standing in for the container's root, with stub tools (copr-cli, osc, dput,
# debsign, rpmbuild, and so on) that record their arguments in container paths.
# So a container script that stops calling its tool, or calls it differently,
# fails a test. Runs without --dest-dir go through the real upload branches;
# git's insteadOf turns the overlay's and the AUR's URLs into local repositories.
#
# Each stub tool call is kept in $STUB/calls, one argument per line, and compared
# whole, so a merged or split argument fails. tools.log is only their order.
#
# testdata/publish/ holds real public answers from Launchpad (mozillateam's
# PPA) and COPR (@copr/copr), fetched 2026-10-30, with people's names replaced.
# lp-sources.json and lp-sources-none.json answer the script's exact query;
# lp-sources-partial.json and lp-binaries.json are real partial pages. The OBS
# XML here is written by hand after osc's own parsing, since OBS has no anonymous
# read.
#
# Usage: scripts/test-publish.sh
# Exits non-zero if any case behaves wrongly. Needs cargo, jq, and python3.
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(cd "$script_dir/.." && pwd)
version=$(awk '/^Version:/ { print $2; exit }' "$root/packaging/rpm/wineo.spec")
tag=v$version

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
    if "$@" >/dev/null 2>&1; then pass; else fail "$what"; fi
}

tmp=$(mktemp -d)
trap 'rm -rf "${tmp:?}"' EXIT
stubs=$tmp/stubs
mkdir -p "$stubs" "$tmp/home"

# The toolchain stays where it is when HOME moves below.
export RUSTUP_HOME=${RUSTUP_HOME:-$HOME/.rustup} CARGO_HOME=${CARGO_HOME:-$HOME/.cargo}
export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
export HOME=$tmp/home XDG_CONFIG_HOME=$tmp/home/.config XDG_STATE_HOME=$tmp/home/.state
export XDG_CACHE_HOME=$tmp/home/.cache
git config --global init.defaultBranch main
git config --global advice.detachedHead false

# Stubs. All state lives under $STUB, one directory per scenario.

cat >"$stubs/gh" <<'EOF'
#!/bin/bash
echo "$*" >>"$STUB/gh.log"
# tick NAME: true once NAME has been asked for more than its -after count.
tick() {
    local count=$(($(cat "$STUB/count-$1" 2>/dev/null || echo 0) + 1))
    echo "$count" >"$STUB/count-$1"
    ((count > $(cat "$STUB/$1-after" 2>/dev/null || echo 0)))
}
case "$1 $2" in
    "api repos/MeandrousShark/wineo/git/ref/tags/"*)
        [[ -f $STUB/tag.json ]] && tick tag || exit 1
        cat "$STUB/tag.json"
        ;;
    "api repos/MeandrousShark/wineo/compare/"*)
        cat "$STUB/compare.json"
        ;;
    "run list")
        [[ " $* " == *" -R MeandrousShark/wineo "* ]] || exit 99
        count=$(($(cat "$STUB/count-runs" 2>/dev/null || echo 0) + 1))
        echo "$count" >"$STUB/count-runs"
        while ((count > 1)) && [[ ! -f $STUB/runs.$count.json ]]; do count=$((count - 1)); done
        cat "$STUB/runs.$count.json"
        ;;
    "release view")
        [[ -f $STUB/release.json ]] && tick release || exit 1
        cat "$STUB/release.json"
        ;;
    "release download")
        shift 2
        while (($# > 0)); do
            case $1 in
                -D) out=$2; shift 2 ;;
                --pattern) name=$2; shift 2 ;;
                *) shift ;;
            esac
        done
        cp "$STUB/assets/$name" "$out/"
        ;;
    "auth git-credential") ;;
    *)
        echo "gh: unexpected call: $*" >&2
        echo "UNEXPECTED $*" >>"$STUB/gh.log"
        exit 99
        ;;
esac
EOF

cat >"$stubs/curl" <<'EOF'
#!/bin/bash
echo "$*" >>"$STUB/curl.log"
out=
url=
while (($# > 0)); do
    case $1 in
        -o) out=$2; shift 2 ;;
        --proto | --retry | --max-time | --data-urlencode) shift 2 ;;
        -*) shift ;;
        *) url=$1; shift ;;
    esac
done
case $url in
    */archive/refs/tags/*) [[ -f $STUB/no-tag-tarball ]] && exit 22; cp "$STUB/tag.tar.gz" "$out" ;;
    */license-mapping.conf) : >"$out" ;;
    *copr*/build/list*)
        offset=${url##*offset=}
        offset=${offset%%&*}
        [[ ! -f $STUB/copr-down-$offset ]] || exit 22
        if [[ -f $STUB/copr-page-$offset.json ]]; then
            cat "$STUB/copr-page-$offset.json"
        else
            echo "{\"items\":[],\"meta\":{\"offset\":$offset,\"limit\":100}}"
        fi
        ;;
    *copr*/build-chroot/list*) cat "$STUB/copr-chroots.json" ;;
    *api.launchpad.net*)
        if [[ -f $STUB/lp-sources.json ]]; then
            cat "$STUB/lp-sources.json"
        else
            echo '{"start":0,"total_size":0,"entries":[]}'
        fi
        ;;
    *) exit 22 ;;
esac
EOF

cat >"$stubs/podman" <<'EOF'
#!/bin/bash
echo "$*" >>"$STUB/podman.log"
case $1 in
    image) [[ -e $STUB/image-${3##*/} ]]; exit ;;
    commit)
        [[ ! -f $STUB/fail-commit ]] || exit 1
        name=${*: -1}
        touch "$STUB/image-${name##*/}"
        exit
        ;;
    rm) exit 0 ;;
    rmi)
        rm -f "$STUB/image-${3##*/}"
        exit 0
        ;;
    unshare)
        shift
        exec "$@"
        ;;
    run) shift ;;
    *) exit 1 ;;
esac

# C stands in for the container's root, with each mount a symlink in it.
export C
C=$(mktemp -d)
trap 'rm -rf "${C:?}"' EXIT
mkdir -p "$C/tmp" "$C/etc/portage" "$C/usr/share/portage/config"
: >"$C/usr/share/portage/config/repos.conf"
environment=()
workdir=/tmp
while (($# > 0)); do
    case $1 in
        --rm) shift ;;
        --name | --tmpfs) shift 2 ;;
        -e)
            environment+=("$2")
            shift 2
            ;;
        -w)
            workdir=$2
            shift 2
            ;;
        -v)
            IFS=: read -r host inside _ <<<"$2"
            mkdir -p "$C$(dirname "$inside")"
            ln -s "$host" "$C$inside"
            shift 2
            ;;
        -*)
            echo "podman stub: unknown option $1" >&2
            exit 125
            ;;
        *) break ;;
    esac
done
shift # the image

# Container paths in the script become paths under C.
in_root() {
    perl -pe 's{(?<![\w./-])(/work|/files|/status|/overlay|/tag|/root|/tmp|/etc/portage|/usr/share/portage|/var/cache/distfiles|/var/db/repos|/var/tmp/portage|/signing-key\.asc)(?![\w-])}{$ENV{C}$1}g' <<<"$1"
}
args=()
for arg in "$@"; do
    mapped=$(in_root "$arg")
    args+=("$mapped")
done
mapped_env=()
for item in "${environment[@]}"; do
    mapped=$(in_root "$item")
    mapped_env+=("$mapped")
done
workdir=$(in_root "$workdir")
cd "$workdir" || exit 1
env "${mapped_env[@]}" PATH="$STUB_TOOLS:$PATH" "${args[@]}"
EOF

mkdir "$stubs/in-container"
cat >"$stubs/in-container/tool" <<'EOF'
#!/bin/bash
name=${0##*/}
args=()
for arg in "$@"; do args+=("${arg//$C/}"); done
mkdir -p "$STUB/calls"
count=$(find "$STUB/calls" -type f | wc -l)
printf '%s\n' "$name" "${args[@]}" >"$STUB/calls/$(printf %04d "$count")"
echo "$name" >>"$STUB/tools.log"
if [[ -f $STUB/fail-tool-$name ]]; then
    cat "$STUB/fail-tool-$name"
    exit 1
fi
case $name in
    rpmbuild)
        [[ $# -eq 6 && $1 == -bs && $2 == --define && $3 == '_topdir '* && $4 == --define ]] || exit 64
        top=$3
        top=${top#_topdir }
        spec=$top/SPECS/wineo.spec
        id=$(awk '/^Version:/ { v = $2 } /^Release:/ { sub(/%.*/, "", $2); print v "-" $2; exit }' "$spec")
        cmp -s "$top/SOURCES/wineo-$VERSION_UNDER_TEST-vendored.tar.xz" \
            "$STUB/assets/wineo-$VERSION_UNDER_TEST-vendored.tar.xz" || exit 1
        touch "$top/SRPMS/wineo-$id.src.rpm"
        ;;
    dpkg-buildpackage)
        [[ $# -eq 5 && $1 == -S && $2 == -sa && $3 == -d && $4 == -us && $5 == -uc ]] || exit 64
        id=$(sed -nE '1 s/^wineo \(([^)]+)\).*/\1/p' debian/changelog)
        cp debian/control "../wineo_$id.dsc"
        head -n 1 debian/changelog >"../wineo_${id}_source.changes"
        if cmp packaging/debian/changelog "$STUB/orig-changelog" >/dev/null 2>&1; then
            echo "packaging/debian untouched" >>"../wineo_${id}_source.changes"
        fi
        touch "../wineo_$id.debian.tar.xz" "../wineo_${id}_source.buildinfo"
        ;;
    ebuild)
        [[ $# -eq 2 && $2 == manifest ]] || exit 64
        mkdir -p ../../metadata/md5-cache
        touch ../../metadata/md5-cache/stale
        dist=$C/var/cache/distfiles/wineo-$VERSION_UNDER_TEST.gh.tar.gz
        size=$(stat -c %s "$dist")
        [[ ! -f $STUB/stale-manifest ]] || size=1
        echo "DIST ${dist##*/} $size BLAKE2B stub" >>Manifest
        ;;
    osc)
        case $1 in
            api)
                # Only the exact endpoints have answers, as on the server.
                project=/build/home:meandrousshark
                case $2 in
                    /source/home:meandrousshark/wineo) file=directory ;;
                    /source/home:meandrousshark/wineo/wineo.spec) file=spec ;;
                    "$project/_result?package=wineo&arch=x86_64") file=result ;;
                    "$project/"*"/x86_64/_jobhistory?package=wineo&limit=5")
                        file=${2#"$project"/}
                        file=jobhistory-${file%%/*}
                        ;;
                    *) file=no-such-endpoint ;;
                esac
                if [[ ! -f $STUB/obs/$file ]]; then
                    echo "Server returned an error: HTTP Error 404: Not Found" >&2
                    exit 1
                fi
                cat "$STUB/obs/$file"
                ;;
            checkout)
                mkdir -p "$2/$3"
                touch "$2/$3/wineo-0.9.0-vendored.tar.xz"
                ;;
            commit)
                ls >"$STUB/osc-tree"
                if [[ -d $STUB/obs-after-commit ]]; then
                    rm -rf "$STUB/obs"
                    cp -r "$STUB/obs-after-commit" "$STUB/obs"
                fi
                ;;
        esac
        ;;
esac
EOF
chmod +x "$stubs/in-container/tool"
for tool in copr-cli osc dput debsign rpmbuild dpkg-buildpackage gpg dnf apt-get \
    emerge-webrsync getuto emerge ebuild pkgcheck; do
    ln -s tool "$stubs/in-container/$tool"
done
export STUB_TOOLS=$stubs/in-container

# The host's side of signing. gpg never runs for real in these tests.
cat >"$stubs/gpg" <<'EOF'
#!/bin/bash
case $1 in
    --list-keys) [[ ! -f $STUB/no-key ]] ;;
    --export) echo "fake public key" ;;
    *) exit 1 ;;
esac
EOF
cat >"$stubs/gpgconf" <<'EOF'
#!/bin/bash
case $1 in
    --launch) ;;
    --list-dirs) echo "$STUB/S.gpg-agent" ;;
    *) exit 1 ;;
esac
EOF

# Written the way makepkg writes it, from the recipe's real values.
cat >"$stubs/makepkg" <<'EOF'
#!/bin/bash
[[ $1 == --printsrcinfo ]] || exit 1
source ./PKGBUILD
echo "pkgbase = $pkgname"
printf '\tpkgver = %s\n\tpkgrel = %s\n' "$pkgver" "$pkgrel"
for value in "${source[@]}"; do printf '\tsource = %s\n' "$value"; done
for value in "${sha256sums[@]}"; do printf '\tsha256sums = %s\n' "$value"; done
if [[ -n ${b2sums+set} ]]; then
    for value in "${b2sums[@]}"; do printf '\tb2sums = %s\n' "$value"; done
fi
EOF

cat >"$stubs/sleep" <<'EOF'
#!/bin/bash
echo "$1" >>"$STUB/sleep.log"
EOF

# Keeps the lock it was pointed at; gentoo-ebuild.sh passes the crates' paths.
cat >"$stubs/pycargoebuild" <<'EOF'
#!/bin/bash
for arg in "$@"; do
    if [[ -f $arg/../../Cargo.lock ]]; then cp "$arg/../../Cargo.lock" "$STUB/lock-seen"; fi
done
EOF

cat >"$stubs/cargo-deny" <<'EOF'
#!/bin/bash
echo "$PWD $*" >>"$STUB/deny.log"
[[ ! -f $STUB/deny-fails ]]
EOF

chmod +x "$stubs"/*

# The fixture: everything the script reads from a checkout, at the tag.

base=$tmp/base
mkdir -p "$base/scripts" "$base/.github/workflows" "$base/crates/fixture/src" "$base/crates/wineo-cli"
touch "$base/crates/wineo-cli/.keep"
cp "$root"/scripts/{publish-packages,prepare-debian-tree,gentoo-ebuild,rust-floor}.sh "$base/scripts/"
cp -r "$root/packaging" "$base/packaging"
cp "$root/PKGBUILD" "$base/PKGBUILD"
cp "$root/.github/workflows/release.yml" "$base/.github/workflows/"
cat >"$base/Cargo.toml" <<EOF
[workspace]
members = ["crates/fixture"]
resolver = "2"

[workspace.package]
version = "$version"
EOF
cat >"$base/crates/fixture/Cargo.toml" <<EOF
[package]
name = "fixture"
version.workspace = true
edition = "2021"
rust-version = "1.90"
EOF
: >"$base/crates/fixture/src/lib.rs"
: >"$base/deny.toml"
(cd "$base" && cargo generate-lockfile --offline -q)
git -C "$base" init -q
git -C "$base" add .
git -C "$base" commit -qm fixture
git -C "$base" tag -a "$tag" -m "$tag"
commit=$(git -C "$base" rev-parse HEAD)
tag_sha=$(git -C "$base" rev-parse "refs/tags/$tag")

# new_scenario: fresh checkout, canned GitHub, and an empty destination.
new_scenario() {
    scenario=$tmp/s$cases-$RANDOM
    repo=$scenario/repo
    dest=$scenario/dest
    export STUB=$scenario/stub
    mkdir -p "$STUB/assets" "$dest/control" "$scenario/state"
    cp -a "$base" "$repo"
    make_assets
    cp "$repo/packaging/debian/changelog" "$STUB/orig-changelog"
    git -C "$repo" archive --prefix="wineo-$version/" "$tag" | gzip -n >"$STUB/tag.tar.gz"
    printf '{"object":{"sha":"%s","type":"tag"}}\n' "$tag_sha" >"$STUB/tag.json"
    echo '{"status":"identical","ahead_by":0,"behind_by":0}' >"$STUB/compare.json"
    set_runs "$(run_json completed success)"
    make_release false
    : >"$STUB/gh.log"
    : >"$STUB/tools.log"
}

make_assets() {
    git -C "$repo" archive --prefix="wineo-$version/" "$tag" | xz -0 >"$STUB/assets/wineo-$version-vendored.tar.xz"
    echo tarball >"$STUB/assets/wineo-$version-x86_64-linux.tar.gz"
    echo deb >"$STUB/assets/wineo_${version}_amd64.deb"
    echo rpm >"$STUB/assets/wineo-$version-1.x86_64.rpm"
    rm -f "$STUB/assets/SHA256SUMS"
    (cd "$STUB/assets" && sha256sum -- * >"$STUB/SHA256SUMS" && mv "$STUB/SHA256SUMS" SHA256SUMS)
}

run_json() {
    printf '{"databaseId":7,"headSha":"%s","status":"%s","conclusion":"%s","url":"https://github.invalid/runs/7"}' \
        "$commit" "$1" "$2"
}

# set_runs JSON...: what each successive `gh run list` answers.
set_runs() {
    local i=1 run
    rm -f "$STUB"/runs.*.json
    for run in "$@"; do
        echo "[$run]" >"$STUB/runs.$i.json"
        i=$((i + 1))
    done
}

make_release() {
    (cd "$STUB/assets" && ls) | jq -R . | jq -s --arg tag "$tag" --argjson draft "$1" \
        '{tagName: $tag, isDraft: $draft, url: "https://github.invalid/release",
          assets: map({name: .})}' >"$STUB/release.json"
}

# publish ARGS...: runs the script with the fakes, leaving $out and $rc.
publish() {
    publish_real --dest-dir "$dest" "$@"
}

publish_real() {
    out=$scenario/output
    rc=0
    env PATH="$stubs:$PATH" CARGO_DENY="$stubs/cargo-deny" VERSION_UNDER_TEST="$version" \
        XDG_STATE_HOME="$scenario/state" \
        WINEO_PUBLISH_WORK="$scenario/work" WINEO_PUBLISH_INTERVAL=0 WINEO_PUBLISH_BUILD_INTERVAL=0 \
        WINEO_PUBLISH_MIRROR_LIMIT="${MIRROR_LIMIT:-30}" WINEO_PUBLISH_RUN_LIMIT="${RUN_LIMIT:-30}" \
        WINEO_PUBLISH_RELEASE_LIMIT="${RELEASE_LIMIT:-30}" WINEO_PUBLISH_COPR_LIMIT="${BUILD_LIMIT:-30}" \
        WINEO_PUBLISH_OBS_LIMIT="${BUILD_LIMIT:-30}" WINEO_PUBLISH_PPA_LIMIT="${BUILD_LIMIT:-30}" \
        "$repo/scripts/publish-packages.sh" "$version" "$@" >"$out" 2>&1 || rc=$?
}

said() { grep -qE -- "$1" "$out"; }
uploads() { cat "$dest/uploads.log" 2>/dev/null || true; }
upload_count() { uploads | grep -c "${1:-.}" || true; }

# expect_refused NAME REASON: stopped with REASON and nothing went out.
expect_refused() {
    local name=$1 reason=$2
    check "$name: exit $rc, not 1" test "$rc" -eq 1
    check "$name: no reason matching '$reason' in: $(tail -n 3 "$out")" said "$reason"
    check "$name: something was uploaded: $(uploads)" test -z "$(uploads)"
    check "$name: a destination ran" test "$(grep -c '^== overlay$' "$out")" -eq 0
}

# called TOOL ARG...: some call had exactly these arguments.
called() {
    local want file
    want=$(printf '%s\n' "$@")
    for file in "$STUB"/calls/*; do
        [[ -f $file ]] || continue
        if [[ $(cat "$file") == "$want" ]]; then return 0; fi
    done
    return 1
}

# called_times N TOOL ARG...
called_times() {
    local times=$1 want file count=0
    shift
    want=$(printf '%s\n' "$@")
    for file in "$STUB"/calls/*; do
        [[ -f $file ]] || continue
        if [[ $(cat "$file") == "$want" ]]; then count=$((count + 1)); fi
    done
    ((count == times))
}

tool_ran() { grep -qx "$1" "$STUB/tools.log"; }

# What the OBS API answers, written by hand after osc's parsing.
obs_dir() { # DIR SPEC_RELEASE DIRTY CODE JOB_SRCMD5 JOB_CODE [REPO_STATE] [SRCMD5]
    local dir=$1 target repo_state=${7:-published} srcmd5=${8-abc} attribute=''
    [[ -z $srcmd5 ]] || attribute=" srcmd5=\"$srcmd5\""
    rm -rf "$dir"
    mkdir -p "$dir"
    printf 'Version: %s\nRelease: %s%%{?dist}\n' "${OBS_VERSION:-1.2.3}" "$2" >"$dir/spec"
    echo "<directory name=\"wineo\" rev=\"3\"$attribute/>" >"$dir/directory"
    {
        echo '<resultlist state="x">'
        for target in openSUSE_Tumbleweed 16.0 15.6; do
            echo "<result repository=\"$target\" arch=\"x86_64\" code=\"$repo_state\" state=\"$repo_state\" dirty=\"$3\"><status package=\"wineo\" code=\"$4\"/></result>"
        done
        echo '</resultlist>'
    } >"$dir/result"
    for target in openSUSE_Tumbleweed 16.0 15.6; do
        attribute=''
        [[ -z $5 ]] || attribute=" srcmd5=\"$5\""
        echo "<jobhistlist><jobhist package=\"wineo\" srcmd5=\"old\" code=\"failed\"/><jobhist package=\"wineo\"$attribute code=\"$6\"/></jobhistlist>" >"$dir/jobhistory-$target"
    done
}

reviewed_commit() {
    git -C "$repo" add -- .
    git -C "$repo" commit -qm "$1"
}

all_nine="overlay
aur wineo
aur wineo-bin
aur wineo-git
copr all
obs all
ppa jammy
ppa noble
ppa resolute
ppa stonking"

# Usage errors, before anything else happens.

for args in "--only aur --skip ppa" "--only nope" "--skip nope" "--only" \
    "--only aur --only ppa" "--bogus"; do
    new_scenario
    read -r -a words <<<"$args"
    publish "${words[@]}"
    check "'$args': exit $rc, not 2" test "$rc" -eq 2
    check "'$args': it called gh" test ! -s "$STUB/gh.log"
done
new_scenario
out=$scenario/output
rc=0
"$repo/scripts/publish-packages.sh" 1.1 >"$out" 2>&1 || rc=$?
check "a bad version: exit $rc, not 2" test "$rc" -eq 2

# A full run.

new_scenario
publish
check "a full run: exit $rc, not 0: $(tail -n 5 "$out")" test "$rc" -eq 0
check "a full run went to the wrong places, or in the wrong order: $(uploads)" \
    test "$(uploads | awk '{ print ($2 == "overlay" ? "overlay" : $2 " " $3) }')" = "$all_nine"
overlay_bare=$dest/remote/overlay.git
show() { git -C "$1" show "HEAD:$2" 2>/dev/null; }
check "the overlay lacks the ebuild" show "$overlay_bare" "games-util/wineo/wineo-$version.ebuild"
check "the first run did not lay down all of packaging/gentoo" \
    test -n "$(show "$overlay_bare" profiles/repo_name)" -a \
    -n "$(show "$overlay_bare" metadata/layout.conf)" -a \
    -n "$(show "$overlay_bare" LICENSE)" -a \
    -n "$(show "$overlay_bare" games-util/wineo/metadata.xml)" -a \
    -n "$(show "$overlay_bare" games-util/wineo/wineo-9999.ebuild)"
check "Portage's cache went into the overlay" \
    test "$(git -C "$overlay_bare" ls-tree -r --name-only HEAD | grep -c md5-cache || true)" -eq 0
check "the Manifest does not hold the tag tarball" \
    grep -q "^DIST wineo-$version.gh.tar.gz $(stat -c %s "$STUB/tag.tar.gz") " \
    <(show "$overlay_bare" games-util/wineo/Manifest)
check "the overlay's branch is not main" git -C "$overlay_bare" rev-parse -q --verify refs/heads/main
tag_sum=$(sha256sum "$STUB/tag.tar.gz" | awk '{print $1}')
bin_sum=$(sha256sum "$STUB/assets/wineo-$version-x86_64-linux.tar.gz" | awk '{print $1}')
aur=$dest/remote
check "wineo is not pinned to the tag tarball" \
    grep -qxF "sha256sums=('$tag_sum')" <(show "$aur/wineo.git" PKGBUILD)
check "wineo-bin is not pinned from SHA256SUMS" \
    grep -qxF "sha256sums=('$bin_sum')" <(show "$aur/wineo-bin.git" PKGBUILD)
check "wineo-git lost its SKIP" grep -qxF "sha256sums=('SKIP')" <(show "$aur/wineo-git.git" PKGBUILD)
check "wineo-git's pkgver was not filled in" grep -qxF "pkgver=$version" <(show "$aur/wineo-git.git" PKGBUILD)
for package in wineo wineo-bin wineo-git; do
    check "$package has no LICENSE" grep -q 'Permission to use' <(show "$aur/$package.git" LICENSE)
    check "$package's .SRCINFO has the wrong pkgver" \
        grep -qE "pkgver = $version$" <(show "$aur/$package.git" .SRCINFO)
    check "$package's branch is not master" git -C "$aur/$package.git" rev-parse -q --verify refs/heads/master
done
check "the pins reached the checkout" grep -qxF "sha256sums=('SKIP')" "$repo/packaging/aur/wineo/PKGBUILD"
check "the checkout changed" test -z "$(git -C "$repo" status --porcelain)"
check "the SRPM was not recorded" \
    test -f "$dest/uploads/copr/all/$version-1/wineo-$version-1.src.rpm"
check "OBS did not get the spec and the tarball" \
    test -f "$dest/uploads/obs/all/$version-1/wineo.spec" -a \
    -f "$dest/uploads/obs/all/$version-1/wineo-$version-vendored.tar.xz"
for series in jammy noble resolute stonking; do
    ppa=$dest/uploads/ppa/$series/$version-1ppa1~${series}1
    check "$series has the wrong version" \
        grep -q "^wineo ($version-1ppa1~${series}1) $series;" "$ppa/wineo_$version-1ppa1~${series}1_source.changes"
    check "$series edited packaging/debian in the unpacked tree" \
        grep -q 'packaging/debian untouched' "$ppa/wineo_$version-1ppa1~${series}1_source.changes"
    check "$series's orig differs from the Release's tarball" \
        cmp -s "$ppa/wineo_$version.orig.tar.xz" "$STUB/assets/wineo-$version-vendored.tar.xz"
done
check "jammy's control doesn't name rustc-1.91" grep -q 'rustc-1.91' "$dest/uploads/ppa/jammy/$version-1ppa1~jammy1/wineo_$version-1ppa1~jammy1.dsc"
check "stonking's control names rustc-1.91" test "$(grep -c 'rustc-1.91' "$dest/uploads/ppa/stonking/$version-1ppa1~stonking1/wineo_$version-1ppa1~stonking1.dsc")" -eq 0
check "the license check did not run on the tag's tree" \
    grep -qE "/wineo-$version --locked check licenses$" "$STUB/deny.log"
check "the source package isn't -S -sa unsigned" \
    called_times 4 dpkg-buildpackage -S -sa -d -us -uc
check "the tag's tree isn't mounted read-only" \
    grep -q ":/tag:ro" "$STUB/podman.log"
check "rpmbuild ran differently" called rpmbuild -bs --define '_topdir /work/rpmbuild' \
    --define 'dist %{nil}' /work/rpmbuild/SPECS/wineo.spec
check "ebuild manifest ran differently" called ebuild "wineo-$version.ebuild" manifest
check "pkgcheck ran differently" called pkgcheck scan --repo wineo --exit error
check "gh was asked to change something: $(grep UNEXPECTED "$STUB/gh.log" || true)" \
    test "$(grep -c UNEXPECTED "$STUB/gh.log")" -eq 0
check "gh ran without -R or against another repository" \
    test "$(grep -vE '^(api repos/MeandrousShark/wineo/|.* -R MeandrousShark/wineo( |$))' "$STUB/gh.log" | grep -c . )" -eq 0
check "the test reached a service: $(grep -vE 'github.com/MeandrousShark/wineo/archive|raw.githubusercontent.com/gentoo' "$STUB/curl.log" || true)" \
    test "$(grep -cvE 'github.com/MeandrousShark/wineo/archive|raw.githubusercontent.com/gentoo' "$STUB/curl.log")" -eq 0
# The summary.
for line in "overlay +published \($version\)" "aur +published \($version-1\)" \
    "copr +published \($version-1\)" "obs +published \($version-1\)" "ppa +published \($version-1ppa1\)"; do
    check "the summary lacks '$line'" said "^  $line$"
done

# A rerun skips everything.
publish
check "a rerun: exit $rc, not 0" test "$rc" -eq 0
check "a rerun uploaded again: $(uploads | tail -n 3)" test "$(upload_count)" -eq 10
for d in overlay aur copr obs ppa; do
    check "a rerun did not skip $d" said "^  $d +already published"
done

# Waiting: started before anything exists.

new_scenario
echo 2 >"$STUB/tag-after"
echo 2 >"$STUB/release-after"
set_runs '' "$(run_json in_progress '')" "$(run_json completed success)"
echo '[]' >"$STUB/runs.1.json"
publish --only overlay
check "started early: exit $rc, not 0: $(tail -n 3 "$out")" test "$rc" -eq 0
check "started early: it did not wait for each" \
    test "$(grep -c '^== waiting for' "$out")" -ge 3
check "started early: the upload is missing" test "$(upload_count overlay)" -eq 1

# Failures before anything is published.

new_scenario
set_runs "$(run_json completed failure)"
publish
expect_refused "a failed run" 'ended in failure: https://github.invalid/runs/7'
check "a failed run: it went on to the Release" test "$(grep -c 'release view' "$STUB/gh.log")" -eq 0

new_scenario
make_release true
publish
expect_refused "a draft" 'is a draft: https://github.invalid/release'
check "a draft: it downloaded assets" test "$(grep -c 'release download' "$STUB/gh.log")" -eq 0

new_scenario
rm "$STUB/release.json"
RELEASE_LIMIT=0 publish
expect_refused "a missing Release" "there is no Release for $tag after 0s"

new_scenario
echo corrupt >>"$STUB/assets/wineo-$version-x86_64-linux.tar.gz"
publish
expect_refused "a corrupted asset" "does not match SHA256SUMS"
check "a corrupted asset: it does not name the file" said "wineo-$version-x86_64-linux.tar.gz: FAILED"

new_scenario
rm "$STUB/assets/wineo_${version}_amd64.deb"
make_release false
publish
expect_refused "a missing asset" "has no wineo_${version}_amd64.deb"

new_scenario
rm "$STUB/assets/SHA256SUMS"
make_release false
publish
expect_refused "missing SHA256SUMS" "has no SHA256SUMS"

new_scenario
(cd "$STUB/assets" && grep -v '_amd64.deb' SHA256SUMS >sums && mv sums SHA256SUMS)
publish
expect_refused "a short SHA256SUMS" "does not list exactly"

new_scenario
printf '{"object":{"sha":"%040d","type":"tag"}}\n' 1 >"$STUB/tag.json"
publish
expect_refused "a mismatched tag" "local $tag is not GitHub's"

new_scenario
rm "$STUB/tag.json"
MIRROR_LIMIT=0 publish
expect_refused "a tag that never arrives" "did not reach GitHub in 0s"

new_scenario
echo '[]' >"$STUB/runs.1.json"
rm "$STUB/release.json"
RUN_LIMIT=0 publish
expect_refused "a run that never comes" "release run for $tag did not finish"

new_scenario
set_runs "$(run_json in_progress '')"
RUN_LIMIT=0 publish
expect_refused "a run that never ends" "release run for $tag did not finish"

# An old tag whose run has aged out still publishes, on the Release's word.
new_scenario
echo '[]' >"$STUB/runs.1.json"
publish --only overlay
check "no run left: exit $rc, not 0" test "$rc" -eq 0

new_scenario
echo '{"status":"ahead","ahead_by":1,"behind_by":0}' >"$STUB/compare.json"
MIRROR_LIMIT=0 publish
expect_refused "HEAD not on GitHub's main" "is not on GitHub's main"

new_scenario
echo dirty >>"$repo/packaging/rpm/wineo.spec"
publish
expect_refused "a dirty checkout" "uncommitted changes"
check "a dirty checkout: it reached GitHub first" test ! -s "$STUB/gh.log"

new_scenario
echo stray >"$repo/stray"
publish
expect_refused "an untracked file" "uncommitted changes"

new_scenario
git -C "$repo" tag -d "$tag" >/dev/null
publish
expect_refused "no local tag" "no local tag $tag"

new_scenario
sed -i -E 's/^Version:( +).*/Version:\19.9.9/' "$repo/packaging/rpm/wineo.spec"
reviewed_commit "another version"
publish
expect_refused "a spec at another version" "wineo.spec at HEAD is not at $version"

new_scenario
sed -i -E '1 s/^wineo \([^)]*\)/wineo (9.9.9-1ppa1)/' "$repo/packaging/debian/changelog"
reviewed_commit "another version"
publish
expect_refused "a changelog at another version" "changelog at HEAD is at 9.9.9-1ppa1"

new_scenario
touch "$STUB/deny-fails"
publish
expect_refused "a refused license" "check licenses failed"

# The floor is read from the Release's tree, so raise it there.
new_scenario
sed -i 's/rust-version = "1.90"/rust-version = "1.92"/' "$repo/crates/fixture/Cargo.toml"
reviewed_commit "raise the floor"
git -C "$repo" tag -d "$tag" >/dev/null
git -C "$repo" tag -a "$tag" -m "$tag"
tag_sha_new=$(git -C "$repo" rev-parse "refs/tags/$tag")
printf '{"object":{"sha":"%s","type":"tag"}}\n' "$tag_sha_new" >"$STUB/tag.json"
make_assets
make_release false
commit_saved=$commit
commit=$(git -C "$repo" rev-parse HEAD)
set_runs "$(run_json completed success)"
commit=$commit_saved
publish
expect_refused "a target below the floor" "ppa jammy 1.91"
check "a target below the floor: noble isn't named" said "ppa noble 1.91"
check "a target below the floor: resolute is named" test "$(grep -c 'ppa resolute' "$out")" -eq 0

# A destination that fails doesn't stop the later ones.

new_scenario
touch "$dest/control/fail-upload-copr"
publish
check "an upload that fails: exit $rc, not 1" test "$rc" -eq 1
check "an upload that fails: the reason is missing" said "^  copr +failed: the upload of all failed"
check "an upload that fails: it counts as uploaded: $(uploads)" test "$(upload_count ' copr ')" -eq 0
check "an upload that fails: the later ones stopped" \
    test "$(upload_count ' obs ')" -eq 1 -a "$(upload_count ' ppa ')" -eq 4
check "an upload that fails: the summary lost the others" said "^  ppa +published"
publish
check "the retry: exit $rc, not 0: $(tail -n 5 "$out")" test "$rc" -eq 0
check "the retry did not upload COPR once" test "$(upload_count ' copr ')" -eq 1
check "the retry uploaded others again: $(uploads)" test "$(upload_count)" -eq 10
check "the retry did not say it skipped obs" said "^  obs +already published"

for d in overlay aur; do
    new_scenario
    touch "$dest/control/fail-upload-$d"
    publish
    check "a refused $d push: exit $rc, not 1" test "$rc" -eq 1
    check "a refused $d push isn't in the summary" said "^  $d +failed"
    check "a refused $d push stopped the later ones" test "$(upload_count ' ppa ')" -eq 4
    publish --only "$d"
    check "the $d retry: exit $rc, not 0: $(tail -n 3 "$out")" test "$rc" -eq 0
done

# A failed part-way PPA upload keeps what went, and sends only the rest.
new_scenario
echo always >"$dest/control/fail-upload-ppa"
publish --only ppa
check "a refused PPA: exit $rc, not 1" test "$rc" -eq 1
check "a refused PPA went on to the next series: $(uploads)" test "$(upload_count)" -eq 0
rm "$dest/control/fail-upload-ppa"
mkdir -p "$dest/remote/ppa/jammy"
touch "$dest/remote/ppa/jammy/$version-1ppa1~jammy1"
publish --only ppa
check "a partial PPA: exit $rc, not 0" test "$rc" -eq 0
check "a partial PPA uploaded jammy again: $(uploads)" test "$(upload_count ' ppa jammy ')" -eq 0
check "a partial PPA missed the rest: $(uploads)" test "$(upload_count ' ppa ')" -eq 3

# Remote builds.

new_scenario
echo failed >"$dest/control/build-ppa-noble"
publish --only ppa
check "a failed build: exit $rc, not 1" test "$rc" -eq 1
check "a failed build: the reason is missing" said "the build failed on noble"
check "a failed build: it does not say what to do" said "bump its revision"
check "a failed build: the summary says it" said "^  ppa +failed: the build failed on noble"
publish --only ppa
check "a failed build, rerun: exit $rc, not 1" test "$rc" -eq 1
check "a failed build, rerun: uploaded again" test "$(upload_count ' ppa ')" -eq 4
check "a failed build, rerun: the reason is missing" said "already failed to build on noble"

# Only success counts: a target left behind holds the destination back.
new_scenario
echo building >"$dest/control/build-copr-fedora-44"
BUILD_LIMIT=0 publish --only copr
check "a timeout: exit $rc, not 1" test "$rc" -eq 1
check "a timeout: the reason is missing" said "timed out after 0s waiting on fedora-44"
check "a timeout: it doesn't promise no second upload" said "nothing is uploaded again"
rm "$dest/control/build-copr-fedora-44"
echo building >"$dest/control/build-copr"
BUILD_LIMIT=0 publish --only copr
check "a timeout, rerun: exit $rc, not 1" test "$rc" -eq 1
check "a timeout, rerun: uploaded again" test "$(upload_count ' copr ')" -eq 1
rm "$dest/control/build-copr"
publish --only copr
check "a timeout, resumed: exit $rc, not 0" test "$rc" -eq 0
check "a timeout, resumed: uploaded again" test "$(upload_count ' copr ')" -eq 1
check "a timeout, resumed: the summary is wrong" said "^  copr +already published"

# Launchpad shows nothing until it has processed an upload, so only this
# machine knows it went. A rerun must not send it twice.
new_scenario
echo hidden >"$dest/control/build-ppa"
BUILD_LIMIT=0 publish --only ppa
check "an unseen upload: exit $rc, not 1" test "$rc" -eq 1
check "an unseen upload: it doesn't mention the mail" said "only by mail"
BUILD_LIMIT=0 publish --only ppa
check "an unseen upload, rerun: uploaded again: $(uploads)" test "$(upload_count ' ppa ')" -eq 4

# A wait that ends well, polled at the default pace.
new_scenario
printf 'building\nbuilding\nbuilt\n' >"$dest/control/build-obs"
publish --only obs
check "a slow build: exit $rc, not 0: $(tail -n 3 "$out")" test "$rc" -eq 0
check "a slow build: it did not say what it's waiting on" said "waiting on openSUSE_Tumbleweed"

new_scenario
printf 'building\nbuilt\n' >"$dest/control/build-copr"
out=$scenario/output
rc=0
env PATH="$stubs:$PATH" CARGO_DENY="$stubs/cargo-deny" VERSION_UNDER_TEST="$version" \
    WINEO_PUBLISH_WORK="$scenario/work" "$repo/scripts/publish-packages.sh" "$version" \
    --only copr --dest-dir "$dest" >"$out" 2>&1 || rc=$?
check "the default pace: exit $rc, not 0" test "$rc" -eq 0
check "the default pace is not five minutes: $(sort -u "$STUB/sleep.log" 2>/dev/null | tr '\n' ' ')" \
    test "$(sort -u "$STUB/sleep.log" 2>/dev/null)" = 300
statuses=$(grep -c '' "$STUB/sleep.log" 2>/dev/null || true)
check "a poll came without a full interval before it ($statuses sleeps)" test "$statuses" -ge 2

# A revision bump goes out under the same tag.

new_scenario
publish
check "before the bump: exit $rc, not 0" test "$rc" -eq 0
sed -i 's/^pkgrel=1$/pkgrel=2/' "$repo/packaging/aur/wineo/PKGBUILD" "$repo/packaging/aur/wineo-bin/PKGBUILD" "$repo/PKGBUILD"
sed -i -E 's/^Release:( +)1%/Release:\12%/' "$repo/packaging/rpm/wineo.spec"
sed -i -E '1 s/-1ppa1\)/-1ppa2)/' "$repo/packaging/debian/changelog"
sed -i 's/^# Release revision: 0$/# Release revision: 1/' "$repo/packaging/gentoo/games-util/wineo/wineo-9999.ebuild"
reviewed_commit "Bump the package revisions"
: >"$dest/uploads.log"
publish
check "a bump: exit $rc, not 0: $(tail -n 5 "$out")" test "$rc" -eq 0
check "a bump: not every destination went: $(uploads)" test "$(upload_count)" -eq 10
check "a bump: the wrong identities: $(uploads)" \
    test "$(uploads | grep -c -e "overlay overlay $version-r1$" -e "aur .* $version-2$" -e "copr all $version-2$" -e "obs all $version-2$" -e "ppa .* $version-1ppa2~")" -eq 10
overlay_bare=$dest/remote/overlay.git
check "a bump: no -r1 ebuild" show "$overlay_bare" "games-util/wineo/wineo-$version-r1.ebuild"
check "a bump: the first ebuild is gone" show "$overlay_bare" "games-util/wineo/wineo-$version.ebuild"
changes=$dest/uploads/ppa/jammy/$version-1ppa2~jammy1/wineo_$version-1ppa2~jammy1_source.changes
check "a bump: jammy's version is wrong" grep -q "^wineo ($version-1ppa2~jammy1) jammy;" "$changes"
check "a bump: the bump went into the unpacked packaging/debian" grep -q 'packaging/debian untouched' "$changes"
check "a bump: the source isn't the tag's" \
    test "$(ls "$dest/uploads/obs/all/")" = "$version-1
$version-2"
check "a bump: the SRPM isn't the new one" test -f "$dest/uploads/copr/all/$version-2/wineo-$version-2.src.rpm"

# HEAD's lock can move past the tag; the ebuild's must not.
new_scenario
echo '# moved' >>"$repo/Cargo.lock"
reviewed_commit "Update dependencies"
publish --only overlay
check "a moved lock: exit $rc, not 0" test "$rc" -eq 0
check "a moved lock: pycargoebuild saw no lock" test -f "$STUB/lock-seen"
check "a moved lock: the crate list came from HEAD, not the tag" \
    test "$(grep -c '# moved' "$STUB/lock-seen" || true)" -eq 0

# --only and --skip.

new_scenario
publish --only obs
check "--only: exit $rc, not 0" test "$rc" -eq 0
check "--only went elsewhere: $(uploads)" test "$(uploads)" = "upload obs all $version-1"
check "--only: the summary doesn't say the rest didn't run" said "^  ppa +not run \(--only obs\)"

new_scenario
publish --skip aur
check "--skip aur: exit $rc, not 0" test "$rc" -eq 0
check "--skip aur reached the AUR: $(uploads)" test "$(upload_count ' aur ')" -eq 0
check "--skip aur: it needed makepkg's repositories" test ! -e "$dest/remote/wineo.git"
check "--skip aur: the summary is wrong" said "^  aur +not run \(--skip\)"
check "--skip aur: the rest didn't go" test "$(upload_count)" -eq 7
publish --only aur
check "--only aur after: exit $rc, not 0" test "$rc" -eq 0
check "--only aur after: it didn't go: $(uploads)" test "$(upload_count ' aur ')" -eq 3
check "--only aur after: others went again" test "$(upload_count)" -eq 10

new_scenario
publish --skip aur --skip ppa
check "two --skips: exit $rc, not 0" test "$rc" -eq 0
check "two --skips went to the AUR or PPA: $(uploads)" \
    test "$(upload_count ' aur ')" -eq 0 -a "$(upload_count ' ppa ')" -eq 0

# Recipes that can't go out.

new_scenario
sed -i "s/^sha256sums=('SKIP')$/sha256sums=('SKIP' 'SKIP')/" "$repo/packaging/aur/wineo-bin/PKGBUILD"
reviewed_commit "two sources"
publish
check "SKIP left: exit $rc, not 1" test "$rc" -eq 1
check "SKIP left: the reason is missing" said "wineo-bin still has a SKIP checksum"
check "SKIP left: part of the AUR went: $(uploads)" test "$(upload_count ' aur ')" -eq 0
check "SKIP left: the later ones stopped" test "$(upload_count ' copr ')" -eq 1 -a "$(upload_count ' ppa ')" -eq 4
check "SKIP left: the summary doesn't say it" said "^  aur +failed: wineo-bin still has a SKIP"

new_scenario
touch "$STUB/no-tag-tarball"
publish --only aur
check "no tag tarball: exit $rc, not 1" test "$rc" -eq 1
check "no tag tarball: the reason is missing" said "could not download GitHub's tarball"
check "no tag tarball: it published anyway: $(uploads)" test "$(upload_count)" -eq 0

new_scenario
echo 'pkgcheck: an error' >"$STUB/fail-tool-pkgcheck"
publish --only overlay
check "a pkgcheck error: exit $rc, not 1" test "$rc" -eq 1
check "a pkgcheck error: the reason is missing" said "ebuild manifest, or pkgcheck failed"
check "a pkgcheck error: it pushed anyway" test -z "$(git -C "$dest/remote/overlay.git" for-each-ref)"
check "a pkgcheck error: the error is not shown" said 'pkgcheck: an error'

# A manifest of something else isn't the tag's tarball.
new_scenario
touch "$STUB/stale-manifest"
publish --only overlay
check "a stale Manifest: exit $rc, not 1" test "$rc" -eq 1
check "a stale Manifest: the reason is missing" said "Manifest doesn't hold GitHub's tarball"
check "a stale Manifest: it pushed anyway" test -z "$(git -C "$dest/remote/overlay.git" for-each-ref)"

# Without --dest-dir: what stops a destination before it can upload. These are
# the real seams, with no service behind them.

new_scenario
publish_real --only copr
check "no COPR token: exit $rc, not 1" test "$rc" -eq 1
check "no COPR token: it does not say where to get one" \
    said "^  copr +failed: .*copr is missing: get an API token from https://copr.fedorainfracloud.org/api/"
check "no COPR token: it started a container" test "$(grep -c '^run' "$STUB/podman.log" 2>/dev/null || true)" -eq 0

mkdir -p "$XDG_CONFIG_HOME"
echo token >"$XDG_CONFIG_HOME/copr"
chmod 644 "$XDG_CONFIG_HOME/copr"
new_scenario
publish_real --only copr
check "a loose COPR token: exit $rc, not 1" test "$rc" -eq 1
check "a loose COPR token: it does not name the mode" said "has mode 644: run chmod 600"

chmod 600 "$XDG_CONFIG_HOME/copr"
new_scenario
# copr-cli's own words, from a local server that refused its login.
printf '%s\n' '' "Error: Operation requires API authentication. See the 'AUTHENTICATION' section in man copr-cli(1)." \
    '' 'Error: Login invalid/expired. Please visit https://copr.example/api to get or renew your API token.' \
    >"$STUB/fail-tool-copr-cli"
publish_real --only copr
check "an expired token: exit $rc, not 1" test "$rc" -eq 1
check "an expired token: it does not say so" said "COPR tokens expire: get a new one from https://copr.fedorainfracloud.org/api/"
check "the token isn't mounted read-only" grep -q "$XDG_CONFIG_HOME/copr:/root/.config/copr:ro" "$STUB/podman.log"

# A state that can't be read must never count as absent.
new_scenario
touch "$STUB/copr-down-0"
publish_real --only copr
check "COPR down: exit $rc, not 1" test "$rc" -eq 1
check "COPR down: the reason is missing" said "could not read the build state"
check "COPR down: it went on to upload" test "$(grep -c 'copr-cli' "$STUB/podman.log" 2>/dev/null || true)" -eq 0

new_scenario
publish_real --only obs
check "no oscrc: exit $rc, not 1" test "$rc" -eq 1
check "no oscrc: it does not say what to do" said "oscrc is missing: create it"
check "no oscrc: it started a container" test "$(grep -c '^run' "$STUB/podman.log" || true)" -eq 0

# A status that prints nothing at all is not "all built".
new_scenario
# shellcheck disable=SC2016
sed -i 's/^    if \[\[ -n \$dest_dir \]\] && has_remote_build "\$1"; then$/    if [[ -n ${EMPTY_STATUS:-} ]]; then return 0; fi\n&/' "$repo/scripts/publish-packages.sh"
reviewed_commit "a status with no lines"
EMPTY_STATUS=1 publish --only copr
check "an empty status: exit $rc, not 1" test "$rc" -eq 1
check "an empty status: the reason is missing" said "could not read the build state"
check "an empty status: it published: $(uploads)" test -z "$(uploads)"

# oscrc is the maintainer's own login: read-only mounts, and nothing else.
mkdir -p "$XDG_CONFIG_HOME/osc"
echo secret >"$XDG_CONFIG_HOME/osc/oscrc"
new_scenario
# osc's own words when the connection fails, which isn't a 404.
echo "Failed to establish a new connection: [Errno 111] Connection refused" >"$STUB/fail-tool-osc"
publish_real --only obs
check "OBS status: it isn't refused when unknown" said "could not read the build state"
check "OBS status: it went on to upload" test "$(grep -c 'STEP=obs-upload' "$STUB/podman.log" || true)" -eq 0
check "oscrc is mounted without :ro" test "$(grep -F oscrc "$STUB/podman.log" | grep -cv 'oscrc:/root/.config/osc/oscrc:ro')" -eq 0
check "oscrc reached a work directory" test "$(grep -rl secret "$scenario/work" 2>/dev/null | grep -c . || true)" -eq 0
rm -rf "$XDG_CONFIG_HOME/osc"

new_scenario
touch "$STUB/no-key"
publish_real --only ppa
check "no signing key: exit $rc, not 1" test "$rc" -eq 1
check "no signing key: it does not say so" said "gpg has no key 8EE69509"
check "no signing key: the status needed a login" test "$(grep -c 'api.launchpad.net' "$STUB/curl.log")" -eq 4
# The stub answers any query, so the query itself is compared here: a wrong
# identity would read as absent and upload again.
for series in jammy noble resolute stonking; do
    check "the Launchpad query for $series is not the exact one" grep -qxF -- \
        "-fsS -G --max-time 60 https://api.launchpad.net/1.0/~meandrous-shark/+archive/ubuntu/wineo --data-urlencode ws.op=getPublishedSources --data-urlencode source_name=wineo --data-urlencode exact_match=true --data-urlencode version=$version-1ppa1~${series}1 --data-urlencode distro_series=https://api.launchpad.net/1.0/ubuntu/$series" \
        "$STUB/curl.log"
done

# OBS through its real branch, with the stub answering only the exact endpoints.
mkdir -p "$XDG_CONFIG_HOME/osc"
echo secret >"$XDG_CONFIG_HOME/osc/oscrc"
export OBS_VERSION=$version
obs_upload_calls() { grep -cx 'osc' "$STUB/tools.log" || true; }
committed() { called osc commit -m "Update to $version-1"; }

new_scenario
obs_dir "$STUB/obs" 1 false succeeded abc succeeded
publish_real --only obs
check "OBS published: exit $rc, not 0: $(tail -n 3 "$out")" test "$rc" -eq 0
check "OBS published: not skipped" said "^  obs +already published \($version-1\)"
check "OBS published: it read another endpoint" \
    called osc api '/build/home:meandrousshark/_result?package=wineo&arch=x86_64'
for target in openSUSE_Tumbleweed 16.0 15.6; do
    check "OBS published: job history of $target wasn't read" \
        called osc api "/build/home:meandrousshark/$target/x86_64/_jobhistory?package=wineo&limit=5"
done
if committed; then fail "OBS published: it committed again"; else pass; fi

new_scenario
obs_dir "$STUB/obs" 1 false succeeded abc succeeded publishing
BUILD_LIMIT=0 publish_real --only obs
check "OBS publishing: exit $rc, not 1" test "$rc" -eq 1
check "OBS publishing: it did not wait" said "timed out after 0s waiting on openSUSE_Tumbleweed\(building\)"
if committed; then fail "OBS publishing: it committed again"; else pass; fi

new_scenario
obs_dir "$STUB/obs" 1 false failed abc failed
publish_real --only obs
check "OBS failed: exit $rc, not 1" test "$rc" -eq 1
check "OBS failed: the reason is missing" said "already failed to build on openSUSE_Tumbleweed 16.0 15.6"
if committed; then fail "OBS failed: it committed again"; else pass; fi

# An older revision on the server: upload, wait, and the new one builds.
new_scenario
obs_dir "$STUB/obs" 0 false succeeded abc succeeded
obs_dir "$STUB/obs-after-commit" 1 false succeeded abc succeeded
publish_real --only obs
check "OBS old revision: exit $rc, not 0: $(tail -n 3 "$out")" test "$rc" -eq 0
check "OBS old revision: no commit" committed
check "OBS old revision: the summary is wrong" said "^  obs +published \($version-1\)"

# The source answered, the rest didn't: unknown, and no upload.
for gone in result jobhistory-16.0; do
    new_scenario
    obs_dir "$STUB/obs" 1 false succeeded abc succeeded
    rm "$STUB/obs/$gone"
    publish_real --only obs
    check "OBS without $gone: exit $rc, not 1" test "$rc" -eq 1
    check "OBS without $gone: the reason is missing" said "could not read the build state"
    if committed; then fail "OBS without $gone: it uploaded"; else pass; fi
done

# A garbled listing or spec is unreadable. Only a 404 for them says absent.
new_scenario
obs_dir "$STUB/obs" 1 false succeeded abc succeeded
echo '<not xml' >"$STUB/obs/directory"
publish_real --only obs
check "OBS with a garbled listing: exit $rc, not 1" test "$rc" -eq 1
if committed; then fail "OBS with a garbled listing: it uploaded"; else pass; fi
new_scenario
obs_dir "$STUB/obs" 1 false succeeded abc succeeded
: >"$STUB/obs/spec"
publish_real --only obs
check "OBS with an empty spec: exit $rc, not 1" test "$rc" -eq 1
if committed; then fail "OBS with an empty spec: it uploaded"; else pass; fi
new_scenario
obs_dir "$STUB/obs" 1 false succeeded abc succeeded
: >"$STUB/obs/spec"
rm "$STUB/obs/result"
publish_real --only obs
check "OBS with an empty spec and no result: exit $rc, not 1" test "$rc" -eq 1
if committed; then fail "OBS with an empty spec and no result: it uploaded"; else pass; fi
unset OBS_VERSION
rm -rf "$XDG_CONFIG_HOME/osc"

# Launchpad through its real branch: anything but a clean empty answer stops it.
for answer in '{}' '{"entries":[{}],"total_size":1,"start":0}' '{"entries":[]}' \
    '{"total_size":0}' '{"entries":[],"total_size":1,"start":0}' '<html>'; do
    new_scenario
    echo "$answer" >"$STUB/lp-sources.json"
    publish_real --only ppa
    check "Launchpad answered '$answer': exit $rc, not 1" test "$rc" -eq 1
    check "Launchpad answered '$answer': the reason is missing" said "could not read the build state"
    check "Launchpad answered '$answer': it signed or uploaded" \
        test "$(grep -c -e '^debsign$' -e '^dput$' "$STUB/tools.log" || true)" -eq 0
done

# COPR found the build, and its chroots can't be read.
for answer in '{"items":[]}' '{}' ''; do
    new_scenario
    jq -n --arg version "$version-1" \
        '{items: [{id: 5, state: "pending", source_package: {version: $version}}],
          meta: {offset: 0, limit: 100}}' >"$STUB/copr-page-0.json"
    echo "$answer" >"$STUB/copr-chroots.json"
    mkdir -p "$XDG_CONFIG_HOME"
    publish_real --only copr
    check "chroots answered '$answer': exit $rc, not 1" test "$rc" -eq 1
    check "chroots answered '$answer': the reason is missing" said "could not read the build state"
    check "chroots answered '$answer': it uploaded" \
        test "$(grep -c '^copr-cli$' "$STUB/tools.log" || true)" -eq 0
done

# Repositories that hold something, but not what they should.
new_scenario
publish --only overlay
git -C "$dest/remote/overlay.git" symbolic-ref HEAD refs/heads/nowhere
: >"$dest/uploads.log"
publish --only overlay
check "an overlay clone with no checkout: exit $rc, not 1" test "$rc" -eq 1
check "an overlay clone with no checkout: it pushed: $(uploads)" test -z "$(uploads)"
check "an overlay clone with no checkout: the reason is missing" said "could not read MeandrousShark/wineo-overlay"

new_scenario
publish --only aur
aur_clone=$scenario/aur-clone
git clone -q "$dest/remote/wineo-bin.git" "$aur_clone"
git -C "$aur_clone" rm -q .SRCINFO
git -C "$aur_clone" commit -qm "lose it"
git -C "$aur_clone" push -q origin HEAD:master
: >"$dest/uploads.log"
publish --only aur
check "an AUR repository with no .SRCINFO: exit $rc, not 1" test "$rc" -eq 1
check "an AUR repository with no .SRCINFO: it pushed: $(uploads)" test -z "$(uploads)"

# Pins: every value in the published copy has to be the file's own sum.

wrong_sum=$(printf 'a%.0s' {1..64})
pin_refused() { # NAME REASON: the AUR went nowhere, and the rest went on
    local name=$1 reason=$2
    publish
    check "$name: exit $rc, not 1" test "$rc" -eq 1
    check "$name: no reason matching '$reason': $(grep -m1 '^  aur' "$out")" said "$reason"
    check "$name: part of the AUR went: $(uploads)" test "$(upload_count ' aur ')" -eq 0
    check "$name: nothing reached the AUR's repositories" \
        test -z "$(git -C "$dest/remote/wineo-bin.git" for-each-ref 2>/dev/null)"
    check "$name: the later ones stopped" test "$(upload_count ' ppa ')" -eq 4
}
for package in wineo wineo-bin; do
    new_scenario
    sed -i "s/^sha256sums=('SKIP')$/sha256sums=('$wrong_sum')/" "$repo/packaging/aur/$package/PKGBUILD"
    reviewed_commit "a wrong pin"
    pin_refused "a wrong committed pin in $package" "$package has a committed checksum that doesn't match"
done

new_scenario
python3 - "$repo/packaging/aur/wineo-bin/PKGBUILD" "$wrong_sum" <<'EOF'
import sys
path, wrong = sys.argv[1:]
text = open(path).read().replace("sha256sums=('SKIP')", f"sha256sums=(\n  '{wrong}'\n)")
open(path, "w").write(text)
EOF
reviewed_commit "a wrong pin over several lines"
pin_refused "a wrong pin over several lines" "wineo-bin has a committed checksum that doesn't match"

new_scenario
echo "b2sums=('$wrong_sum')" >>"$repo/packaging/aur/wineo-bin/PKGBUILD"
reviewed_commit "a second kind of sum"
pin_refused "an unchecked kind of sum" "wineo-bin has a committed checksum that doesn't match"

new_scenario
sed -i 's#^source=.*#source=("https://example.invalid/other.tar.gz")#' "$repo/packaging/aur/wineo-bin/PKGBUILD"
reviewed_commit "another source"
pin_refused "another source" "wineo-bin names the source"

new_scenario
sed -i "s/^sha256sums=('SKIP')$/sha256sums=('$wrong_sum')/" "$repo/PKGBUILD"
reviewed_commit "a pin on a git source"
pin_refused "a pin in wineo-git" "wineo-git must keep SKIP"

# A correct committed pin is fine.
new_scenario
good_sum=$(sha256sum "$STUB/assets/wineo-$version-x86_64-linux.tar.gz" | awk '{print $1}')
sed -i "s/^sha256sums=('SKIP')$/sha256sums=('$good_sum')/" "$repo/packaging/aur/wineo-bin/PKGBUILD"
reviewed_commit "a right pin"
publish --only aur
check "a right committed pin: exit $rc, not 0: $(tail -n 3 "$out")" test "$rc" -eq 0

# A tool image that can't be built stops the destination.

for row in "dnf copr" "apt-get ppa"; do
    read -r tool d <<<"$row"
    new_scenario
    echo "$tool: no such package" >"$STUB/fail-tool-$tool"
    publish --only "$d"
    check "a failed $tool: exit $rc, not 1" test "$rc" -eq 1
    check "a failed $tool: the reason is missing" said "^  $d +failed: installing the tools into a .* image failed"
    check "a failed $tool: nothing went: $(uploads)" test -z "$(uploads)"
    check "a failed $tool: an image was kept" test "$(grep -c '^commit' "$STUB/podman.log" || true)" -eq 0
    check "a failed $tool: the next step ran" \
        test "$(grep -c -e '^rpmbuild$' -e '^dpkg-buildpackage$' "$STUB/tools.log" || true)" -eq 0
done

new_scenario
touch "$STUB/fail-commit"
publish --only copr
check "a failed commit: exit $rc, not 1" test "$rc" -eq 1
check "a failed commit: the reason is missing" said "^  copr +failed: installing the tools"
check "a failed commit: a half-built image is left" \
    test "$(find "$STUB" -name 'image-*' | grep -c . || true)" -eq 0
check "a failed commit: the builder is left" grep -q '^rm -f wineo-publish-builder-fedora' "$STUB/podman.log"

# osc crashes at startup without git, which no help output or stub would show.
new_scenario
publish --only copr
check "the Fedora tools changed" called dnf -y install rpm-build copr-cli osc git-core
new_scenario
publish --only ppa
check "the Ubuntu tools changed" called apt-get install -y --no-install-recommends \
    ca-certificates debhelper devscripts dpkg-dev dput fakeroot gnupg xz-utils

check "the Ubuntu image was not reused across series" \
    test "$(grep -c '^commit' "$STUB/podman.log")" -eq 1

# A failure inside $(...) has to stop the function it's in, as tool_image and
# tag_tarball rely on. Bash doesn't do that by default.
rc=0
reached=$(bash -c 'source "$1"
    inner() { false; echo reached; }
    got=$(inner)
    echo "$got"' bash "$repo/scripts/publish-packages.sh" 2>&1) || rc=$?
check "a failure in \$(...) did not stop it: $reached" test "$rc" -ne 0 -a -z "$reached"

# COPR's list, past the first page.

copr_pages() { # OFFSET VERSION: a full page of one version from OFFSET
    jq -n --argjson offset "$1" --arg version "$2" \
        '{items: [range(100) | {id: (100000 - $offset - .), state: "succeeded", source_package: {version: $version}}],
          meta: {offset: $offset, limit: 100}}'
}
new_scenario
mkdir -p "$XDG_CONFIG_HOME/osc"
copr_pages 0 0.0.1-1 >"$STUB/copr-page-0.json"
copr_pages 100 "$version-1" >"$STUB/copr-page-100.json"
cp "$root/scripts/testdata/publish/copr-chroots.json" "$STUB/copr-chroots.json"
publish_real --only copr
check "a build on page two: exit $rc, not 0: $(tail -n 3 "$out")" test "$rc" -eq 0
check "a build on page two was not found" said "^  copr +already published"
check "a build on page two: it read a third page" \
    test "$(grep -c 'build/list' "$STUB/curl.log")" -eq 2
check "the COPR query is not the exact one" grep -qF \
    'https://copr.fedorainfracloud.org/api_3/build/list?ownername=meandrousshark&projectname=wineo&limit=100&offset=0&order=id&order_type=DESC' \
    "$STUB/curl.log"
check "a build on page two: it uploaded" test "$(grep -c '^copr-cli$' "$STUB/tools.log" || true)" -eq 0

for broken in failed garbled short; do
    new_scenario
    copr_pages 0 0.0.1-1 >"$STUB/copr-page-0.json"
    case $broken in
        failed) touch "$STUB/copr-down-100" ;;
        garbled) echo '<html>' >"$STUB/copr-page-100.json" ;;
        short) echo '{"items":[],"meta":{"offset":0,"limit":100}}' >"$STUB/copr-page-100.json" ;;
    esac
    publish_real --only copr
    check "a $broken second page: exit $rc, not 1" test "$rc" -eq 1
    check "a $broken second page: the reason is missing" said "could not read the build state"
    check "a $broken second page read as absent" \
        test "$(grep -c '^copr-cli$' "$STUB/tools.log" || true)" -eq 0
done

# The real upload branches, with the exact commands each container ran.

new_scenario
BUILD_LIMIT=0 publish_real --only copr
check "copr-cli ran differently" called copr-cli build --nowait --chroot fedora-43-x86_64 \
    --chroot fedora-44-x86_64 --chroot fedora-45-x86_64 meandrousshark/wineo \
    "/work/wineo-$version-1.src.rpm"

mkdir -p "$XDG_CONFIG_HOME/osc"
echo secret >"$XDG_CONFIG_HOME/osc/oscrc"
new_scenario
BUILD_LIMIT=0 publish_real --only obs
check "osc checkout ran differently" called osc checkout home:meandrousshark wineo
check "osc addremove ran differently" called osc addremove
check "osc commit ran differently" called osc commit -m "Update to $version-1"
check "OBS did not get exactly the spec and the tarball: $(cat "$STUB/osc-tree" 2>/dev/null)" \
    test "$(tr '\n' ' ' <"$STUB/osc-tree")" = "wineo-$version-vendored.tar.xz wineo.spec "
check "the OBS status read something else" called osc api /source/home:meandrousshark/wineo
check "the upload's oscrc isn't read-only" \
    test "$(grep -F 'STEP=obs-upload' "$STUB/podman.log" | grep -c 'oscrc:/root/.config/osc/oscrc:ro')" -eq 1
rm -rf "$XDG_CONFIG_HOME/osc"

new_scenario
BUILD_LIMIT=0 publish_real --only ppa
for series in jammy noble resolute stonking; do
    changes=wineo_$version-1ppa1~${series}1_source.changes
    check "debsign ran differently for $series" called debsign --no-conf -k8EE69509 "$changes"
    check "dput ran differently for $series" called dput ppa:meandrous-shark/wineo "$changes"
done
check "dput ran before debsign" \
    test "$(grep -E '^(debsign|dput)$' "$STUB/tools.log" | sed -n 1,2p | tr '\n' ' ')" = "debsign dput "
check "the public key was not imported" called gpg --batch --quiet --import /signing-key.asc
check "the agent socket isn't mounted" grep -qF "$STUB/S.gpg-agent:/root/.gnupg/S.gpg-agent" "$STUB/podman.log"

# Real URLs, through insteadOf, into local repositories.
new_scenario
remotes=$scenario/remotes
mkdir -p "$remotes/github/MeandrousShark" "$remotes/aur"
git init -q --bare --initial-branch=main "$remotes/github/MeandrousShark/wineo-overlay.git"
for package in wineo wineo-bin wineo-git; do
    git init -q --bare --initial-branch=master "$remotes/aur/$package.git"
done
git config --global "url.$remotes/github/.insteadOf" https://github.com/
git config --global "url.$remotes/aur/.insteadOf" ssh://aur@aur.archlinux.org/
publish_real --only overlay
check "the real overlay: exit $rc, not 0: $(tail -n 3 "$out")" test "$rc" -eq 0
check "the real overlay did not reach main" \
    git -C "$remotes/github/MeandrousShark/wineo-overlay.git" rev-parse -q --verify refs/heads/main
publish_real --only aur
check "the real AUR: exit $rc, not 0: $(tail -n 3 "$out")" test "$rc" -eq 0
for package in wineo wineo-bin wineo-git; do
    check "the real $package did not reach master" \
        git -C "$remotes/aur/$package.git" rev-parse -q --verify refs/heads/master
done
publish_real --only aur
check "a real AUR rerun did not skip" said "^  aur +already published"
git config --global --remove-section "url.$remotes/github/"
git config --global --remove-section "url.$remotes/aur/"

# The parsers, on canned answers. Run in the script's own functions.

testdata=$root/scripts/testdata/publish
script=$repo/scripts/publish-packages.sh
pure() { (
    set +eu
    # shellcheck disable=SC1090
    source "$script"
    "$@"
); }

# The names in release.yml's `gh release create`, with its own variables.
listed=$(awk '/gh release create/ { on = 1 } on && /dist\// { print } on && !/\\$/ { if (seen++) on = 0 }' \
    "$root/.github/workflows/release.yml" |
    sed -E 's/^ *"?dist\///; s/"? *\\?$//; s/\$\{?version\}?/'"$version"'/g' | sort)
check "the asset names differ from release.yml: $listed" \
    test "$listed" = "$(pure expected_assets "$version" | sort)"
# shellcheck disable=SC2016
check "SHA256SUMS' list differs from release.yml's check" \
    grep -qF '"wineo-${version}-1.x86_64.rpm"' "$root/.github/workflows/release.yml"

for row in "1.91 1.90 y" "1.90 1.90 y" "1.90.0 1.90 y" "1.89 1.90 n" "1.9 1.90 n" \
    "1.100 1.99 y" "1.99.1 1.99.2 n"; do
    read -r a b want <<<"$row"
    if pure version_at_least "$a" "$b"; then got=y; else got=n; fi
    check "version_at_least $a $b is $got" test "$got" = "$want"
done
check "a floor of 1.90 refuses something" test -z "$(pure targets_below_floor 1.90)"
check "a floor of 1.92 doesn't refuse exactly jammy and noble" \
    test "$(pure targets_below_floor 1.92)" = "ppa jammy 1.91
ppa noble 1.91"
check "a floor of 1.98 refuses the wrong ones" \
    test "$(pure targets_below_floor 1.98 | awk '{print $2}' | tr '\n' ' ')" = "jammy noble resolute stonking 15.6 "
check "the targets aren't all there" \
    test "$(pure targets_of ppa | tr '\n' ' ')$(pure targets_of copr | tr '\n' ' ')$(pure targets_of obs | tr '\n' ' ')" = \
    "jammy noble resolute stonking fedora-43 fedora-44 fedora-45 openSUSE_Tumbleweed 16.0 15.6 "

check "tag_run picks the wrong run" test "$(pure tag_run abc <<<'[
    {"databaseId":1,"headSha":"abc","conclusion":"failure"},
    {"databaseId":3,"headSha":"zzz","conclusion":"failure"},
    {"databaseId":2,"headSha":"abc","conclusion":"success"}]' | jq -r .databaseId)" = 2

chroots='{"items":[
    {"name":"fedora-43-x86_64","state":"succeeded"},
    {"name":"fedora-44-x86_64","state":"running"},
    {"name":"fedora-45-x86_64","state":"failed"}]}'
check "copr_states reads it wrong" test "$(pure copr_states <<<"$chroots" | tr '\n' ' ')" = \
    "fedora-43 built fedora-44 building fedora-45 failed "
check "copr_states misreads a real answer" \
    test "$(pure copr_states <"$testdata/copr-chroots.json" | tr '\n' ' ')" = \
    "fedora-43 built fedora-44 built fedora-45 built "
check "copr_states takes nothing for an answer" \
    test "$(pure copr_states </dev/null | head -n 1)" = "fedora-43 unknown"
real_offset=$(jq .meta.offset "$testdata/copr-builds.json")
real_id=$(jq -r '.items[1].id' "$testdata/copr-builds.json")
real_version=$(jq -r '.items[1].source_package.version' "$testdata/copr-builds.json")
check "copr_page misses a build in a real page" \
    test "$(pure copr_page "$(cat "$testdata/copr-builds.json")" "$real_offset" 3 "$real_version")" -ge "$real_id"
check "copr_page finds a version that isn't in a real page" \
    test -z "$(pure copr_page "$(cat "$testdata/copr-builds.json")" "$real_offset" 3 0.0.0-9)"
if pure copr_page "$(cat "$testdata/copr-builds.json")" 100 3 x >/dev/null; then
    fail "copr_page takes the wrong page for the one asked for"
else
    pass
fi
if pure copr_page '{"error":"down"}' 0 3 x >/dev/null; then
    fail "copr_page takes an error for an empty page"
else
    pass
fi
if pure copr_page '' 0 3 x >/dev/null; then fail "copr_page takes nothing for a page"; else pass; fi
check "copr_states takes a missing chroot for absent" \
    test "$(pure copr_states <<<'{"items":[]}' | head -n 1)" = "fedora-43 unknown"
check "copr_states takes a new state for an answer" \
    test "$(pure copr_states <<<'{"items":[{"name":"fedora-43-x86_64","state":"unheard-of"}]}' | head -n 1)" = "fedora-43 unknown"
if pure copr_page '{"items":[{"id":1}],"meta":{"offset":0,"limit":3}}' 0 3 x >/dev/null; then
    fail "copr_page takes a malformed build for a page"
else
    pass
fi
check "copr_states takes garbage for an answer" \
    test "$(pure copr_states <<<'<html>' | head -n 1)" = "fedora-43 unknown"

binary='{"start":0,"total_size":1,"entries":[{"status":"Published","distro_arch_series_link":"https://x/ubuntu/jammy/amd64"}]}'
pending='{"start":0,"total_size":1,"entries":[{"status":"Pending","distro_arch_series_link":"https://x/ubuntu/jammy/amd64"}]}'
wrong_arch='{"start":0,"total_size":1,"entries":[{"status":"Published","distro_arch_series_link":"https://x/ubuntu/jammy/amd64v3"}]}'
builds() { # one amd64 build in state $1, plus an armhf build that failed
    jq -nc --arg state "$1" '{start:0,total_size:2,entries:[{arch_tag:"armhf",buildstate:"Failed to build"},
        {arch_tag:"amd64",buildstate:$state}]}'
}
lp_case() { # WANT STATUS BUILDSTATE BINARIES
    local got
    got=$(pure lp_state "$2" "$(builds "$3")" "$4")
    check "lp_state '$2' '$3' is $got, not $1" test "$got" = "$1"
}
lp_case built Published 'Successfully built' "$binary"
lp_case built Superseded 'Successfully built' "$binary"
lp_case building Published 'Successfully built' "$pending"
lp_case building Published 'Successfully built' "$wrong_arch"
lp_case unknown Published 'Successfully built' ''
lp_case unknown Published 'A state nobody has seen' '{"entries":[],"total_size":0,"start":0}'
lp_case unknown 'Some new status' 'Successfully built' "$binary"
lp_case building Pending 'Needs building' '{"start":0,"total_size":0,"entries":[]}'
lp_case building Published 'Currently building' '{"start":0,"total_size":0,"entries":[]}'
lp_case building Published 'Uploading build' '{"start":0,"total_size":0,"entries":[]}'
lp_case failed Published 'Failed to build' '{"start":0,"total_size":0,"entries":[]}'
lp_case failed Published 'Dependency wait' '{"start":0,"total_size":0,"entries":[]}'
lp_case failed Published 'Chroot problem' '{"start":0,"total_size":0,"entries":[]}'
lp_case failed Published 'Failed to upload' '{"start":0,"total_size":0,"entries":[]}'
lp_case failed Published 'Cancelled build' '{"start":0,"total_size":0,"entries":[]}'
lp_case failed Deleted 'Successfully built' "$binary"
# Real answers: firefox's amd64 built, while armhf, riscv64, and s390x failed.
# lp-binaries.json is a real first page of 394, so as it stands it is partial.
whole_binaries=$(jq -c '.total_size = (.entries | length)' "$testdata/lp-binaries.json")
check "real Launchpad answers aren't built" test "$(pure lp_state \
    "$(jq -r '.entries[0].status' "$testdata/lp-sources.json")" \
    "$(cat "$testdata/lp-builds.json")" "$whole_binaries")" = built
check "a real partial page of binaries counted" test "$(pure lp_state Published \
    "$(cat "$testdata/lp-builds.json")" "$(cat "$testdata/lp-binaries.json")")" = unknown
real_version=$(jq -r '.entries[0].source_package_version' "$testdata/lp-sources.json")
lp_real() { pure lp_source "$(cat "$testdata/$1")" "$2" "$3" firefox; }
check "lp_source misreads a real source" \
    test "$(lp_real lp-sources.json "$real_version" stonking | cut -d' ' -f1)" = Published
check "lp_source misreads a real empty answer" \
    test "$(lp_real lp-sources-none.json 0.0-1 stonking)" = absent
for row in "lp-sources-partial.json $real_version stonking" "lp-sources.json 0.0-1 stonking" \
    "lp-sources.json $real_version jammy"; do
    read -r file id series <<<"$row"
    if lp_real "$file" "$id" "$series" >/dev/null; then
        fail "lp_source took $file for $id on $series"
    else
        pass
    fi
done
check "a real Launchpad answer lost its armhf failure" \
    grep -q 'Failed to build' "$testdata/lp-builds.json"
real_stonking=$(jq -c '.entries |= map(select(.arch_tag == "amd64") | .buildstate = "Failed to build")
    | .total_size = (.entries | length)' \
    "$testdata/lp-builds.json")
check "a real Launchpad build that failed on amd64 isn't failed" \
    test "$(pure lp_state Published "$real_stonking" "$whole_binaries")" = failed
check "lp_state takes no answer for one" test "$(pure lp_state Published '' '')" = unknown
check "lp_state counts another architecture" \
    test "$(pure lp_state Published '{"start":0,"total_size":1,"entries":[{"arch_tag":"armhf","buildstate":"Successfully built"}]}' "$binary")" = building

obs_case() { # WANT WHY RELEASE DIRTY CODE JOB_SRCMD5 JOB_CODE [REPO_STATE] [SRCMD5]
    local want=$1 why=$2 got
    shift 2
    obs_dir "$tmp/obs" "$@"
    got=$(pure obs_states "$tmp/obs" 1.2.3-1 | awk '{print $2}' | sort -u | tr '\n' ' ')
    check "obs_states $why is '$got', not $want" test "$got" = "$want "
}
obs_case built "a finished build" 1 false succeeded abc succeeded
obs_case built "an unchanged rebuild" 1 false succeeded abc unchanged
obs_case absent "another revision on the server" 2 false succeeded abc succeeded
obs_case building "a dirty repository" 1 true succeeded abc succeeded
obs_case building "the last revision's result" 1 false succeeded old succeeded
obs_case failed "a failed build" 1 false failed abc failed
obs_case failed "unresolvable" 1 false unresolvable abc succeeded
obs_case failed "excluded" 1 false excluded abc succeeded
obs_case building "scheduled" 1 false scheduled abc succeeded
obs_case unknown "a new package code" 1 false unheard-of abc succeeded
obs_case building "a repository still publishing" 1 false succeeded abc succeeded publishing
obs_case building "a repository still building" 1 false succeeded abc succeeded building
obs_case building "an unpublished repository" 1 false succeeded abc succeeded unpublished
obs_case building "a dirty repository that also publishes" 1 true succeeded abc succeeded publishing
obs_case building "a dirty attribute that isn't true" 1 1 succeeded abc succeeded
obs_case unknown "no hash on either side" 1 false succeeded '' succeeded published ''
obs_case unknown "no hash on the source" 1 false succeeded abc succeeded published ''
obs_case unknown "no hash on the job" 1 false succeeded '' succeeded
obs_dir "$tmp/obs" 1 false succeeded abc succeeded
rm "$tmp/obs/result"
check "obs_states takes a missing file for an answer" \
    test "$(pure obs_states "$tmp/obs" 1.2.3-1 | head -n 1)" = "openSUSE_Tumbleweed unknown"
touch "$tmp/obs/missing"
check "obs_states misses an absent package" \
    test "$(pure obs_states "$tmp/obs" 1.2.3-1 | head -n 1)" = "openSUSE_Tumbleweed absent"

# A repository's state: absent only for an empty one or another identity.
aur_repo=$tmp/aur-state
git init -q "$aur_repo"
aur_case() { # WANT WHY SRCINFO
    printf '%b' "$3" >"$aur_repo/.SRCINFO"
    git -C "$aur_repo" add .SRCINFO
    git -C "$aur_repo" commit -qm state --allow-empty
    local got
    got=$(pure aur_state "$aur_repo" wineo 1.2.3-1 2>/dev/null) || got=stopped
    check "aur_state $2 is $got, not $1" test "$got" = "$1"
}
aur_case built "the same identity" 'pkgbase = wineo\n\tpkgver = 1.2.3\n\tpkgrel = 1\n'
aur_case absent "another revision" 'pkgbase = wineo\n\tpkgver = 1.2.3\n\tpkgrel = 2\n'
aur_case unknown "another package" 'pkgbase = other\n\tpkgver = 1.2.3\n\tpkgrel = 2\n'
aur_case unknown "no pkgrel" 'pkgbase = wineo\n\tpkgver = 1.2.3\n'
aur_case unknown "two pkgvers" 'pkgbase = wineo\n\tpkgver = 1.2.3\n\tpkgver = 1.2.4\n\tpkgrel = 2\n'
empty_repo=$tmp/empty-state
git init -q "$empty_repo"
check "aur_state takes an empty repository for anything but absent" \
    test "$(pure aur_state "$empty_repo" wineo 1.2.3-1)" = absent
check "overlay_state takes an empty repository for anything but absent" \
    test "$(pure overlay_state "$empty_repo" 1.2.3)" = absent
check "overlay_state takes another repository for this one" \
    test "$(pure overlay_state "$aur_repo" 1.2.3)" = unknown

# The real clients' words, from a local server standing in for the service.
printf "Error: Operation requires API authentication. See the 'AUTHENTICATION' section in man copr-cli(1).\n" >"$tmp/said"
check "copr_auth_failed misses copr-cli's refusal" pure copr_auth_failed "$tmp/said"
printf 'Error: Login invalid/expired. Please visit https://copr.example/api to get or renew your API token.\n' >"$tmp/said"
check "copr_auth_failed misses an expired login" pure copr_auth_failed "$tmp/said"
printf 'Error: the build was refused\n' >"$tmp/said"
if pure copr_auth_failed "$tmp/said"; then fail "copr_auth_failed fires on anything"; else pass; fi
printf 'Server returned an error: HTTP Error 404: Not Found\nPackage not found: x\n' >"$tmp/said"
check "osc_said_missing misses osc's 404" pure osc_said_missing "$tmp/said"
for text in 'Server returned an error: HTTP Error 403: Forbidden' \
    "HTTPConnection(host='127.0.0.1', port=8998): Failed to establish a new connection: [Errno 111] Connection refused"; do
    echo "$text" >"$tmp/said"
    if pure osc_said_missing "$tmp/said"; then fail "osc_said_missing fires on: $text"; else pass; fi
done

# shellcheck disable=SC2016
check "the polling is not five minutes by default" \
    grep -qxF 'build_interval=${WINEO_PUBLISH_BUILD_INTERVAL:-300}' "$root/scripts/publish-packages.sh"
if grep -qE 'osc results.*(-w|--watch)|--watch' "$root/scripts/publish-packages.sh"; then
    fail "the script watches OBS"
else
    pass
fi

echo "$((cases - failures)) of $cases cases behaved correctly"
[[ $failures -eq 0 ]]
