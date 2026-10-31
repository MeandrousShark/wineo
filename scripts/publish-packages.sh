#!/bin/bash
# Publish a tagged release to the package repositories, from the maintainer's
# machine, with the maintainer's own keys. CI holds no secrets for these.
#
# Usage: scripts/publish-packages.sh VERSION [--only DEST] [--skip DEST]...
#                                    [--dest-dir DIR]
#
# DEST is overlay, aur, copr, obs, or ppa, published in that order. --only and
# --skip can't be combined. A release with no AUR account runs --skip aur, and
# the recipes go up later with --only aur.
#
# It waits for the mirror to carry the tag to GitHub, for the tag's release
# run, and for the Release. Then, before anything is published, it checks that
# the Release isn't a draft, that every asset is attached and matches
# SHA256SUMS, that the local tag is GitHub's, that the crate licenses pass, and
# that the Rust floor still allows every target in the table below.
#
# What goes out:
#   recipes  from this checkout's committed HEAD, which must be clean, hold the
#            tag, and be on GitHub's main, so a reviewed packaging fix can bump
#            a revision and go out under the same tag. Versions and checksums
#            are only ever filled in in temporary copies.
#   source   from the Release (or the tag, where the recipe says so), never
#            from the checkout. HEAD's scripts/ reach a package only through
#            the tarball, so a fix there needs a new release.
#
# Each destination's identity is the version plus the revision in its recipe:
# pkgrel, the spec's Release, the Debian revision, or the "Release revision"
# line in wineo-9999.ebuild. An identity that is already published and built
# is skipped. PPA, COPR, and OBS count only once the remote build succeeds on
# every target. A rerun after a timeout goes back to waiting and uploads
# nothing, and a failed build stays failed until the revision is bumped. A
# failed destination doesn't stop the later ones; the exit status is non-zero
# if any failed.
#
# --dest-dir DIR replaces every upload and remote status with a local fake in
# DIR, and nothing needs credentials. Everything else runs for real, so with
# real podman it proves the container steps. Controls, read from DIR/control:
#   fail-upload-DEST       the next upload fails; holding "always", every one
#   build-DEST             a remote build's state: built, building, failed,
#                          hidden (uploaded but not visible yet), or unknown.
#                          Several lines are read one per status check, and the
#                          last one sticks. build-DEST-TARGET overrides it.
# The record goes to DIR/uploads.log and DIR/uploads/DEST/TARGET/IDENTITY/. The
# overlay and AUR are bare repositories in DIR/remote, so their git is real.
#
# Environment:
#   WINEO_PUBLISH_WORK            parent of the scratch directory, which takes
#                                 gigabytes (default ~/.cache/wineo-publish, or
#                                 DIR with --dest-dir)
#   WINEO_PUBLISH_INTERVAL        seconds between mirror, run, and Release
#                                 checks (default 15)
#   WINEO_PUBLISH_BUILD_INTERVAL  seconds between remote build checks
#                                 (default 300; see build_interval below)
#   WINEO_PUBLISH_MIRROR_LIMIT, _RUN_LIMIT, _RELEASE_LIMIT,
#   _COPR_LIMIT, _OBS_LIMIT, _PPA_LIMIT
#                                 how long each wait lasts, in seconds
#   WINEO_PUBLISH_PPA_KEY         the signing key (default 8EE69509)
#   CARGO_DENY                    a cargo-deny to use instead of fetching the
#                                 pinned one
#   With --dest-dir only, for proving the real steps:
#   WINEO_PUBLISH_TAG_TARBALL     a stand-in for GitHub's tag tarball
#   WINEO_PUBLISH_PPA_SIMULATE    if set, sign and run `dput --simulate`
#
# Exits 2 on bad usage and 1 on a failed check or a failed destination.
#
# The container scripts are single-quoted so the host leaves them alone.
# shellcheck disable=SC2016
set -euo pipefail

repo=MeandrousShark/wineo
overlay_repo=MeandrousShark/wineo-overlay
copr_project=meandrousshark/wineo
copr_api=https://copr.fedorainfracloud.org/api_3
copr_token_url=https://copr.fedorainfracloud.org/api/
obs_project=home:meandrousshark
obs_package=wineo
ppa_name=ppa:meandrous-shark/wineo
lp_archive=https://api.launchpad.net/1.0/~meandrous-shark/+archive/ubuntu/wineo
lp_series=https://api.launchpad.net/1.0/ubuntu
fedora_image=registry.fedoraproject.org/fedora:44
ubuntu_image=docker.io/library/ubuntu:24.04
gentoo_image=docker.io/gentoo/stage3:latest

destinations=(overlay aur copr obs ppa)
aur_packages=(wineo wineo-bin wineo-git)

# Every repository target and its own Rust, reviewed against the plan's distro
# table. A target whose Rust falls below the floor stops the whole run; drop it
# here and from the recipes rather than building it with anything else.
rust_table='
ppa jammy 1.91
ppa noble 1.91
ppa resolute 1.93
ppa stonking 1.97
copr fedora-43 1.99.1
copr fedora-44 1.99.1
copr fedora-45 1.99.1
obs openSUSE_Tumbleweed 1.99.0
obs 16.0 1.98.0
obs 15.6 1.94.0
'

interval=${WINEO_PUBLISH_INTERVAL:-15}
# OBS's terms warn that excessively frequent API requests can suspend access,
# so remote builds are polled slowly, and COPR and Launchpad get the same.
build_interval=${WINEO_PUBLISH_BUILD_INTERVAL:-300}
mirror_limit=${WINEO_PUBLISH_MIRROR_LIMIT:-1800}
run_limit=${WINEO_PUBLISH_RUN_LIMIT:-14400}
release_limit=${WINEO_PUBLISH_RELEASE_LIMIT:-600}
copr_limit=${WINEO_PUBLISH_COPR_LIMIT:-21600}
obs_limit=${WINEO_PUBLISH_OBS_LIMIT:-43200}
ppa_limit=${WINEO_PUBLISH_PPA_LIMIT:-86400}
ppa_key=${WINEO_PUBLISH_PPA_KEY:-8EE69509}

usage() {
    echo "usage: ${0##*/} VERSION [--only DEST] [--skip DEST]... [--dest-dir DIR]" >&2
    echo "DEST: ${destinations[*]}" >&2
    exit 2
}

die() {
    echo "${0##*/}: $*" >&2
    exit 1
}

say() { echo "== $*"; }

is_destination() {
    local d
    for d in "${destinations[@]}"; do
        [[ $d != "$1" ]] || return 0
    done
    return 1
}

# Pure helpers. The tests call these with canned input.

# version_at_least A B: A is B or newer.
version_at_least() {
    [[ $(printf '%s\n' "$1" "$2" | sort -V | head -n 1) == "$2" ]]
}

# Prints the targets the floor refuses, as "dest target rust" lines.
targets_below_floor() {
    local floor=$1 dest target rust
    while read -r dest target rust; do
        [[ -n $dest ]] || continue
        version_at_least "$rust" "$floor" || echo "$dest $target $rust"
    done <<<"$rust_table"
}

targets_of() {
    case $1 in
        overlay) echo overlay ;;
        aur) printf '%s\n' "${aur_packages[@]}" ;;
        *) awk -v d="$1" '$1 == d { print $2 }' <<<"$rust_table" ;;
    esac
}

has_remote_build() { [[ $1 == copr || $1 == obs || $1 == ppa ]]; }

# COPR and OBS take one upload for all their targets; the others take one each.
one_upload() { [[ $1 == copr || $1 == obs ]]; }

# The field a release must hold. awk, not source: recipes run code.
pkgbuild_field() { awk -F= -v f="$1" '$1 == f { print $2; exit }' "$2"; }

spec_version() { awk '/^Version:/ { print $2; exit }' "$1"; }

# "1%{?dist}" becomes 1.
spec_release() { awk '/^Release:/ { sub(/%.*/, "", $2); print $2; exit }' "$1"; }

# "wineo (1.1.0-1ppa1) UNRELEASED; ..." becomes 1.1.0-1ppa1.
changelog_version() { sed -nE '1 s/^wineo \(([^)]+)\).*/\1/p' "$1"; }

ebuild_revision() { sed -nE 's/^# Release revision: ([0-9]+)$/\1/p' "$1"; }

# Asset names as in release.yml's `gh release create`; test-publish.sh compares
# them with that step, and SHA256SUMS lists the first four.
checksummed_assets() {
    printf '%s\n' \
        "wineo-$1-x86_64-linux.tar.gz" \
        "wineo_$1_amd64.deb" \
        "wineo-$1-1.x86_64.rpm" \
        "wineo-$1-vendored.tar.xz"
}

expected_assets() {
    checksummed_assets "$1"
    echo SHA256SUMS
}

# A failed run is the newest of those for the tag's commit, since a rerun of
# the workflow keeps its id.
tag_run() { jq -c --arg sha "$1" 'map(select(.headSha == $sha)) | max_by(.databaseId) // empty'; }

# COPR's chroot states on stdin, one "target state" line per target. forked is
# a build copied from a fork, so it is a success too.
copr_states() {
    local json target name state
    json=$(cat)
    while read -r target; do
        name=$target-x86_64
        state=$(jq -er --arg name "$name" \
            'first(.items[] | select(.name == $name) | .state) // "absent"' \
            <<<"$json" 2>/dev/null) || state=unknown
        case $state in
            succeeded | forked) state=built ;;
            failed | canceled | skipped) state=failed ;;
            unknown | absent) ;;
            *) state=building ;;
        esac
        echo "$target $state"
    done < <(targets_of copr)
}

# lp_state SOURCE_STATUS BUILDS_JSON BINARIES_JSON prints a series' state.
# Only amd64 counts: the recipe is amd64 only, and a PPA can also run
# variants that say nothing about the package users get. A built package
# isn't installable until its binary is published. Launchpad retries a
# dependency wait by itself, but nothing here can fix it, so it stops the wait;
# a rerun sees it if it clears.
lp_state() {
    local status=$1 builds=$2 binaries=$3 states
    case $status in
        Deleted | Obsolete)
            echo failed
            return
            ;;
    esac
    states=$(jq -er '[.entries[] | select(.arch_tag == "amd64") | .buildstate] | join("\n")' \
        <<<"$builds" 2>/dev/null) || {
        echo unknown
        return
    }
    if [[ -z $states ]]; then
        echo building
    elif grep -qx 'Successfully built' <<<"$states"; then
        if jq -e '[.entries[] | select(.status == "Published")
                | select(.distro_arch_series_link | endswith("/amd64"))] | length > 0' \
            <<<"$binaries" >/dev/null 2>&1; then
            echo built
        else
            echo building
        fi
    elif grep -qvxE 'Needs building|Currently building|Gathering build output|Uploading build|Cancelling build' <<<"$states"; then
        echo failed
    else
        echo building
    fi
}

# obs_states DIR IDENTITY: DIR holds what obs_status fetched, one "target
# state" line per target. A repository counts only if its newest job built the
# source now on the server, since _result can still show the last revision's
# result right after a commit.
obs_states() {
    local dir=$1 identity=$2 targets
    targets=$(targets_of obs | tr '\n' ' ')
    python3 -I - "$dir" "$identity" "$targets" <<'EOF'
import os
import re
import sys
import xml.etree.ElementTree as ET

folder, identity, targets = sys.argv[1], sys.argv[2], sys.argv[3].split()


def emit(state):
    for target in targets:
        print(target, state)
    sys.exit(0)


def read(name):
    with open(os.path.join(folder, name), encoding="utf-8") as handle:
        return handle.read()


try:
    if os.path.exists(os.path.join(folder, "missing")):
        emit("absent")
    spec = read("spec")
    version = re.search(r"^Version:\s*(\S+)", spec, re.M).group(1)
    release = re.search(r"^Release:\s*([^%\s]+)", spec, re.M).group(1)
    if f"{version}-{release}" != identity:
        emit("absent")
    srcmd5 = ET.fromstring(read("directory")).get("srcmd5")
    results = {r.get("repository"): r for r in ET.fromstring(read("result")).iter("result")}
    for target in targets:
        result = results.get(target)
        status = None if result is None else result.find("status")
        code = "" if status is None else status.get("code")
        jobs = list(ET.fromstring(read(f"jobhistory-{target}")).iter("jobhist"))
        job = jobs[-1] if jobs else None
        if result is None or status is None or result.get("dirty") == "true":
            state = "building"
        elif code in ("unresolvable", "broken", "excluded", "disabled"):
            state = "failed"
        elif job is None or job.get("srcmd5") != srcmd5:
            state = "building"
        elif code == "succeeded" and job.get("code") in ("succeeded", "unchanged"):
            state = "built"
        elif code == "failed":
            state = "failed"
        else:
            state = "building"
        print(target, state)
except (OSError, AttributeError, ET.ParseError):
    emit("unknown")
EOF
}

# osc says this for a package or file that isn't there, which is an answer.
# Anything else means the state wasn't read.
osc_said_missing() { grep -qE 'HTTP Error 404|404 Client Error|Not Found' "$1"; }

copr_auth_failed() { grep -qiE 'login invalid|expired|unauthori[sz]ed|forbidden|token' "$1"; }

# Everything below needs a run: version, root, work, and so on.

record() { echo "$2" >"$work/result-$1"; }

dest_fail() {
    echo "${0##*/}: $current: $*" >&2
    record "$current" "failed: $*"
    exit 1
}

need_tool() {
    command -v "$1" >/dev/null || dest_fail "$1 not found: $2"
}

# Container steps. A step that's needed only once a destination has something
# to do pulls its image then, so a run that skips everything needs no podman.

tool_image() {
    local kind=$1 name base packages builder
    name=localhost/wineo-publish-$kind-$$
    if podman image exists "$name" 2>/dev/null; then
        echo "$name"
        return
    fi
    builder=wineo-publish-builder-$kind-$$
    case $kind in
        fedora)
            base=$fedora_image
            packages='dnf -y install rpm-build copr-cli osc'
            ;;
        ubuntu)
            base=$ubuntu_image
            packages='export DEBIAN_FRONTEND=noninteractive
                apt-get update
                apt-get install -y --no-install-recommends ca-certificates debhelper \
                    devscripts dpkg-dev dput fakeroot gnupg xz-utils'
            ;;
    esac
    say "$current: installing the tools into a $base image" >&2
    nice -n 15 podman run --name "$builder" -e STEP="tools-$kind" "$base" \
        bash -euo pipefail -c "$packages" >&2
    podman commit -q "$builder" "$name" >/dev/null
    podman rm "$builder" >/dev/null
    echo "$name"
}

remove_tool_images() {
    local kind
    command -v podman >/dev/null || return 0
    for kind in fedora ubuntu; do
        podman rmi -f "localhost/wineo-publish-$kind-$$" >/dev/null 2>&1 || true
    done
}

# Local repositories for the fakes. A hook refuses pushes on request, so the
# overlay and AUR run the same git as the real thing.
fake_repo() {
    local dest=$1 name=$2 branch=main
    local bare=$dest_dir/remote/$name.git
    [[ $dest != aur ]] || branch=master
    if [[ ! -d $bare ]]; then
        git init -q --bare --initial-branch="$branch" "$bare"
        cat >"$bare/hooks/pre-receive" <<EOF
#!/bin/bash
control='$dest_dir/control/fail-upload-$dest'
[[ -f \$control ]] || exit 0
grep -qx always "\$control" || rm -f "\$control"
echo 'fake $dest: push refused' >&2
exit 1
EOF
        chmod +x "$bare/hooks/pre-receive"
    fi
    echo "$bare"
}

# Fake status and upload for the destinations with a remote build.

fake_state() {
    local dest=$1 target=$2 state
    local control=$dest_dir/control/build-$dest-$target
    [[ -f $control ]] || control=$dest_dir/control/build-$dest
    if [[ ! -f $control ]]; then
        echo built
        return
    fi
    state=$(head -n 1 "$control")
    if [[ $(wc -l <"$control") -gt 1 ]]; then sed -i 1d "$control"; fi
    echo "$state"
}

fake_status() {
    local dest=$1 target state
    while read -r target; do
        if [[ ! -e $dest_dir/remote/$dest/$target/$(identity_of "$dest" "$target") ]]; then
            state=absent
        else
            state=$(fake_state "$dest" "$target")
            [[ $state != hidden ]] || state=absent
        fi
        echo "$target $state"
    done < <(targets_of "$dest")
}

fake_upload() {
    local dest=$1 unit=$2 target record_dir identity
    local control=$dest_dir/control/fail-upload-$dest
    shift 2
    if [[ -f $control ]]; then
        grep -qx always "$control" || rm -f "$control"
        echo "fake $dest: upload refused" >&2
        return 1
    fi
    if [[ $unit == all ]]; then
        identity=$(identity_of "$dest" "$(targets_of "$dest" | head -n 1)")
    else
        identity=$(identity_of "$dest" "$unit")
    fi
    record_dir=$dest_dir/uploads/$dest/$unit/$identity
    mkdir -p "$record_dir"
    cp -- "$@" "$record_dir/"
    echo "upload $dest $unit $identity" >>"$dest_dir/uploads.log"
    while read -r target; do
        [[ $unit == all || $unit == "$target" ]] || continue
        mkdir -p "$dest_dir/remote/$dest/$target"
        touch "$dest_dir/remote/$dest/$target/$(identity_of "$dest" "$target")"
    done < <(targets_of "$dest")
}

# Each identity is the recipe's, so it comes from HEAD and not from the tag.

identity_of() {
    local dest=$1 target=$2 revision
    case $dest in
        overlay)
            revision=$(ebuild_revision "$recipes/packaging/gentoo/games-util/wineo/wineo-9999.ebuild")
            if [[ $revision == 0 ]]; then echo "$version"; else echo "$version-r$revision"; fi
            ;;
        aur)
            echo "$version-$(pkgbuild_field pkgrel "$(aur_recipe "$target")")"
            ;;
        copr | obs)
            echo "$version-$(spec_release "$recipes/packaging/rpm/wineo.spec")"
            ;;
        ppa)
            echo "$(changelog_version "$recipes/packaging/debian/changelog")~${target}1"
            ;;
    esac
}

identity_label() {
    case $1 in
        ppa) changelog_version "$recipes/packaging/debian/changelog" ;;
        *) identity_of "$1" "$(targets_of "$1" | head -n 1)" ;;
    esac
}

aur_recipe() {
    if [[ $1 == wineo-git ]]; then
        echo "$recipes/PKGBUILD"
    else
        echo "$recipes/packaging/aur/$1/PKGBUILD"
    fi
}

# GitHub's generated tarball for the tag: the wineo recipe's source and the
# ebuild's. It isn't a Release asset, so SHA256SUMS can't vouch for it.
tag_tarball() {
    local file=$work/wineo-$version.gh.tar.gz
    if [[ ! -f $file ]]; then
        if [[ -n $dest_dir && -n ${WINEO_PUBLISH_TAG_TARBALL:-} ]]; then
            cp "$WINEO_PUBLISH_TAG_TARBALL" "$file"
        else
            curl --proto '=https' --tlsv1.2 -fsSL --retry 3 -o "$file" \
                "https://github.com/$repo/archive/refs/tags/$tag.tar.gz" ||
                dest_fail "could not download GitHub's tarball for $tag"
        fi
    fi
    echo "$file"
}

clone_repo() {
    local url=$1 into=$2
    rm -rf "$into"
    git -c credential.helper= -c 'credential.helper=!gh auth git-credential' \
        clone -q "$url" "$into" 2>/dev/null
}

push_repo() {
    local clone=$1 branch=$2 dest=$3 unit=$4 identity=$5
    git -C "$clone" -c credential.helper= -c 'credential.helper=!gh auth git-credential' \
        push -q origin "HEAD:refs/heads/$branch"
    if [[ -n $dest_dir ]]; then echo "upload $dest $unit $identity" >>"$dest_dir/uploads.log"; fi
}

# overlay

overlay_url() {
    if [[ -n $dest_dir ]]; then
        fake_repo overlay overlay
    else
        echo "https://github.com/$overlay_repo.git"
    fi
}

overlay_status() {
    local identity
    identity=$(identity_of overlay overlay)
    if ! clone_repo "$(overlay_url)" "$work/overlay/repo"; then
        echo overlay unknown
    elif [[ -f $work/overlay/repo/games-util/wineo/wineo-$identity.ebuild ]]; then
        echo overlay built
    else
        echo overlay absent
    fi
}

overlay_ready() { need_tool podman "install podman; the manifest runs in a gentoo/stage3 container"; }

# The crate list has to match the tag's source, and HEAD may have moved on, so
# gentoo-ebuild.sh runs in the tag's tree with HEAD's recipe laid over it.
overlay_prepare() {
    local identity clone=$work/overlay/repo tree=$work/overlay/tag distfiles=$work/overlay/distfiles
    local package_dir name=wineo-$version.gh.tar.gz tarball size
    identity=$(identity_of overlay overlay)
    package_dir=$clone/games-util/wineo
    mkdir -p "$tree" "$distfiles" "$work/overlay/tmp" "$work/overlay/gentoo-tree" "$package_dir"
    git -C "$root" archive "$tag" | tar -x -C "$tree"
    rm -rf "$tree/packaging/gentoo"
    cp -r "$recipes/packaging/gentoo" "$tree/packaging/gentoo"
    cp "$recipes/scripts/gentoo-ebuild.sh" "$tree/scripts/gentoo-ebuild.sh"

    # All of it, since the first run starts from an empty repository.
    cp -r "$recipes/packaging/gentoo/." "$clone/"
    tarball=$(tag_tarball)
    cp "$tarball" "$distfiles/$name"

    # gentoo-ebuild.sh runs in there too, since pycargoebuild needs wget or
    # aria2, which stage3 has. It doesn't write straight into the overlay,
    # because under a new revision that would overwrite the published ebuild.
    # assume-digests keeps the DIST lines of older ebuilds, whose distfiles
    # aren't here. See check-gentoo-build.sh for the settings rootless podman
    # needs.
    nice -n 15 podman run --rm -e STEP=gentoo-manifest -e IDENTITY="$identity" \
        -e VERSION="$version" -e WINEO_GENTOO_DISTDIR=/var/cache/distfiles \
        -v "$tree:/tag:ro" \
        --tmpfs /tmp:rw,exec,nosuid,nodev,mode=1777 \
        -v "$clone:/overlay" -v "$distfiles:/var/cache/distfiles" \
        -v "$work/overlay/tmp:/var/tmp/portage" \
        -v "$work/overlay/gentoo-tree:/var/db/repos/gentoo" \
        "$gentoo_image" bash -euo pipefail -c '
        cat >>/etc/portage/make.conf <<MAKECONF
FEATURES="-sandbox -usersandbox -ipc-sandbox -pid-sandbox -network-sandbox -mount-sandbox -userpriv -userfetch assume-digests"
EMERGE_DEFAULT_OPTS="--getbinpkg --usepkg"
TAR_OPTIONS="--no-same-owner"
MAKECONF
        mkdir -p /etc/portage/repos.conf
        cp /usr/share/portage/config/repos.conf /etc/portage/repos.conf/gentoo.conf
        printf "[wineo]\nlocation = /overlay\nauto-sync = no\n" >/etc/portage/repos.conf/wineo.conf
        if [[ ! -e /var/db/repos/gentoo/profiles ]]; then
            emerge-webrsync
        fi
        getuto
        emerge -q dev-util/pkgcheck
        mkdir /tmp/ebuild
        (cd /tmp && /tag/scripts/gentoo-ebuild.sh "$VERSION" /tmp/ebuild)
        cp "/tmp/ebuild/wineo-$VERSION.ebuild" "/overlay/games-util/wineo/wineo-$IDENTITY.ebuild"
        (cd /overlay/games-util/wineo && ebuild "wineo-$IDENTITY.ebuild" manifest)
        cd /overlay
        pkgcheck scan --repo wineo --exit error
    ' || dest_fail "gentoo-ebuild.sh, ebuild manifest, or pkgcheck failed; see the output above"

    size=$(stat -c %s "$tarball")
    grep -q "^DIST $name $size " "$package_dir/Manifest" ||
        dest_fail "the Manifest doesn't hold GitHub's tarball for $tag"
}

overlay_upload() {
    local identity clone=$work/overlay/repo
    identity=$(identity_of overlay overlay)
    # Portage's cache of the ebuilds, written while they were scanned.
    rm -rf "$clone/metadata/md5-cache"
    git -C "$clone" checkout -q -B main
    git -C "$clone" add -- .
    git -C "$clone" commit -q -m "games-util/wineo: add $identity"
    push_repo "$clone" main overlay overlay "$identity"
}

# aur

aur_url() {
    if [[ -n $dest_dir ]]; then
        fake_repo aur "$1"
    else
        echo "ssh://aur@aur.archlinux.org/$1.git"
    fi
}

aur_status() {
    local package identity clone srcinfo
    for package in "${aur_packages[@]}"; do
        identity=$(identity_of aur "$package")
        clone=$work/aur/repos/$package
        mkdir -p "$work/aur/repos"
        if ! clone_repo "$(aur_url "$package")" "$clone"; then
            echo "$package unknown"
            continue
        fi
        srcinfo=$clone/.SRCINFO
        if [[ -f $srcinfo ]] &&
            [[ "$(sed -nE 's/^\s*pkgver = //p' "$srcinfo" | head -n 1)-$(sed -nE 's/^\s*pkgrel = //p' "$srcinfo" | head -n 1)" == "$identity" ]]; then
            echo "$package built"
        else
            echo "$package absent"
        fi
    done
}

aur_ready() { need_tool makepkg "it writes .SRCINFO, and comes with Arch's pacman"; }

# Fills in a temporary copy, never the checkout. wineo-git's pkgver() runs on
# the user's machine, but .SRCINFO has to name something better than the 0 the
# repository keeps. Its SKIP is right for a git source.
aur_prepare() {
    local package=$1 recipe sum tarball name
    local out=$work/aur/out/$package
    recipe=$(aur_recipe "$package")
    mkdir -p "$out"
    cp "$recipe" "$out/PKGBUILD"
    case $package in
        wineo)
            tarball=$(tag_tarball)
            sum=$(sha256sum "$tarball" | awk '{print $1}')
            ;;
        wineo-bin)
            name=wineo-$version-x86_64-linux.tar.gz
            sum=$(awk -v n="$name" '$2 == n { print $1 }' "$assets/SHA256SUMS")
            ;;
        wineo-git)
            sed -i -E "s/^pkgver=.*/pkgver=$version/" "$out/PKGBUILD"
            cp "$recipes/packaging/aur/wineo/LICENSE" "$out/LICENSE"
            ;;
    esac
    if [[ $package != wineo-git ]]; then
        cp "$(dirname "$recipe")/LICENSE" "$out/LICENSE"
        if [[ $sum =~ ^[0-9a-f]{64}$ ]]; then
            sed -i -E "s/^sha256sums=\('SKIP'\)$/sha256sums=('$sum')/" "$out/PKGBUILD"
        fi
        if grep -q SKIP "$out/PKGBUILD"; then
            dest_fail "$package still has a SKIP checksum after pinning; refusing to publish it"
        fi
    fi
    (cd "$out" && makepkg --printsrcinfo >.SRCINFO) ||
        dest_fail "makepkg --printsrcinfo failed for $package"
    [[ -s $out/.SRCINFO ]] || dest_fail "makepkg wrote an empty .SRCINFO for $package"
}

aur_upload() {
    local package=$1 clone=$work/aur/repos/$1 identity
    identity=$(identity_of aur "$package")
    cp "$work/aur/out/$package/PKGBUILD" "$work/aur/out/$package/.SRCINFO" \
        "$work/aur/out/$package/LICENSE" "$clone/"
    git -C "$clone" checkout -q -B master
    git -C "$clone" add -- PKGBUILD .SRCINFO LICENSE
    git -C "$clone" commit -q -m "Update to $identity"
    push_repo "$clone" master aur "$package" "$identity"
}

# copr

copr_status() {
    local identity list build states
    identity=$(identity_of copr "$(targets_of copr | head -n 1)")
    list=$(curl -fsS --max-time 60 \
        "$copr_api/build/list?ownername=${copr_project%/*}&projectname=${copr_project#*/}&limit=100&order=id&order_type=DESC") || list=
    # The newest build of the identity, so a resubmission by hand counts.
    if ! build=$(jq -er --arg id "$identity" \
        '[.items[] | select(.source_package.version == $id) | .id] | max // ""' \
        <<<"$list" 2>/dev/null); then
        targets_of copr | sed 's/$/ unknown/'
    elif [[ -z $build ]]; then
        targets_of copr | sed 's/$/ absent/'
    else
        states=$(curl -fsS --max-time 60 "$copr_api/build-chroot/list?build_id=$build") || states=
        copr_states <<<"$states"
    fi
}

copr_ready() {
    need_tool podman "install podman; rpmbuild and copr-cli run in a Fedora container"
    [[ -n $dest_dir ]] && return 0
    local config=${XDG_CONFIG_HOME:-$HOME/.config}/copr mode
    [[ -f $config ]] ||
        dest_fail "$config is missing: get an API token from $copr_token_url and save it there with mode 600"
    mode=$(stat -c %a "$config")
    [[ $mode == 600 ]] || dest_fail "$config has mode $mode: run chmod 600 on it"
}

copr_prepare() {
    local top=$work/copr/rpmbuild identity image srpm
    identity=$(identity_of copr "$(targets_of copr | head -n 1)")
    mkdir -p "$top/SOURCES" "$top/SPECS" "$top/SRPMS"
    cp "$assets/wineo-$version-vendored.tar.xz" "$top/SOURCES/"
    cp "$recipes/packaging/rpm/wineo.spec" "$top/SPECS/"
    image=$(tool_image fedora)
    # No dist tag, so the source package is plain VERSION-RELEASE, which is
    # what identifies it on COPR. Each chroot still adds its own.
    nice -n 15 podman run --rm -e STEP=srpm -v "$top:/work/rpmbuild" "$image" \
        bash -euo pipefail -c '
        rpmbuild -bs --define "_topdir /work/rpmbuild" --define "dist %{nil}" \
            /work/rpmbuild/SPECS/wineo.spec
    ' || dest_fail "rpmbuild -bs failed; see the output above"
    srpm=$top/SRPMS/wineo-$identity.src.rpm
    [[ -f $srpm ]] || dest_fail "rpmbuild didn't write $(basename "$srpm")"
}

copr_files() { echo "$work/copr/rpmbuild/SRPMS/wineo-$(identity_of copr "$(targets_of copr | head -n 1)").src.rpm"; }

copr_upload() {
    local srpm config=${XDG_CONFIG_HOME:-$HOME/.config}/copr image output=$work/copr/upload.txt
    local chroots=() target
    srpm=$(copr_files)
    image=$(tool_image fedora)
    while read -r target; do
        chroots+=(--chroot "$target-x86_64")
    done < <(targets_of copr)
    if ! nice -n 15 podman run --rm -e STEP=copr-upload -e SRPM="/work/$(basename "$srpm")" \
        -e PROJECT="$copr_project" -v "$config:/root/.config/copr:ro" \
        -v "$(dirname "$srpm"):/work:ro" "$image" \
        bash -euo pipefail -c 'exec copr-cli build --nowait "$@" "$PROJECT" "$SRPM"' \
        bash "${chroots[@]}" 2>&1 | tee "$output"; then
        if copr_auth_failed "$output"; then
            dest_fail "COPR refused the login. COPR tokens expire: get a new one from $copr_token_url and save it in $config"
        fi
        dest_fail "copr-cli build failed; see the output above"
    fi
}

# obs

obs_url() { echo "https://build.opensuse.org/package/show/$obs_project/$obs_package"; }

obs_config() { echo "${XDG_CONFIG_HOME:-$HOME/.config}/osc/oscrc"; }

# OBS has no public read, so even the status needs oscrc.
obs_ready() {
    need_tool podman "install podman; osc runs in a Fedora container"
    [[ -n $dest_dir ]] && return 0
    local config
    config=$(obs_config)
    [[ -f $config ]] || dest_fail "$config is missing: create it with [general] apiurl=https://api.opensuse.org and a [https://api.opensuse.org] section holding user, pass, and credentials_mgr_class=osc.credentials.ObfuscatedConfigFileCredentialsManager, with mode 600. Keyrings can't work in the container."
}

obs_status() {
    local dir=$work/obs/status image identity
    identity=$(identity_of obs "$(targets_of obs | head -n 1)")
    rm -rf "$dir"
    mkdir -p "$dir"
    image=$(tool_image fedora)
    if ! nice -n 15 podman run --rm -e STEP=obs-status -e PROJECT="$obs_project" \
        -e PACKAGE="$obs_package" -e TARGETS="$(targets_of obs | tr '\n' ' ')" \
        -v "$(obs_config):/root/.config/osc/oscrc:ro" -v "$dir:/status" "$image" \
        bash -euo pipefail -c '
        source_path=/source/$PROJECT/$PACKAGE
        osc api "$source_path" >/status/directory 2>/status/error
        osc api "$source_path/wineo.spec" >/status/spec 2>/status/error
        osc api "/build/$PROJECT/_result?package=$PACKAGE&arch=x86_64" >/status/result 2>/status/error
        for target in $TARGETS; do
            osc api "/build/$PROJECT/$target/x86_64/_jobhistory?package=$PACKAGE&limit=5" \
                >"/status/jobhistory-$target" 2>/status/error
        done
    ' >/dev/null 2>&1; then
        if [[ -f $dir/error ]] && osc_said_missing "$dir/error" && [[ ! -s $dir/spec ]]; then
            touch "$dir/missing"
        else
            targets_of obs | sed 's/$/ unknown/'
            return
        fi
    fi
    obs_states "$dir" "$identity"
}

obs_prepare() {
    mkdir -p "$work/obs/files"
    cp "$recipes/packaging/rpm/wineo.spec" "$assets/wineo-$version-vendored.tar.xz" "$work/obs/files/"
}

obs_files() {
    echo "$work/obs/files/wineo.spec"
    echo "$work/obs/files/wineo-$version-vendored.tar.xz"
}

obs_upload() {
    local image identity
    identity=$(identity_of obs "$(targets_of obs | head -n 1)")
    image=$(tool_image fedora)
    nice -n 15 podman run --rm -e STEP=obs-upload -e PROJECT="$obs_project" \
        -e PACKAGE="$obs_package" -e MESSAGE="Update to $identity" \
        -v "$(obs_config):/root/.config/osc/oscrc:ro" -v "$work/obs/files:/files:ro" \
        "$image" bash -euo pipefail -c '
        cd /tmp
        osc checkout "$PROJECT" "$PACKAGE"
        cd "$PROJECT/$PACKAGE"
        rm -f wineo-*-vendored.tar.xz
        cp /files/* .
        osc addremove
        osc commit -m "$MESSAGE"
    ' || dest_fail "osc commit failed. If the package doesn't exist, create $obs_project/$obs_package on OBS first"
}

# ppa

ppa_ready() {
    need_tool podman "install podman; the source package is built in an Ubuntu container"
    if [[ -n $dest_dir && -z ${WINEO_PUBLISH_PPA_SIMULATE:-} ]]; then return 0; fi
    need_tool gpg "install gnupg; the upload is signed through the host's gpg-agent"
    need_tool gpgconf "install gnupg; the upload is signed through the host's gpg-agent"
    gpg --list-keys "$ppa_key" >/dev/null 2>&1 ||
        dest_fail "gpg has no key $ppa_key: import it, or set WINEO_PUBLISH_PPA_KEY"
}

ppa_status() {
    local series identity sources status link builds binaries
    for series in $(targets_of ppa); do
        identity=$(identity_of ppa "$series")
        sources=$(curl -fsS -G --max-time 60 "$lp_archive" \
            --data-urlencode ws.op=getPublishedSources \
            --data-urlencode source_name=wineo --data-urlencode exact_match=true \
            --data-urlencode "version=$identity" \
            --data-urlencode "distro_series=$lp_series/$series") || sources=
        if ! status=$(jq -er '.entries[0].status // ""' <<<"$sources" 2>/dev/null); then
            echo "$series unknown"
        elif [[ -z $status ]]; then
            echo "$series absent"
        else
            link=$(jq -r '.entries[0].self_link' <<<"$sources")
            builds=$(curl -fsS --max-time 60 "$link?ws.op=getBuilds") || builds=
            binaries=$(curl -fsS --max-time 60 "$link?ws.op=getPublishedBinaries") || binaries=
            echo "$series $(lp_state "$status" "$builds" "$binaries")"
        fi
    done
}

# The orig is the same bytes for every series, which Launchpad requires of the
# same file name, and the tree is unpacked once per series so that one series'
# debian/ can't reach the next.
ppa_prepare() {
    local series=$1 identity image orig=wineo_$version.orig.tar.xz
    local dir=$work/ppa/$series
    identity=$(identity_of ppa "$series")
    mkdir -p "$dir"
    ln "$assets/wineo-$version-vendored.tar.xz" "$dir/$orig" 2>/dev/null ||
        cp "$assets/wineo-$version-vendored.tar.xz" "$dir/$orig"
    tar -xf "$dir/$orig" -C "$dir"
    "$recipes/scripts/prepare-debian-tree.sh" --recipe "$recipes/packaging/debian" \
        "$dir/wineo-$version" "$series" || dest_fail "prepare-debian-tree.sh failed for $series"
    image=$(tool_image ubuntu)
    # -d: the build dependencies are the series' own, and only Launchpad's
    # builder needs them. -sa: every series may carry the orig, since it's
    # byte for byte the same one.
    nice -n 15 podman run --rm -e STEP=deb-source -e SERIES="$series" -e VERSION="$version" \
        -v "$dir:/work" -w "/work/wineo-$version" "$image" \
        bash -euo pipefail -c 'dpkg-buildpackage -S -sa -d -us -uc' ||
        dest_fail "dpkg-buildpackage -S failed for $series; see the output above"
    [[ -f $dir/wineo_${identity}_source.changes ]] ||
        dest_fail "dpkg-buildpackage didn't write wineo_${identity}_source.changes"
    rm -rf "$dir/wineo-$version"
}

ppa_files() {
    local series=$1 dir=$work/ppa/$1 identity
    identity=$(identity_of ppa "$series")
    echo "$dir/wineo_$version.orig.tar.xz"
    echo "$dir/wineo_$identity.dsc"
    echo "$dir/wineo_${identity}_source.changes"
    echo "$dir/wineo_$identity.debian.tar.xz"
    echo "$dir/wineo_${identity}_source.buildinfo"
}

# The private key and the passphrase prompt stay on the host: the container
# gets the agent's socket and the public key.
ppa_upload() {
    local series=$1 mode=${2:-} dir=$work/ppa/$1 identity image socket key=$work/ppa/signing-key.asc
    local dput_args=("$ppa_name")
    identity=$(identity_of ppa "$series")
    if [[ $mode == simulate ]]; then dput_args=(--simulate "$ppa_name"); fi
    gpg --export --armor "$ppa_key" >"$key"
    [[ -s $key ]] || dest_fail "gpg exported nothing for $ppa_key"
    gpgconf --launch gpg-agent
    socket=$(gpgconf --list-dirs agent-socket)
    image=$(tool_image ubuntu)
    nice -n 15 podman run --rm -e STEP=deb-sign -e KEY="$ppa_key" \
        -e CHANGES="wineo_${identity}_source.changes" \
        -v "$dir:/work" -v "$key:/signing-key.asc:ro" \
        -v "$socket:/root/.gnupg/S.gpg-agent" -w /work "$image" \
        bash -euo pipefail -c '
        chmod 700 /root/.gnupg
        gpg --batch --quiet --import /signing-key.asc
        debsign --no-conf -k"$KEY" "$CHANGES"
        exec dput "$@" "$CHANGES"
    ' bash "${dput_args[@]}" || dest_fail "signing or dput failed for $series; see the output above"
}

# The seams: fake or real, and nothing else differs.

status_of() {
    if [[ -n $dest_dir ]] && has_remote_build "$1"; then
        fake_status "$1"
    else
        "$1_status"
    fi
}

upload_of() {
    local dest=$1 unit=$2 files=()
    if [[ -z $dest_dir ]] || ! has_remote_build "$dest"; then
        if one_upload "$dest"; then "$dest"_upload; else "$dest"_upload "$unit"; fi
        return
    fi
    if [[ $dest == ppa && -n ${WINEO_PUBLISH_PPA_SIMULATE:-} ]]; then
        ppa_upload "$unit" simulate
    fi
    if one_upload "$dest"; then
        mapfile -t files < <("$dest"_files)
    else
        mapfile -t files < <("$dest"_files "$unit")
    fi
    fake_upload "$dest" "$unit" "${files[@]}"
}

prepare_of() {
    if one_upload "$1" || [[ $1 == overlay ]]; then
        "$1_prepare"
    else
        "$1_prepare" "$2"
    fi
}

# The remote may not show an upload for a while, and Launchpad reports a
# rejection only by mail, so what this machine sent is remembered. Without this
# a rerun would see nothing there and upload again.
marker() { echo "$state_dir/$1-$2-$(identity_of "$1" "$2")"; }

# Waits until every target is built. Failed builds and timeouts both end the
# destination, and neither lets a rerun upload again.
wait_for_builds() {
    local dest=$1 limit=$2 link=$3 start=$SECONDS line target state failed waiting last='' hint
    local built total
    total=$(targets_of "$dest" | wc -l)
    while :; do
        # First, since the caller has just read the status or uploaded. Every
        # poll stays a full interval from the one before.
        sleep "$build_interval"
        failed=() waiting=() built=0
        while read -r target state; do
            case $state in
                built) built=$((built + 1)) ;;
                failed) failed+=("$target") ;;
                *) waiting+=("$target($state)") ;;
            esac
        done < <(status_of "$dest")
        if ((${#failed[@]} > 0)); then
            dest_fail "the build failed on ${failed[*]} ($link). Fix the recipe, bump its revision, and run --only $dest"
        fi
        if ((built == total)); then
            return 0
        fi
        line=${waiting[*]:-its status}
        if [[ $line != "$last" ]]; then
            say "$dest: waiting on $line"
            last=$line
        fi
        if ((SECONDS - start >= limit)); then
            hint=
            if [[ $dest == ppa ]]; then
                hint=" Launchpad reports a rejected upload only by mail."
            fi
            dest_fail "timed out after ${limit}s waiting on $line ($link).$hint Rerun to keep waiting; nothing is uploaded again"
        fi
    done
}

publish_destination() {
    local dest=$1 target state label all_built=1 failed=() unknown=() absent=() pending=()
    local limit link total=0 unit status
    current=$dest
    label=$(identity_label "$dest")
    if [[ $dest == obs ]]; then obs_ready; fi

    while read -r target state; do
        total=$((total + 1))
        [[ $state == built ]] || all_built=0
        case $state in
            failed) failed+=("$target") ;;
            unknown) unknown+=("$target") ;;
            absent) absent+=("$target") ;;
        esac
    done < <(status_of "$dest")

    if ((${#unknown[@]} > 0)); then
        # Never upload on a guess: an unknown state may hide a real build.
        case $dest in
            overlay) dest_fail "could not read $overlay_repo; check 'gh auth status' and that the repository exists" ;;
            aur) dest_fail "could not read ${unknown[*]} on the AUR; the account needs an SSH key at https://aur.archlinux.org/account/ (run --skip aur until there is one)" ;;
            *) dest_fail "could not read the build state of ${unknown[*]}; try again" ;;
        esac
    fi
    # Too few lines mean a status that broke, which must not read as built.
    if ((total != $(targets_of "$dest" | wc -l))); then
        dest_fail "could not read the build state; try again"
    fi
    if ((all_built)); then
        say "$dest: $label is already published and built, skipping"
        record "$dest" "already published ($label)"
        return 0
    fi
    case $dest in
        copr)
            link=https://copr.fedorainfracloud.org/coprs/$copr_project/builds/
            limit=$copr_limit
            ;;
        obs)
            link=$(obs_url)
            limit=$obs_limit
            ;;
        ppa)
            link=https://launchpad.net/~meandrous-shark/+archive/ubuntu/wineo/+builds
            limit=$ppa_limit
            ;;
        *)
            link=''
            limit=0
            ;;
    esac
    if ((${#failed[@]} > 0)); then
        dest_fail "$label already failed to build on ${failed[*]} ($link). Fix the recipe, bump its revision, and run --only $dest"
    fi

    for target in ${absent[@]+"${absent[@]}"}; do
        if ! has_remote_build "$dest" || [[ ! -e $(marker "$dest" "$target") ]]; then
            pending+=("$target")
        fi
    done
    # One upload covers them all, so it goes only if none of them saw it.
    if one_upload "$dest"; then
        if ((${#pending[@]} == total)); then pending=(all); else pending=(); fi
    fi

    if ((${#pending[@]} > 0)); then
        if [[ $(type -t "${dest}_ready") == function ]]; then "${dest}_ready"; fi
        # Everything is prepared first, so a recipe that can't go out stops
        # the destination before any of it is published.
        for unit in "${pending[@]}"; do
            prepare_of "$dest" "$unit"
        done
        for unit in "${pending[@]}"; do
            say "$dest: uploading $unit"
            # Not with `|| dest_fail`: that would turn errexit off inside it,
            # and a refused push would pass.
            set +e
            (
                set -e
                upload_of "$dest" "$unit"
            )
            status=$?
            set -e
            if ((status != 0)); then
                # A seam that said why has already recorded it.
                [[ -s $work/result-$dest ]] || record "$dest" "failed: the upload of $unit failed; see the output above"
                exit 1
            fi
            if has_remote_build "$dest"; then
                while read -r target; do
                    [[ $unit == all || $unit == "$target" ]] || continue
                    : >"$(marker "$dest" "$target")"
                done < <(targets_of "$dest")
            fi
        done
    fi

    if has_remote_build "$dest"; then
        wait_for_builds "$dest" "$limit" "$link"
    fi
    say "$dest: published $label"
    record "$dest" "published ($label)"
}

run_destination() {
    local dest=$1 status
    say "$dest"
    # Not in an `if`: that would turn errexit off for all of it.
    set +e
    (
        set -e
        publish_destination "$dest"
    )
    status=$?
    set -e
    if ((status != 0)) && [[ ! -s $work/result-$dest ]]; then
        record "$dest" "failed: it stopped early; see the output above"
    fi
    return 0
}

# Checks before anything is published.

check_mirror() {
    gh api "repos/$repo/git/ref/tags/$tag" 2>/dev/null | jq -er '.object.sha' >"$work/github-tag" 2>/dev/null
}

check_run() {
    local runs run status conclusion url
    runs=$(gh run list -R "$repo" --workflow release.yml --event push --branch "$tag" \
        --json databaseId,headSha,status,conclusion,url 2>/dev/null) || return 1
    run=$(tag_run "$commit" <<<"$runs") || return 1
    if [[ -z $run ]]; then
        # GitHub drops old runs, which a revision bump long after a release
        # would run into. Only that run can have made the Release.
        if check_release; then
            say "no release run left for $tag, but the Release exists"
            return 0
        fi
        return 1
    fi
    status=$(jq -r .status <<<"$run")
    conclusion=$(jq -r .conclusion <<<"$run")
    url=$(jq -r .url <<<"$run")
    [[ $status == completed ]] || return 1
    [[ $conclusion == success ]] || die "the release run for $tag ended in $conclusion: $url"
}

check_release() {
    gh release view "$tag" -R "$repo" --json tagName,isDraft,url,assets >"$work/release.json" 2>/dev/null &&
        jq -e '.tagName' "$work/release.json" >/dev/null 2>&1
}

check_head_on_main() {
    local compare
    compare=$(gh api "repos/$repo/compare/main...$head" 2>/dev/null) || return 1
    [[ $(jq -r .ahead_by <<<"$compare") == 0 ]]
}

wait_until() {
    local what=$1 limit=$2 start=$SECONDS said=0
    shift 2
    until "$@"; do
        if ((SECONDS - start >= limit)); then return 1; fi
        if ((said == 0)); then
            say "waiting for $what"
            said=1
        fi
        sleep "$interval"
    done
}

check_checkout() {
    local dirty
    git -C "$root" rev-parse -q --verify "refs/tags/$tag" >/dev/null ||
        die "there is no local tag $tag; fetch it first"
    commit=$(git -C "$root" rev-parse "refs/tags/$tag^{commit}")
    head=$(git -C "$root" rev-parse HEAD)
    dirty=$(git -C "$root" status --porcelain)
    [[ -z $dirty ]] || die "the checkout has uncommitted changes, and recipes must come from a clean HEAD:
$dirty"
    git -C "$root" merge-base --is-ancestor "$commit" "$head" ||
        die "HEAD does not contain $tag"
}

# The recipes must name this release, or a version from elsewhere goes out
# under this tag.
check_recipes() {
    local spec=$recipes/packaging/rpm/wineo.spec changelog=$recipes/packaging/debian/changelog
    local package upstream
    mkdir -p "$recipes"
    git -C "$root" archive HEAD PKGBUILD packaging scripts | tar -x -C "$recipes"
    for package in wineo wineo-bin; do
        [[ $(pkgbuild_field pkgver "$(aur_recipe "$package")") == "$version" ]] ||
            die "packaging/aur/$package/PKGBUILD at HEAD is not at $version"
    done
    [[ $(spec_version "$spec") == "$version" ]] ||
        die "packaging/rpm/wineo.spec at HEAD is not at $version"
    upstream=$(changelog_version "$changelog")
    [[ ${upstream%-*} == "$version" ]] ||
        die "packaging/debian/changelog at HEAD is at $upstream, not $version"
    head -n 1 "$changelog" | grep -q ') UNRELEASED;' ||
        die "the newest packaging/debian/changelog entry is not UNRELEASED"
    [[ -n $(ebuild_revision "$recipes/packaging/gentoo/games-util/wineo/wineo-9999.ebuild") ]] ||
        die "wineo-9999.ebuild has no '# Release revision: N' line"
}

check_assets() {
    local name attached listed sums=$assets/SHA256SUMS failed
    attached=$(jq -r '.assets[].name' "$work/release.json")
    while read -r name; do
        grep -qxF -- "$name" <<<"$attached" || die "the Release for $tag has no $name"
    done < <(expected_assets "$version")

    mkdir -p "$assets"
    while read -r name; do
        gh release download "$tag" -R "$repo" -D "$assets" --clobber --pattern "$name" ||
            die "could not download $name from the Release"
    done < <(expected_assets "$version")

    listed=$(awk '{print $2}' "$sums" | sort)
    [[ $listed == "$(checksummed_assets "$version" | sort)" ]] ||
        die "SHA256SUMS does not list exactly the expected assets:
$listed"
    # --strict fails on a malformed line instead of skipping it.
    if ! failed=$(cd "$assets" && sha256sum --strict -c SHA256SUMS 2>&1); then
        die "an asset does not match SHA256SUMS:
$(grep -v ': OK$' <<<"$failed")"
    fi
}

# On the tag's tree, the exact crate graph the tarball carries.
check_tree() {
    local deny=${CARGO_DENY:-} floor refused
    mkdir -p "$source_tree"
    tar -xf "$assets/wineo-$version-vendored.tar.xz" -C "$source_tree" ||
        die "could not unpack the vendored tarball"
    if [[ -z $deny ]]; then
        "$root/scripts/fetch-cargo-deny.sh" "$work/tools" || die "could not fetch cargo-deny"
        deny=$work/tools/cargo-deny
    fi
    (cd "$source_tree/wineo-$version" && "$deny" --locked check licenses) ||
        die "cargo deny check licenses failed on $tag"

    floor=$("$source_tree/wineo-$version/scripts/rust-floor.sh") || die "rust-floor.sh failed"
    floor=${floor%% *}
    refused=$(targets_below_floor "$floor")
    [[ -z $refused ]] || die "the Rust floor is $floor, above these targets (dest target rust):
$refused
Drop them from the recipes and this script's table, or lower the floor"
    say "Rust floor $floor allows every target"
    rm -rf "$source_tree"
}

remove_work() {
    if [[ -n ${work:-} && -d $work ]]; then
        # The Gentoo container leaves files only its own users can remove.
        if command -v podman >/dev/null; then
            podman unshare rm -rf "$work" 2>/dev/null || true
        fi
        rm -rf "$work"
    fi
}

summarize() {
    local dest result status=0
    echo
    echo "wineo $version:"
    for dest in "${destinations[@]}"; do
        result=$(cat "$work/result-$dest" 2>/dev/null || echo "not run")
        printf '  %-8s %s\n' "$dest" "$result"
        [[ $result != failed* ]] || status=1
    done
    return "$status"
}

main() {
    only='' skip=() dest_dir=''
    (($# >= 1)) || usage
    version=$1
    shift
    [[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || usage
    while (($# > 0)); do
        case $1 in
            --only)
                if (($# < 2)) || ! is_destination "$2" || [[ -n $only ]]; then usage; fi
                only=$2
                shift 2
                ;;
            --skip)
                if (($# < 2)) || ! is_destination "$2"; then usage; fi
                skip+=("$2")
                shift 2
                ;;
            --dest-dir)
                if (($# < 2)) || [[ -z $2 ]]; then usage; fi
                dest_dir=$2
                shift 2
                ;;
            *) usage ;;
        esac
    done
    if [[ -n $only ]] && ((${#skip[@]} > 0)); then usage; fi

    local tool
    for tool in gh git jq curl tar xz sha256sum cargo python3; do
        command -v "$tool" >/dev/null || die "$tool not found"
    done

    tag=v$version
    root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
    if [[ -n $dest_dir ]]; then
        mkdir -p "$dest_dir/control" "$dest_dir/remote"
        dest_dir=$(cd "$dest_dir" && pwd)
        state_dir=$dest_dir/state/$version
    else
        state_dir=${XDG_STATE_HOME:-$HOME/.local/state}/wineo-publish/$version
    fi
    mkdir -p "$state_dir"
    local parent=${WINEO_PUBLISH_WORK:-${dest_dir:-${XDG_CACHE_HOME:-$HOME/.cache}/wineo-publish}}
    mkdir -p "$parent"
    work=$(mktemp -d "$parent/work.XXXXXX")
    trap 'remove_tool_images; remove_work' EXIT
    assets=$work/assets
    recipes=$work/recipes
    source_tree=$work/source

    check_checkout
    check_recipes

    wait_until "$tag on GitHub" "$mirror_limit" check_mirror ||
        die "$tag did not reach GitHub in ${mirror_limit}s; check the Gitea mirror"
    [[ $(cat "$work/github-tag") == "$(git -C "$root" rev-parse "refs/tags/$tag")" ]] ||
        die "the local $tag is not GitHub's (GitHub has $(cat "$work/github-tag"))"
    wait_until "HEAD on GitHub's main" "$mirror_limit" check_head_on_main ||
        die "HEAD ($head) is not on GitHub's main; recipes must be pushed first"
    wait_until "the release run for $tag" "$run_limit" check_run ||
        die "the release run for $tag did not finish in ${run_limit}s: https://github.com/$repo/actions/workflows/release.yml"
    wait_until "the Release for $tag" "$release_limit" check_release ||
        die "there is no Release for $tag after ${release_limit}s"
    [[ $(jq -r .isDraft "$work/release.json") == false ]] ||
        die "the Release for $tag is a draft: $(jq -r .url "$work/release.json")"
    check_assets
    check_tree
    say "the Release for $tag checks out"

    local dest
    for dest in "${destinations[@]}"; do
        if [[ -n $only && $dest != "$only" ]]; then
            record "$dest" "not run (--only $only)"
        elif [[ " ${skip[*]:-} " == *" $dest "* ]]; then
            record "$dest" "not run (--skip)"
        else
            run_destination "$dest"
        fi
    done
    summarize
}

# Sourced by the tests for the parsers above.
if [[ ${BASH_SOURCE[0]} == "$0" ]]; then
    main "$@"
fi
