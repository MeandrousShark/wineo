#!/bin/bash
# Publish a tagged release to the package repositories, from the maintainer's
# machine, with the maintainer's own keys. CI holds no secrets for these.
#
# Usage: scripts/publish-packages.sh VERSION [--only DEST] [--skip DEST]...
#                                    [--dest-dir DIR]
#
# DEST is ppa, overlay, aur, copr, or obs, published in that order. --only and
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
#   WINEO_PUBLISH_PPA_KEY         the signing key's full fingerprint, since
#                                 debsign refuses a short ID (default
#                                 6D463D92CDF5CDEABD40D6301C369B098EE69509)
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
# Without this, a failure inside $(...) doesn't stop the function it's in.
shopt -s inherit_errexit

repo=MeandrousShark/wineo
overlay_repo=MeandrousShark/wineo-overlay
copr_project=meandrousshark/wineo
copr_api=https://copr.fedorainfracloud.org/api_3
copr_token_url=https://copr.fedorainfracloud.org/api/
copr_page_size=100
obs_project=home:meandrousshark
obs_package=wineo
ppa_name=ppa:meandrous-shark/wineo
lp_archive=https://api.launchpad.net/1.0/~meandrous-shark/+archive/ubuntu/wineo
lp_series=https://api.launchpad.net/1.0/ubuntu
fedora_image=registry.fedoraproject.org/fedora:44
ubuntu_image=docker.io/library/ubuntu:24.04
gentoo_image=docker.io/gentoo/stage3:latest

# The PPA goes first because signing asks for the passphrase: the rest needs
# nobody, so whoever started the run can walk away once that's typed.
destinations=(ppa overlay aur copr obs)
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
ppa_key=${WINEO_PUBLISH_PPA_KEY:-6D463D92CDF5CDEABD40D6301C369B098EE69509}

usage() {
    echo "usage: ${0##*/} VERSION [--only DEST] [--skip DEST]... [--dest-dir DIR]" >&2
    echo "DEST: ${destinations[*]}" >&2
    exit 2
}

die() {
    echo "${0##*/}: $*" >&2
    exit 1
}

# say MESSAGE: a progress line. A destination's run also gets a prefixed copy on
# the terminal, through term_fd, while its stdout and stderr go to its log.
say() {
    echo "== $*"
    term_copy "${term_fd:-}" "${1#"${current:-}: "}"
}

# term_copy FD LINE: LINE to the terminal's descriptor, prefixed with the
# destination, in one short write. It never fails: after a hangup the terminal
# is gone, and that must not stop a job. The write is in a subshell so a SIGPIPE
# from a pipe with no reader kills only that. When the terminal is a pipe
# (term_pipe), the destinations have closed their copy of FD, since a daemon
# born in a run would hold it and keep the pipe's reader waiting forever, so
# each line reopens main's descriptor through /proc ($$ is main's PID in
# subshells). A file's offset isn't shared that way, so only pipes do this, and
# only while the number still names a pipe: once main has closed it, bash can
# reuse it for a file it saves a descriptor in, which `>` would truncate.
term_copy() {
    [[ -n $1 ]] || return 0
    if [[ -n ${term_pipe:-} ]]; then
        [[ -p /proc/$$/fd/$1 ]] || return 0
        { (printf '%-9s %s\n' "[$current]" "$2" >"/proc/$$/fd/$1"); } 2>/dev/null || true
    else
        { (printf '%-9s %s\n' "[$current]" "$2" >&"$1"); } 2>/dev/null || true
    fi
}

is_destination() {
    local d
    for d in "${destinations[@]}"; do
        [[ $d != "$1" ]] || return 0
    done
    return 1
}

# Pure helpers. The tests call these with canned input.
#
# The status parsers share one rule. absent is the only state that lets an
# upload go, so it takes a well-formed answer showing the identity isn't there.
# Anything missing, partial, or unexpected is unknown, which stops the
# destination and uploads nothing.

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

# copr_states: the chroots of a known build on stdin, one "target state" line
# per target. forked is a build copied from a fork, so it is a success too. The
# build exists, so a chroot with no entry is an answer that can't be read.
copr_states() {
    local json target name state
    json=$(cat)
    while read -r target; do
        name=$target-x86_64
        state=$(jq -er --arg name "$name" \
            '[.items[] | select(.name == $name) | .state | strings] | first' \
            <<<"$json" 2>/dev/null) || state=unknown
        case $state in
            succeeded | forked) state=built ;;
            failed | canceled | skipped) state=failed ;;
            waiting | pending | importing | starting | running) state=building ;;
            *) state=unknown ;;
        esac
        echo "$target $state"
    done < <(targets_of copr)
}

# True for a Launchpad collection read in full. Results for one exact version
# are small, so a partial one is refused rather than paged through.
lp_complete() {
    jq -e '(.entries | type) == "array" and .start == 0
        and .total_size == (.entries | length)' <<<"$1" >/dev/null 2>&1
}

# lp_source JSON IDENTITY SERIES [PACKAGE] prints absent, or "STATUS LINK" for the source
# of IDENTITY in SERIES. Fails on anything else.
lp_source() {
    lp_complete "$1" || return 1
    jq -er --arg id "$2" --arg series "/$3" --arg package "${4:-wineo}" '
        if .total_size == 0 then "absent"
        elif all(.entries[];
            .source_package_name == $package and .source_package_version == $id
            and (.status | type) == "string" and (.self_link | type) == "string"
            and (.distro_series_link | endswith($series)))
        then "\(.entries[0].status) \(.entries[0].self_link)"
        else error("not the source asked for") end' <<<"$1" 2>/dev/null
}

# lp_state SOURCE_STATUS BUILDS_JSON BINARIES_JSON prints a series' state.
# Only amd64 counts: the recipe is amd64 only, and a PPA can also run
# variants that say nothing about the package users get. A built package
# isn't installable until its binary is published. Launchpad retries a
# dependency wait by itself, but nothing here can fix it, so it stops the wait;
# a rerun sees it if it clears.
lp_state() {
    local status=$1 builds=$2 binaries=$3 states state
    case $status in
        Deleted | Obsolete)
            echo failed
            return
            ;;
        Pending | Published | Superseded) ;;
        *)
            echo unknown
            return
            ;;
    esac
    if ! lp_complete "$builds"; then
        echo unknown
        return
    fi
    states=$(jq -er '[.entries[] | select(.arch_tag == "amd64") | .buildstate] | join("\n")' \
        <<<"$builds" 2>/dev/null) || {
        echo unknown
        return
    }
    if [[ -z $states ]]; then
        echo building
        return
    fi
    if grep -qx 'Successfully built' <<<"$states"; then
        if ! lp_complete "$binaries"; then
            echo unknown
        elif jq -e '[.entries[] | select(.status == "Published")
                | select(.distro_arch_series_link | endswith("/amd64"))] | length > 0' \
            <<<"$binaries" >/dev/null 2>&1; then
            echo built
        else
            echo building
        fi
        return
    fi
    state=building
    while read -r status; do
        case $status in
            'Needs building' | 'Currently building' | 'Gathering build output' | \
                'Uploading build' | 'Cancelling build') ;;
            'Failed to build' | 'Dependency wait' | 'Chroot problem' | 'Failed to upload' | \
                'Cancelled build' | 'Build for superseded Source')
                state=failed
                ;;
            *)
                echo unknown
                return
                ;;
        esac
    done <<<"$states"
    echo "$state"
}

# obs_states DIR IDENTITY: DIR holds what obs_status fetched, one "target
# state" line per target. A repository counts only once it has published, and
# only if its newest job built the source now on the server, since _result can
# still show the last revision's result right after a commit. A missing hash
# means the answer can't be read, so two of them never compare equal. Like osc,
# any value of dirty but "false" counts as dirty.
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
    if not srcmd5:
        emit("unknown")
    results = {r.get("repository"): r for r in ET.fromstring(read("result")).iter("result")}
    for target in targets:
        result = results.get(target)
        status = None if result is None else result.find("status")
        code = "" if status is None else status.get("code")
        jobs = list(ET.fromstring(read(f"jobhistory-{target}")).iter("jobhist"))
        job = jobs[-1] if jobs else None
        published = result is not None and all(
            result.get(field) == "published" for field in ("state", "code")
        )
        if job is not None and not job.get("srcmd5"):
            state = "unknown"
        elif result is None or status is None or result.get("dirty") not in (None, "false"):
            state = "building"
        elif code in ("unresolvable", "broken", "excluded", "disabled"):
            state = "failed"
        elif code not in ("succeeded", "failed", "scheduled", "blocked", "building",
                          "dispatching", "finished", "signing"):
            state = "unknown"
        elif job is None or job.get("srcmd5") != srcmd5:
            state = "building"
        elif code == "succeeded" and job.get("code") in ("succeeded", "unchanged") and published:
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
osc_said_missing() { grep -q 'HTTP Error 404' "$1"; }

# copr-cli's words, read from it against a local server that refused the login.
copr_auth_failed() { grep -qiE 'login invalid|expired|requires API authentication' "$1"; }

# The first target's identity. sed, not head: with pipefail and inherit_errexit
# a writer that head cut off would stop the script.
first_target() { targets_of "$1" | sed -n 1p; }

# The value of FIELD in a .SRCINFO, one per line.
srcinfo_values() { sed -nE "s/^\s*$1 = //p" "$2"; }

# Everything below needs a run: version, root, work, and so on.

record() {
    echo "$2" >"$work/result-$1"
    notify_main
}

# notify_main: wakes main, which then reads every phase and result file again,
# so signals that merge lose nothing. usr1_trapped is set only once main traps
# USR1, which would end it otherwise; main itself never needs waking.
notify_main() {
    [[ -n ${usr1_trapped:-} && $BASHPID != "$$" ]] || return 0
    kill -USR1 "$$" 2>/dev/null || true
}

# set_phase DEST PHASE [LINE]: what DEST's job is doing, for the walking-away
# lines and the interrupted reasons: preparing, uploading (from just before its
# first upload), or waiting, with its latest "waiting on" line. Moved into
# place, so main never reads half of it.
set_phase() {
    printf '%s\n' "$2" ${3:+"$3"} >"$work/phase-$1.new"
    mv -f "$work/phase-$1.new" "$work/phase-$1"
    notify_main
}

# read_phase DEST: DEST's phase and line into phase and phase_line. A job that
# hasn't written one yet is preparing.
read_phase() {
    phase=preparing phase_line=''
    { { read -r phase && read -r phase_line; } <"$work/phase-$1"; } 2>/dev/null || true
}

# Record locks. A lock is a file holding its owner's PID and that process's
# start time, never an open descriptor: one held across a command is inherited
# by everything the command starts, and some of that outlives the run
# (gpg-agent, podman's pause process, an ssh master) and would hold the lock
# forever. flock guards only the short check-and-write of the record, which
# runs nothing but builtins, and is released before anything else runs.

# proc_start PID VAR: PID's start time from /proc into VAR, or fail if it isn't
# running. comm, the second field, can hold spaces and parens, so the fields are
# counted from after the last ")". A zombie keeps its start time until it's
# reaped, so it doesn't count as running. read, not $(<...), so nothing forks
# while the guard is held.
proc_start() {
    local _stat='' _fields
    [[ $1 =~ ^[0-9]+$ ]] || return 1
    { read -r -d '' _stat <"/proc/$1/stat" || [[ -n $_stat ]]; } 2>/dev/null || return 1
    read -ra _fields <<<"${_stat##*) }"
    [[ -n ${_fields[19]:-} && ${_fields[0]} != [ZX] ]] || return 1
    printf -v "$2" %s "${_fields[19]}"
}

# lock_live FILE: the record names a process that is still the one that wrote
# it. An empty or garbled record names nobody.
lock_live() {
    local pid start now
    { read -r pid start <"$1"; } 2>/dev/null || return 1
    [[ $pid =~ ^[0-9]+$ && $start =~ ^[0-9]+$ ]] || return 1
    proc_start "$pid" now || return 1
    [[ $now == "$start" ]]
}

# lock_try FILE OWNER: take the lock for OWNER (a PID) if nobody live holds it.
# Status 1 means held, 2 means OWNER isn't running, 3 means the guard or the
# record couldn't be used. Its callers run it with errexit off (`|| status=$?`),
# so every step that can fail is checked here.
lock_try() {
    local file=$1 owner=$2 fd start taken=0
    proc_start "$owner" start || return 2
    exec {fd}>>"$file.guard" || return 3
    if ! flock "$fd"; then
        exec {fd}>&-
        return 3
    fi
    if ! lock_live "$file"; then
        if echo "$owner $start" >"$file"; then taken=1; else taken=3; fi
    fi
    exec {fd}>&-
    ((taken != 3)) || return 3
    ((taken))
}

# lock_release FILE OWNER: free the lock if OWNER still holds it; status 3 as
# for lock_try. Emptying the record, not removing it, keeps everything under the
# guard a builtin.
lock_release() {
    local file=$1 owner=$2 fd pid='' status=0
    exec {fd}>>"$file.guard" || return 3
    if flock "$fd"; then
        { read -r pid _ <"$file"; } 2>/dev/null || true
        if [[ $pid == "$owner" ]]; then : >"$file" || status=3; fi
    else
        status=3
    fi
    exec {fd}>&-
    return "$status"
}

# lock_hand FILE FROM TO: rewrite the record from FROM (a PID, still the process
# that wrote it) to TO, in one step under the guard, so the lock is never free
# in between. Status 1 means FROM doesn't hold it, 2 that TO isn't running, 3
# that the guard or the record couldn't be used.
lock_hand() {
    local file=$1 from=$2 to=$3 fd pid='' start status=1
    proc_start "$to" start || return 2
    exec {fd}>>"$file.guard" || return 3
    if flock "$fd"; then
        { read -r pid _ <"$file"; } 2>/dev/null || true
        if [[ $pid == "$from" ]] && lock_live "$file"; then
            if echo "$to $start" >"$file"; then status=0; else status=3; fi
        fi
    else
        status=3
    fi
    exec {fd}>&-
    return "$status"
}

# lock_why STATUS OWNER VAR: what a lock_try status of 2 or 3 means, into VAR.
lock_why() {
    case $1 in
        2) printf -v "$3" %s "/proc/$2 couldn't be read" ;;
        *) printf -v "$3" %s "the lock file couldn't be used" ;;
    esac
}

# lock_wait FILE OWNER WHAT: take the lock, saying so once if it has to wait.
lock_wait() {
    local file=$1 owner=$2 what=$3 told=0 status why
    while true; do
        status=0
        lock_try "$file" "$owner" || status=$?
        ((status != 0)) || return 0
        if ((status != 1)); then
            lock_why "$status" "$owner" why
            dest_fail "could not take the lock for $what ($file): $why"
        fi
        if ((!told)); then
            say "$current: waiting for $what" >&2
            told=1
        fi
        sleep "$interval"
    done
}

dest_fail() {
    echo "${0##*/}: $current: $*" >&2
    record "$current" "failed: $*"
    exit 1
}

# The run lock: a destination holds $state_dir/lock-DEST for the whole of its
# run, so a second run that reaches it first doesn't publish it twice. Try once,
# never wait: the other run is publishing it, which this one doesn't confirm.
# Nothing here is called from a trap, which could fire inside the guarded window
# and wait on itself. A run that dies leaves its record behind, stale, and the
# next run takes it over.
held_result='not run (another run is publishing it)'

# lock_dest DEST OWNER: take DEST's run lock for OWNER (a PID). If it isn't
# taken, the result is recorded and shown, and the status is 1.
lock_dest() {
    local dest=$1 owner=$2 status=0 file=$state_dir/lock-$1 why result current=$1
    lock_try "$file" "$owner" || status=$?
    ((status != 0)) || return 0
    if ((status == 1)); then
        result=$held_result
    else
        lock_why "$status" "$owner" why
        result="failed: could not take the run lock ($file): $why"
    fi
    record "$dest" "$result"
    term_copy "$term_out" "$result"
    return 1
}

# unlock_dest DEST OWNER: free DEST's run lock after a normal end. A failure
# changes nothing the run needs: the record goes stale with its owner.
unlock_dest() {
    lock_release "$state_dir/lock-$1" "$2" || true
}

need_tool() {
    command -v "$1" >/dev/null || dest_fail "$1 not found: $2"
}

# Container steps. A step that's needed only once a destination has something
# to do pulls its image then, so a run that skips everything needs no podman.

# container NAME ARG...: podman run, with the container named for this run and
# the destination, so stopping can find what's left. --init gives it a PID 1
# that passes signals on; the kernel drops a signal with no handler that
# reaches a container's own init from outside.
container() {
    local name=wineo-publish-$$-$current-$1
    shift
    nice -n 15 podman run --init --name "$name" "$@"
}

tool_image() {
    local kind=$1 name base packages builder status
    local lock=$work/lock-tools-$kind
    name=localhost/wineo-publish-$$-tools-$kind
    # Checked and built under the lock, so concurrent callers build it once.
    lock_wait "$lock" "$BASHPID" "the ${kind^} tools image"
    # Only ever committed after the install below passed.
    if podman image exists "$name" 2>/dev/null; then
        lock_release "$lock" "$BASHPID" || dest_fail "could not release the lock for the ${kind^} tools image ($lock)"
        echo "$name"
        return
    fi
    builder=wineo-publish-$$-tools-$kind
    # A holder killed mid-build leaves its container behind under this name.
    podman rm -f "$builder" >/dev/null 2>&1 || true
    case $kind in
        fedora)
            base=$fedora_image
            # osc crashes at startup with no git.
            packages='dnf -y install rpm-build copr-cli osc git-core
                command -v rpmbuild copr-cli osc git >/dev/null'
            ;;
        ubuntu)
            base=$ubuntu_image
            packages='export DEBIAN_FRONTEND=noninteractive
                apt-get update
                apt-get install -y --no-install-recommends ca-certificates debhelper \
                    devscripts dpkg-dev dput fakeroot gnupg xz-utils
                command -v dpkg-buildpackage debsign dput gpg >/dev/null'
            ;;
    esac
    say "$current: installing the tools into a $base image" >&2
    status=0
    nice -n 15 podman run --init --name "$builder" -e STEP="tools-$kind" "$base" \
        bash -euo pipefail -c "$packages" >&2 || status=$?
    if ((status == 0)); then
        podman commit -q "$builder" "$name" >/dev/null || status=$?
    fi
    podman rm -f "$builder" >/dev/null 2>&1 || true
    if ((status != 0)); then
        podman rmi -f "$name" >/dev/null 2>&1 || true
        # Freed first so the next caller tries again and fails with its own reason.
        lock_release "$lock" "$BASHPID" || true
        dest_fail "installing the tools into a $base image failed (exit $status); see the log"
    fi
    lock_release "$lock" "$BASHPID" || dest_fail "could not release the lock for the ${kind^} tools image ($lock)"
    echo "$name"
}

remove_tool_images() {
    local kind
    command -v podman >/dev/null || return 0
    for kind in fedora ubuntu; do
        podman rm -f "wineo-publish-$$-tools-$kind" >/dev/null 2>&1 || true
        podman rmi -f "localhost/wineo-publish-$$-tools-$kind" >/dev/null 2>&1 || true
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
    local dest=$1 target state identity
    while read -r target; do
        identity=$(identity_of "$dest" "$target")
        if [[ ! -e $dest_dir/remote/$dest/$target/$identity ]]; then
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
        target=$(first_target "$dest")
        identity=$(identity_of "$dest" "$target")
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
        identity=$(identity_of "$dest" "$target")
        touch "$dest_dir/remote/$dest/$target/$identity"
    done < <(targets_of "$dest")
}

# Each identity is the recipe's, so it comes from HEAD and not from the tag.

identity_of() {
    local dest=$1 target=$2 revision recipe
    case $dest in
        overlay)
            revision=$(ebuild_revision "$recipes/packaging/gentoo/games-util/wineo/wineo-9999.ebuild")
            if [[ $revision == 0 ]]; then echo "$version"; else echo "$version-r$revision"; fi
            ;;
        aur)
            recipe=$(aur_recipe "$target")
            revision=$(pkgbuild_field pkgrel "$recipe")
            echo "$version-$revision"
            ;;
        copr | obs)
            revision=$(spec_release "$recipes/packaging/rpm/wineo.spec")
            echo "$version-$revision"
            ;;
        ppa)
            revision=$(changelog_version "$recipes/packaging/debian/changelog")
            echo "$revision~${target}1"
            ;;
    esac
}

identity_label() {
    local target
    case $1 in
        ppa) changelog_version "$recipes/packaging/debian/changelog" ;;
        *)
            target=$(first_target "$1")
            identity_of "$1" "$target"
            ;;
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
    local file=$work/wineo-$version.gh.tar.gz lock=$work/lock-tag-tarball part status=0
    lock_wait "$lock" "$BASHPID" "GitHub's tag tarball"
    if [[ ! -f $file ]]; then
        # Moved into place only once whole, so an existing file never is a
        # partial one, which the AUR would pin the sha256 of.
        part=$(mktemp "$work/tag-tarball.XXXXXX")
        if [[ -n $dest_dir && -n ${WINEO_PUBLISH_TAG_TARBALL:-} ]]; then
            cp "$WINEO_PUBLISH_TAG_TARBALL" "$part" || status=$?
        else
            curl --proto '=https' --tlsv1.2 -fsSL --retry 3 -o "$part" \
                "https://github.com/$repo/archive/refs/tags/$tag.tar.gz" || status=$?
        fi
        if ((status != 0)); then
            rm -f "$part"
            lock_release "$lock" "$BASHPID" || true
            dest_fail "could not download GitHub's tarball for $tag"
        fi
        chmod 644 "$part"
        mv "$part" "$file"
    fi
    lock_release "$lock" "$BASHPID" || dest_fail "could not release the lock for GitHub's tag tarball ($lock)"
    echo "$file"
}

clone_repo() {
    local url=$1 into=$2
    rm -rf "$into" &&
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

# repo_is_empty CLONE: true for a repository with no refs at all, the one case
# where nothing in the working tree means nothing has been published. A clone
# whose remote HEAD names a missing branch also has an empty tree.
repo_is_empty() { [[ -z $(git -C "$1" for-each-ref) ]]; }

overlay_state() {
    local clone=$1 identity=$2
    if repo_is_empty "$clone"; then
        echo absent
    elif [[ $(cat "$clone/profiles/repo_name" 2>/dev/null) != wineo ]]; then
        echo unknown
    elif [[ -f $clone/games-util/wineo/wineo-$identity.ebuild ]]; then
        echo built
    else
        echo absent
    fi
}

overlay_status() {
    local identity url state
    identity=$(identity_of overlay overlay)
    url=$(overlay_url)
    if ! clone_repo "$url" "$work/overlay/repo"; then
        echo overlay unknown
    else
        state=$(overlay_state "$work/overlay/repo" "$identity")
        echo "overlay $state"
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
    container gentoo-manifest --rm -e STEP=gentoo-manifest -e IDENTITY="$identity" \
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
    ' || dest_fail "gentoo-ebuild.sh, ebuild manifest, or pkgcheck failed; see the log"

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

aur_state() {
    local clone=$1 package=$2 identity=$3 srcinfo=$1/.SRCINFO pkgver pkgrel
    if repo_is_empty "$clone"; then
        echo absent
        return
    fi
    if [[ ! -f $srcinfo ]] || [[ $(srcinfo_values pkgbase "$srcinfo") != "$package" ]]; then
        echo unknown
        return
    fi
    pkgver=$(srcinfo_values pkgver "$srcinfo")
    pkgrel=$(srcinfo_values pkgrel "$srcinfo")
    if [[ ! $pkgver =~ ^[0-9A-Za-z._+]+$ || ! $pkgrel =~ ^[0-9]+$ ]]; then
        echo unknown
    elif [[ $pkgver-$pkgrel == "$identity" ]]; then
        echo built
    else
        echo absent
    fi
}

aur_status() {
    local package identity clone url state
    for package in "${aur_packages[@]}"; do
        identity=$(identity_of aur "$package")
        clone=$work/aur/repos/$package
        mkdir -p "$work/aur/repos"
        url=$(aur_url "$package")
        if ! clone_repo "$url" "$clone"; then
            echo "$package unknown"
            continue
        fi
        state=$(aur_state "$clone" "$package" "$identity")
        echo "$package $state"
    done
}

aur_ready() { need_tool makepkg "it writes .SRCINFO, and comes with Arch's pacman"; }

# check_pins PACKAGE SRCINFO SOURCE SUM: the one source is the expected one, and
# every checksum makepkg sees, whatever its form in the recipe, is SUM. Fails
# with the reason on stdout, so a committed pin can't go out unchecked.
check_pins() {
    local package=$1 srcinfo=$2 source=$3 sum=$4 sources line
    sources=$(srcinfo_values 'source(_[a-z0-9_]+)?' "$srcinfo")
    if [[ $sources != "$source" ]]; then
        echo "$package names the source '${sources//$'\n'/ }', not '$source'"
        return 1
    fi
    line=$(sed -nE 's/^\s*([a-z0-9]+sums(_[a-z0-9_]+)?) = /\1 /p' "$srcinfo")
    if [[ -z $line ]]; then
        echo "$package has no checksum"
        return 1
    fi
    if grep -q SKIP <<<"$line"; then
        echo "$package still has a SKIP checksum after pinning; refusing to publish it"
        return 1
    fi
    if [[ $line != "sha256sums $sum" ]]; then
        echo "$package has a committed checksum that doesn't match its source: '${line//$'\n'/ }', and the file's sha256 is $sum"
        return 1
    fi
}

# Fills in a temporary copy, never the checkout. wineo-git's pkgver() runs on
# the user's machine, but .SRCINFO has to name something better than the 0 the
# repository keeps. Its SKIP is right for a git source, and is the only value
# it can have.
aur_prepare() {
    local package=$1 recipe sum tarball name source why='' values
    local out=$work/aur/out/$package url=https://github.com/$repo
    recipe=$(aur_recipe "$package")
    mkdir -p "$out"
    cp "$recipe" "$out/PKGBUILD"
    case $package in
        wineo)
            tarball=$(tag_tarball)
            sum=$(sha256sum "$tarball" | awk '{print $1}')
            source=wineo-$version.tar.gz::$url/archive/refs/tags/$tag.tar.gz
            ;;
        wineo-bin)
            name=wineo-$version-x86_64-linux.tar.gz
            sum=$(awk -v n="$name" '$2 == n { print $1 }' "$assets/SHA256SUMS")
            source=$url/releases/download/$tag/$name
            ;;
        wineo-git)
            sed -i -E "s/^pkgver=.*/pkgver=$version/" "$out/PKGBUILD"
            cp "$recipes/packaging/aur/wineo/LICENSE" "$out/LICENSE"
            ;;
    esac
    if [[ $package != wineo-git ]]; then
        cp "$(dirname "$recipe")/LICENSE" "$out/LICENSE"
        [[ $sum =~ ^[0-9a-f]{64}$ ]] || dest_fail "no sha256 for $package's source"
        sed -i -E "s/^sha256sums=\('SKIP'\)$/sha256sums=('$sum')/" "$out/PKGBUILD"
    fi
    (cd "$out" && makepkg --printsrcinfo >.SRCINFO) ||
        dest_fail "makepkg --printsrcinfo failed for $package"
    [[ -s $out/.SRCINFO ]] || dest_fail "makepkg wrote an empty .SRCINFO for $package"
    if [[ $package == wineo-git ]]; then
        values=$(srcinfo_values '[a-z0-9]+sums(_[a-z0-9_]+)?' "$out/.SRCINFO")
        [[ $values == SKIP ]] || dest_fail "wineo-git must keep SKIP for its git source, not '$values'"
    else
        why=$(check_pins "$package" "$out/.SRCINFO" "$source" "$sum") || dest_fail "$why"
    fi
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

# copr_page JSON OFFSET LIMIT IDENTITY checks one page of builds, newest first,
# and prints the newest build of IDENTITY, or nothing. Fails on anything that
# isn't the page asked for.
copr_page() {
    [[ -n $1 ]] || return 1
    jq -er --argjson offset "$2" --argjson limit "$3" --arg id "$4" '
        if (.items | type) == "array" and .meta.offset == $offset
            and (.items | length) <= $limit
            and .meta.limit == $limit
            and all(.items[]; (.id | type) == "number"
                and (.source_package | type) == "object" and has("state"))
        then [.items[] | select(.source_package.version == $id) | .id] | max // ""
        else error("not the page asked for") end' <<<"$1" 2>/dev/null
}

# The newest build of the identity, so a resubmission by hand counts. It may be
# past the first page, so a page that can't be read means the state can't be
# known, never that the build isn't there.
copr_status() {
    local identity list build='' states target offset=0 count
    target=$(first_target copr)
    identity=$(identity_of copr "$target")
    while :; do
        list=$(curl -fsS --max-time 60 \
            "$copr_api/build/list?ownername=${copr_project%/*}&projectname=${copr_project#*/}&limit=$copr_page_size&offset=$offset&order=id&order_type=DESC") || list=
        if ! build=$(copr_page "$list" "$offset" "$copr_page_size" "$identity"); then
            targets_of copr | sed 's/$/ unknown/'
            return 0
        fi
        [[ -z $build ]] || break
        count=$(jq '.items | length' <<<"$list")
        if ((count < copr_page_size)); then
            targets_of copr | sed 's/$/ absent/'
            return 0
        fi
        offset=$((offset + copr_page_size))
    done
    states=$(curl -fsS --max-time 60 "$copr_api/build-chroot/list?build_id=$build") || states=
    copr_states <<<"$states"
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
    local top=$work/copr/rpmbuild identity image srpm target
    target=$(first_target copr)
    identity=$(identity_of copr "$target")
    mkdir -p "$top/SOURCES" "$top/SPECS" "$top/SRPMS"
    cp "$assets/wineo-$version-vendored.tar.xz" "$top/SOURCES/"
    cp "$recipes/packaging/rpm/wineo.spec" "$top/SPECS/"
    image=$(tool_image fedora)
    # No dist tag, so the source package is plain VERSION-RELEASE, which is
    # what identifies it on COPR. Each chroot still adds its own.
    container srpm --rm -e STEP=srpm -v "$top:/work/rpmbuild" "$image" \
        bash -euo pipefail -c '
        rpmbuild -bs --define "_topdir /work/rpmbuild" --define "dist %{nil}" \
            /work/rpmbuild/SPECS/wineo.spec
    ' || dest_fail "rpmbuild -bs failed; see the log"
    srpm=$top/SRPMS/wineo-$identity.src.rpm
    [[ -f $srpm ]] || dest_fail "rpmbuild didn't write $(basename "$srpm")"
}

copr_files() {
    local target identity
    target=$(first_target copr)
    identity=$(identity_of copr "$target")
    echo "$work/copr/rpmbuild/SRPMS/wineo-$identity.src.rpm"
}

copr_upload() {
    local srpm config=${XDG_CONFIG_HOME:-$HOME/.config}/copr image output=$work/copr/upload.txt
    local chroots=() target name
    srpm=$(copr_files)
    name=$(basename "$srpm")
    image=$(tool_image fedora)
    while read -r target; do
        chroots+=(--chroot "$target-x86_64")
    done < <(targets_of copr)
    if ! container copr-upload --rm -e STEP=copr-upload -e SRPM="/work/$name" \
        -e PROJECT="$copr_project" -v "$config:/root/.config/copr:ro" \
        -v "$(dirname "$srpm"):/work:ro" "$image" \
        bash -euo pipefail -c 'exec copr-cli build --nowait "$@" "$PROJECT" "$SRPM"' \
        bash "${chroots[@]}" 2>&1 | tee "$output"; then
        if copr_auth_failed "$output"; then
            dest_fail "COPR refused the login. COPR tokens expire: get a new one from $copr_token_url and save it in $config"
        fi
        dest_fail "copr-cli build failed; see the log"
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
    [[ -f $config ]] || dest_fail "$config is missing: create it with [general] apiurl=https://api.opensuse.org and a [https://api.opensuse.org] section holding user, pass, and credentials_mgr_class=osc.credentials.PlaintextConfigFileCredentialsManager, with mode 600. Keyrings can't work in the container."
}

obs_status() {
    local dir=$work/obs/status image identity target config targets
    target=$(first_target obs)
    identity=$(identity_of obs "$target")
    rm -rf "$dir"
    mkdir -p "$dir"
    image=$(tool_image fedora)
    config=$(obs_config)
    targets=$(targets_of obs | tr '\n' ' ')
    if ! container obs-status --rm -e STEP=obs-status -e PROJECT="$obs_project" \
        -e PACKAGE="$obs_package" -e TARGETS="$targets" \
        -v "$config:/root/.config/osc/oscrc:ro" -v "$dir:/status" "$image" \
        bash -euo pipefail -c '
        source_path=/source/$PROJECT/$PACKAGE
        osc api "$source_path" >/status/directory 2>/status/source-error
        osc api "$source_path/wineo.spec" >/status/spec 2>/status/source-error
        osc api "/build/$PROJECT/_result?package=$PACKAGE&arch=x86_64" >/status/result 2>/status/error
        for target in $TARGETS; do
            osc api "/build/$PROJECT/$target/x86_64/_jobhistory?package=$PACKAGE&limit=5" \
                >"/status/jobhistory-$target" 2>/status/error
        done
    ' >/dev/null 2>&1; then
        # Only the source being gone says absent. A 404 further on doesn't, and
        # the result file exists once the source was read.
        if [[ ! -e $dir/result ]] && osc_said_missing "$dir/source-error"; then
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
    local image identity target config
    target=$(first_target obs)
    identity=$(identity_of obs "$target")
    image=$(tool_image fedora)
    config=$(obs_config)
    container obs-upload --rm -e STEP=obs-upload -e PROJECT="$obs_project" \
        -e PACKAGE="$obs_package" -e MESSAGE="Update to $identity" \
        -v "$config:/root/.config/osc/oscrc:ro" -v "$work/obs/files:/files:ro" \
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
    local series identity sources source status link builds binaries state
    for series in $(targets_of ppa); do
        identity=$(identity_of ppa "$series")
        sources=$(curl -fsS -G --max-time 60 "$lp_archive" \
            --data-urlencode ws.op=getPublishedSources \
            --data-urlencode source_name=wineo --data-urlencode exact_match=true \
            --data-urlencode "version=$identity" \
            --data-urlencode "distro_series=$lp_series/$series") || sources=
        if ! source=$(lp_source "$sources" "$identity" "$series"); then
            echo "$series unknown"
        elif [[ $source == absent ]]; then
            echo "$series absent"
        else
            status=${source%% *}
            link=${source#* }
            builds=$(curl -fsS --max-time 60 "$link?ws.op=getBuilds") || builds=
            binaries=$(curl -fsS --max-time 60 "$link?ws.op=getPublishedBinaries") || binaries=
            state=$(lp_state "$status" "$builds" "$binaries")
            echo "$series $state"
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
    container "deb-source-$series" --rm -e STEP=deb-source -e SERIES="$series" -e VERSION="$version" \
        -v "$dir:/work" -w "/work/wineo-$version" "$image" \
        bash -euo pipefail -c 'dpkg-buildpackage -S -sa -d -us -uc' ||
        dest_fail "dpkg-buildpackage -S failed for $series; see the log"
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
    local dput_args=("$ppa_name") tty_name
    identity=$(identity_of ppa "$series")
    if [[ $mode == simulate ]]; then dput_args=(--simulate "$ppa_name"); fi
    gpg --export --armor "$ppa_key" >"$key"
    [[ -s $key ]] || dest_fail "gpg exported nothing for $ppa_key"
    gpgconf --launch gpg-agent
    # The container has no terminal for pinentry, so the passphrase is asked
    # for here, and the agent holds it for the signing in there.
    if tty_name=$(tty 2>/dev/null); then export GPG_TTY=$tty_name; fi
    gpg --local-user "$ppa_key" --clear-sign </dev/null >/dev/null ||
        dest_fail "gpg could not unlock $ppa_key; see the log"
    socket=$(gpgconf --list-dirs agent-socket)
    image=$(tool_image ubuntu)
    container "deb-sign-$series" --rm -e STEP=deb-sign -e KEY="$ppa_key" \
        -e CHANGES="wineo_${identity}_source.changes" \
        -v "$dir:/work" -v "$key:/signing-key.asc:ro" \
        -v "$socket:/root/.gnupg/S.gpg-agent" -w /work "$image" \
        bash -euo pipefail -c '
        chmod 700 /root/.gnupg
        gpg --batch --quiet --import /signing-key.asc
        debsign --no-conf -k"$KEY" "$CHANGES"
        exec dput "$@" "$CHANGES"
    ' bash "${dput_args[@]}" || dest_fail "signing or dput failed for $series; see the log"
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
marker() {
    local identity
    identity=$(identity_of "$1" "$2")
    echo "$state_dir/$1-$2-$identity"
}

# Waits until every target is built. Failed builds and timeouts both end the
# destination, and neither lets a rerun upload again.
wait_for_builds() {
    local dest=$1 limit=$2 link=$3 start=$SECONDS line target state failed waiting last='' hint
    local built total states
    total=$(targets_of "$dest" | wc -l)
    while :; do
        # First, since the caller has just read the status or uploaded. Every
        # poll stays a full interval from the one before.
        sleep "$build_interval"
        failed=() waiting=() built=0
        # A variable, not < <(...): a failure in there would go unseen.
        states=$(status_of "$dest")
        while read -r target state; do
            [[ -n $target ]] || continue
            case $state in
                built) built=$((built + 1)) ;;
                failed) failed+=("$target") ;;
                *) waiting+=("$target($state)") ;;
            esac
        done <<<"$states"
        if ((${#failed[@]} > 0)); then
            dest_fail "the build failed on ${failed[*]} ($link). Fix the recipe, bump its revision, and run --only $dest"
        fi
        if ((built == total)); then
            return 0
        fi
        line=${waiting[*]:-its status}
        if [[ $line != "$last" ]]; then
            say "$dest: waiting on $line"
            set_phase "$dest" waiting "waiting on $line"
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

# build_link DEST: where DEST's remote builds show, and how long to wait on
# them, into the caller's link and limit.
build_link() {
    case $1 in
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
}

# The upload part: the status, then everything prepared, then the uploads. It
# records a result only when there's nothing left to wait for, which is how the
# wait part knows.
publish_upload() {
    local dest=$1 target state label all_built=1 failed=() unknown=() absent=() pending=()
    local limit link total=0 unit status states expected marked
    current=$dest
    set_phase "$dest" preparing
    label=$(identity_label "$dest")
    if [[ $dest == obs ]]; then obs_ready; fi

    states=$(status_of "$dest")
    while read -r target state; do
        [[ -n $target ]] || continue
        total=$((total + 1))
        [[ $state == built ]] || all_built=0
        case $state in
            failed) failed+=("$target") ;;
            unknown) unknown+=("$target") ;;
            absent) absent+=("$target") ;;
        esac
    done <<<"$states"

    if ((${#unknown[@]} > 0)); then
        # Never upload on a guess: an unknown state may hide a real build.
        case $dest in
            overlay) dest_fail "could not read $overlay_repo; check 'gh auth status' and that the repository exists" ;;
            aur) dest_fail "could not read ${unknown[*]} on the AUR; the account needs an SSH key at https://aur.archlinux.org/account/ (run --skip aur until there is one)" ;;
            *) dest_fail "could not read the build state of ${unknown[*]}; try again" ;;
        esac
    fi
    # Too few lines mean a status that broke, which must not read as built.
    expected=$(targets_of "$dest" | wc -l)
    if ((total != expected)); then
        dest_fail "could not read the build state; try again"
    fi
    if ((all_built)); then
        say "$dest: $label is already published and built, skipping"
        record "$dest" "already published ($label)"
        return 0
    fi
    build_link "$dest"
    if ((${#failed[@]} > 0)); then
        dest_fail "$label already failed to build on ${failed[*]} ($link). Fix the recipe, bump its revision, and run --only $dest"
    fi

    for target in ${absent[@]+"${absent[@]}"}; do
        marked=$(marker "$dest" "$target")
        if ! has_remote_build "$dest" || [[ ! -e $marked ]]; then
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
        set_phase "$dest" uploading
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
                [[ -s $work/result-$dest ]] || record "$dest" "failed: the upload of $unit failed; see the log"
                exit 1
            fi
            # Something went out in this run, for "every upload is out".
            : >"$work/uploaded-$dest"
            if has_remote_build "$dest"; then
                while read -r target; do
                    [[ $unit == all || $unit == "$target" ]] || continue
                    marked=$(marker "$dest" "$target")
                    : >"$marked"
                done < <(targets_of "$dest")
            fi
        done
    fi

}

# The wait part. Run in a job of its own for the PPA, so it sets everything
# dest_fail and the wait need again.
publish_wait() {
    local dest=$1 label limit link
    current=$dest
    [[ ! -s $work/result-$dest ]] || return 0
    label=$(identity_label "$dest")
    build_link "$dest"
    if has_remote_build "$dest"; then
        set_phase "$dest" waiting
        wait_for_builds "$dest" "$limit" "$link"
    fi
    say "$dest: published $label"
    record "$dest" "published ($label)"
}

# Two statements, never `&&` or inside an `if`: either turns errexit off for
# all of it.
publish_destination() {
    publish_upload "$1"
    publish_wait "$1"
}

# The log directory is made when the first destination starts, so a run the
# checks refuse leaves none. The stamp is to the second and the PID is this
# script's, so two runs never share one.
make_log_dir() {
    [[ -z $log_dir ]] || return 0
    mkdir -p "$state_dir/logs"
    log_dir=$state_dir/logs/$(date +%Y%m%d-%H%M%S)-$$
    mkdir -m 700 "$log_dir"
}

# A destination's own copy of the terminal descriptor, closed when the terminal
# is a pipe, so nothing the destination starts can hold it. See term_copy.
drop_term_out() {
    [[ -z $term_pipe ]] || exec {term_out}>&-
}

# Every destination's output, containers included, goes to its own log. The
# PPA's foreground stage and --only also show it all on the terminal, so a
# failed step sits next to the passphrase prompt or the command being retried.
# The background jobs show only their say lines, prefixed, and a failure's
# reason, through term_out.

# run_inline DEST PART: PART (publish_upload or publish_destination) of DEST in
# the foreground, tee'd to its log.
run_inline() {
    local dest=$1 part=$2 status
    current=$dest
    make_log_dir
    # Not in an `if`: that would turn errexit off for all of it.
    set +e
    # Relies on daemons detaching fds 0-2 (gpg, ssh, and podman do): one that
    # didn't would hold tee open after the destination ends.
    (
        set -e
        term_fd=''
        drop_term_out
        echo "== $dest"
        "$part" "$dest"
    ) 2>&1 | tee -a "$log_dir/$dest.log"
    status=${PIPESTATUS[0]}
    set -e
    if ((status != 0)) && [[ ! -s $work/result-$dest ]]; then
        record "$dest" "failed: it stopped early; see the log"
    fi
    current=''
}

# background_env: nothing in the background can prompt, so a missing credential
# fails at once instead of hanging. For background jobs only, so --only aur can
# still ask for the key's passphrase.
background_env() {
    export GIT_TERMINAL_PROMPT=0 GIT_SSH_COMMAND='ssh -o BatchMode=yes'
}

# run_job DEST PART: the body of a background job. Only main writes a reason
# for a job that ended without a result, since a killed job can't.
run_job() {
    local dest=$1 part=$2 result status=0
    current=$dest
    background_env
    # The lock first, as this job's own PID. The PPA's wait takes over main's,
    # which main took for its foreground stage, so nothing can slip in between.
    if [[ $part == publish_wait ]]; then
        lock_hand "$state_dir/lock-$dest" "$$" "$BASHPID" || status=$?
        if ((status == 1)); then
            result="failed: the run lock was lost before the wait started"
            record "$dest" "$result"
            term_copy "$term_out" "$result"
            return 0
        elif ((status != 0)); then
            lock_why "$status" "$BASHPID" result
            result="failed: could not take over the run lock ($state_dir/lock-$dest): $result"
            record "$dest" "$result"
            term_copy "$term_out" "$result"
            return 0
        fi
    else
        lock_dest "$dest" "$BASHPID" || return 0
    fi
    set +e
    (
        set -e
        term_fd=$term_out
        drop_term_out
        [[ $part == publish_wait ]] || echo "== $dest"
        "$part" "$dest"
    ) >>"$log_dir/$dest.log" 2>&1
    set -e
    unlock_dest "$dest" "$BASHPID"
    result=$(cat "$work/result-$dest" 2>/dev/null || true)
    [[ $result != failed* ]] || term_copy "$term_out" "$result"
}

# launch DEST PART: DEST in the background, in a process group of its own,
# which the terminal's signals don't reach and which doesn't ignore SIGINT, as
# a job started with job control off would. Job control is on only for the
# launch, so main's own commands and wait behave as before.
launch() {
    set -m
    run_job "$1" "$2" </dev/null &
    set +m
    job_dest[$!]=$1
    proc_start "$!" "job_start[$!]" || true
    live+=("$!")
}

# job_alive PID: PID is still the job launched under it. Neither wait -n nor
# bash's own job table can say when a job has ended: one that exits just as a
# trapped USR1 interrupts wait -n can be reaped with its status lost, and then
# stays "Running" in the table, where `wait PID` would block on it forever.
# The start time keeps a reused PID from passing.
job_alive() {
    local now
    proc_start "$1" now && [[ $now == "${job_start[$1]:-}" ]]
}

# end_group PID: TERM to PID's process group, a few seconds for it to empty,
# then SIGKILL. Only for a group that still has a member, which keeps its ID
# from being reused.
end_group() {
    local i
    kill -0 -- "-$1" 2>/dev/null || return 0
    kill -TERM -- "-$1" 2>/dev/null || true
    for ((i = 0; i < 50; i++)); do
        kill -0 -- "-$1" 2>/dev/null || return 0
        sleep 0.1 || true
    done
    kill -KILL -- "-$1" 2>/dev/null || true
}

# sweep_containers PATTERN: stops the containers whose names match PATTERN and
# removes them all. -a, since a container whose podman run was killed can be
# left created or exited as well as running.
sweep_containers() {
    local name state running=() all=()
    command -v podman >/dev/null || return 0
    while read -r name state; do
        [[ -n $name ]] || continue
        all+=("$name")
        [[ $state != running ]] || running+=("$name")
    done < <(podman ps -a --filter "name=$1" --format '{{.Names}} {{.State}}' 2>/dev/null || true)
    if ((${#running[@]} > 0)); then
        podman stop -t 5 "${running[@]}" >/dev/null 2>&1 || true
    fi
    if ((${#all[@]} > 0)); then
        podman rm -f "${all[@]}" >/dev/null 2>&1 || true
    fi
}

# job_done PID: the job is reaped, so it's no longer stopped through its group.
job_done() {
    local pid=$1 dest=${job_dest[$1]} current p rest=()
    # Whatever is left in the job's group: what a job that published left
    # behind would outlive the run and its work directory, and if only the
    # job's own process died (kill PID, the OOM killer), the rest would go on
    # publishing and could write its own result over main's. Now, not later:
    # a group with a member has kept its ID since the leader exited, at most a
    # tick ago, while one that emptied long since may have had it reused. Still
    # in live while this runs, so a Ctrl+C now stops its group too.
    end_group "$pid"
    if [[ ! -s $work/result-$dest ]]; then
        sweep_containers "^wineo-publish-$$-$dest-"
        current=$dest
        record "$dest" "failed: it stopped early; see the log"
        term_copy "$term_out" "failed: it stopped early; see the log"
    fi
    for p in "${live[@]}"; do
        [[ $p == "$pid" ]] || rest+=("$p")
    done
    live=(${rest[@]+"${rest[@]}"})
    done_with[$pid]=1
}

# tell LINE...: main's own lines while jobs run. Like term_copy, it never fails:
# after a hangup, or with a pipe that lost its reader, the write does.
tell() { { (printf '%s\n' "$@"); } 2>/dev/null || true; }

# walk_away: once every upload is out, stopping loses nothing, since the remote
# builds go on and a rerun goes straight back to waiting. Each line prints at
# most once, from the files alone, however many wake-ups came, and only while
# something is still waiting: with nothing left, the summary is next anyway.
walk_away() {
    local dest result phase phase_line left=() out=1 down='' lines=()
    for dest in "${torun[@]}"; do
        result=''
        { read -r result <"$work/result-$dest"; } 2>/dev/null || true
        if [[ -n $result ]]; then
            [[ $result != failed* ]] || down+=${down:+, }$dest
            continue
        fi
        left+=("$dest")
        read_phase "$dest"
        [[ $phase == waiting ]] || out=0
    done
    if ((!told_out && out && ${#left[@]} > 0)) && compgen -G "$work/uploaded-*" >/dev/null; then
        told_out=1
        tell "== every upload is out${down:+ ($down failed)}; stopping now loses nothing, rerun to keep waiting"
    fi
    if ((told_left || ${#left[@]} != 1 || ${#ran[@]} < 2)); then return 0; fi
    read_phase "${left[0]}"
    [[ $phase == waiting ]] || return 0
    told_left=1
    lines=("== only the ${left[0]}'s wait is left:")
    for dest in "${ran[@]}"; do
        [[ $dest != "${left[0]}" ]] || continue
        result=''
        { read -r result <"$work/result-$dest"; } 2>/dev/null || true
        lines+=("$(printf '  %-8s %s' "$dest" "$result")")
    done
    lines+=("$(printf '  %-8s %s' "${left[0]}" "${phase_line:-waiting}")")
    tell "${lines[@]}"
}

# interrupted_reason DEST: a reason from how far DEST got.
interrupted_reason() {
    local phase phase_line
    read_phase "$1"
    case $phase in
        waiting) echo "failed: interrupted while waiting; uploads are out, rerun --only $1 to keep waiting" ;;
        uploading) echo "failed: interrupted during upload; rerun --only $1, and what finished uploading isn't sent again" ;;
        *) echo "failed: interrupted before it uploaded" ;;
    esac
}

# stop_all: ends every job still running, the whole of its process group, then
# any of this run's containers that are left, and records a reason for every
# destination that has none. It runs once, so a second Ctrl+C or EXIT after
# INT does nothing, and with INT, TERM, and HUP ignored, which what it runs
# inherits. It prints nothing, and it never takes a record lock: a trap that
# fired inside a lock's guarded window would wait on itself.
stop_all() {
    local pid running=() signaled=() i busy dest old_ticker
    ((!stopping)) || return 0
    stopping=1
    trap '' INT TERM HUP
    # A signal during launch can land with job control still on.
    set +m
    old_ticker=$ticker
    stop_ticker
    # A job a signal caught between a launch's `&` and its PID reaching live is
    # only in bash's table, which lists it as running. It has only just started,
    # so its PID can't have been reused. Everything else in the table that isn't
    # live is a job or ticker main has finished with, whose status bash may have
    # lost (see job_alive): its PID may be someone else's by now.
    read -ra running <<<"$(jobs -rp 2>/dev/null | tr '\n' ' ')"
    for pid in ${running[@]+"${running[@]}"}; do
        [[ $pid != "$old_ticker" && -z ${done_with[$pid]:-} && " ${live[*]:-} " != *" $pid "* ]] || continue
        stopped_jobs=1
        kill -TERM -- "-$pid" 2>/dev/null || true
        signaled+=("$pid")
    done
    # The rest by what's really running: a job that's still itself, or one
    # whose own process is gone but whose group isn't, as in job_done. A
    # group with a member keeps its ID from being reused.
    for pid in ${live[@]+"${live[@]}"}; do
        if job_alive "$pid" || kill -0 -- "-$pid" 2>/dev/null; then
            stopped_jobs=1
            kill -TERM -- "-$pid" 2>/dev/null || true
            signaled+=("$pid")
        fi
    done
    # Each group gets a few seconds to empty, then SIGKILL.
    for ((i = 0; i < 50 && ${#signaled[@]} > 0; i++)); do
        busy=()
        for pid in "${signaled[@]}"; do
            if kill -0 -- "-$pid" 2>/dev/null; then busy+=("$pid"); fi
        done
        signaled=(${busy[@]+"${busy[@]}"})
        ((${#signaled[@]} > 0)) || break
        sleep 0.1 || true
    done
    for pid in ${signaled[@]+"${signaled[@]}"}; do
        kill -KILL -- "-$pid" 2>/dev/null || true
    done
    # Once no client is left to answer the signal and remove its own.
    if ((stopped_jobs || interrupted)); then sweep_containers "^wineo-publish-$$-"; fi
    # No wait: the groups are gone, bash reaps what's left, and a job whose
    # status it lost would block it for good.
    live=()
    for dest in ${torun[@]+"${torun[@]}"}; do
        [[ ! -s $work/result-$dest ]] || continue
        if ((interrupted)); then
            record "$dest" "$(interrupted_reason "$dest")"
        else
            record "$dest" "failed: it stopped early; see the log"
        fi
    done
}

# close_term: main's copy of the terminal, closed once nothing writes to it, so
# nothing the cleanup starts (podman unshare) can hold a pipe open.
close_term() {
    if [[ -n $term_out ]]; then
        { exec {term_out}>&-; } 2>/dev/null || true
        term_out=''
    fi
}

# Ctrl+C, a hangup, or TERM: stop everything, then say how far each got.
on_signal() {
    ((!stopping)) || return 0
    interrupted=1
    stop_all
    close_term
    { (summarize); } 2>/dev/null || true
    exit 1
}

# The cleanup comes after the stopping, so nothing still running loses the
# tool images or the work directory under it.
on_exit() {
    # Before anything else, so an INT can't run on_signal in the middle of this
    # and skip the cleanup.
    trap '' INT TERM HUP
    if ((!stopping)); then
        stop_all
        if ((stopped_jobs)); then
            close_term
            { (summarize); } 2>/dev/null || true
        fi
    fi
    close_term
    remove_tool_images
    remove_work
}

# The ticker wakes main every 10 seconds. A USR1 that comes after the woke
# check but before wait -n blocks is otherwise lost until the next one, which
# can be a whole build interval away. It's in main's process group, so it's
# never one of the jobs.
start_ticker() {
    sleep 10 &
    ticker=$!
    proc_start "$ticker" "job_start[$ticker]" || true
}

# Not waited for, which could block for good as job_alive says.
stop_ticker() {
    [[ -n $ticker ]] || return 0
    if job_alive "$ticker"; then kill "$ticker" 2>/dev/null || true; fi
    done_with[$ticker]=1
    ticker=''
}

# reap_ended: job_done for every live job that has ended, and a new ticker if
# the last one has. Each is waited for only once bash lists it as ended, since
# one whose status it lost would block wait for good.
reap_ended() {
    local pid running
    running=" $(jobs -rp 2>/dev/null | tr '\n' ' ') "
    for pid in "${live[@]}"; do
        ! job_alive "$pid" || continue
        [[ $running == *" $pid "* ]] || wait "$pid" 2>/dev/null || true
        job_done "$pid"
    done
    if ! job_alive "$ticker"; then
        [[ $running == *" $ticker "* ]] || wait "$ticker" 2>/dev/null || true
        done_with[$ticker]=1
        start_ticker
    fi
}

# The three stages: the PPA's foreground stage, then everything else and the
# PPA's wait in the background, then the wait for them all.
fan_out() {
    local dest
    if [[ " ${torun[*]} " == *" ppa "* ]]; then
        ran+=(ppa)
        # Main's, until the wait job takes it over. With nothing left to wait
        # for, main lets it go itself.
        if lock_dest ppa "$$"; then
            run_inline ppa publish_upload
            if [[ -s $work/result-ppa ]]; then unlock_dest ppa "$$"; fi
        fi
    fi
    for dest in "${torun[@]}"; do
        if [[ $dest != ppa ]]; then
            ran+=("$dest")
            launch "$dest" publish_destination
        elif [[ ! -s $work/result-ppa ]]; then
            launch ppa publish_wait
        fi
    done
    start_ticker
    while :; do
        woke=0
        reap_ended
        ((${#live[@]} > 0)) || break
        walk_away
        # A wake-up during walk_away would otherwise wait for the next one.
        if ((woke)); then continue; fi
        # Only a way to sleep until something happens: reap_ended says what.
        wait -n "${live[@]}" "$ticker" 2>/dev/null || true
    done
    stop_ticker
    walk_away
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
    # Identities are built from these, so an empty one must stop here.
    for package in "${aur_packages[@]}"; do
        [[ $(pkgbuild_field pkgrel "$(aur_recipe "$package")") =~ ^[0-9]+$ ]] ||
            die "the $package recipe at HEAD has no pkgrel"
    done
    [[ $(spec_release "$spec") =~ ^[0-9]+$ ]] || die "packaging/rpm/wineo.spec at HEAD has no Release"
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
        if [[ $result != published* && $result != "already published"* && -n $log_dir && -f $log_dir/$dest.log ]]; then
            printf '  %-8s log: %s\n' '' "$log_dir/$dest.log"
        fi
        [[ $result != failed* && $result != "$held_result" ]] || status=1
    done
    [[ -z $log_dir ]] || echo "logs: $log_dir/"
    return "$status"
}

main() {
    only='' skip=() dest_dir='' log_dir='' term_out='' term_fd='' term_pipe='' current=''
    torun=() ran=() live=() stopping=0 interrupted=0 stopped_jobs=0
    woke=0 told_out=0 told_left=0 usr1_trapped='' ticker=''
    declare -gA job_dest=() job_start=() done_with=()
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
    for tool in gh git jq curl tar xz sha256sum cargo python3 flock; do
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
    trap on_exit EXIT
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

    # A copy of the terminal for the destinations' say lines, taken before any
    # of them has its stdout sent to a log. Not /dev/tty: a run in CI has none.
    exec {term_out}>&1
    # A named FIFO isn't a pipe here: opening one through /proc blocks while it
    # has no reader, where an anonymous pipe's open doesn't.
    local target dest
    target=$(readlink "/proc/$$/fd/$term_out" 2>/dev/null) || target=''
    if [[ $target == pipe:* ]]; then term_pipe=1; fi
    for dest in "${destinations[@]}"; do
        if [[ -n $only && $dest != "$only" ]]; then
            record "$dest" "not run (--only $only)"
        elif [[ " ${skip[*]:-} " == *" $dest "* ]]; then
            record "$dest" "not run (--skip)"
        else
            torun+=("$dest")
        fi
    done
    # Before anything can send a USR1, which would end main untrapped.
    trap 'woke=1' USR1
    usr1_trapped=1
    trap on_signal INT TERM HUP
    if ((${#torun[@]} > 0)); then make_log_dir; fi
    if [[ -n $only ]]; then
        # Main owns it, since the destination runs inline in this process's
        # pipeline, as the PPA's foreground stage does.
        if lock_dest "$only" "$$"; then
            run_inline "$only" publish_destination
            unlock_dest "$only" "$$"
        fi
    else
        fan_out
    fi
    # Only once every job is reaped, since they write through it. podman
    # unshare in the EXIT trap can leave a process that would hold a pipe.
    close_term
    summarize
}

# Sourced by the tests for the parsers above.
if [[ ${BASH_SOURCE[0]} == "$0" ]]; then
    main "$@"
fi
