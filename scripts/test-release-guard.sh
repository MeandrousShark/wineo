#!/bin/bash
# Runs scripts/release-guard.sh against throwaway git repositories, one case
# per refusal plus the cases that must pass, and checks the two helper
# scripts too. Nothing here touches the real repository, the network, or
# GitHub: the Release list comes from RELEASE_GUARD_RELEASES, except in the
# last guard cases, which run the real `gh` code path against a fake `gh`.
#
# Usage: scripts/test-release-guard.sh
# Exits non-zero if any case behaves wrongly.
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
guard=$script_dir/release-guard.sh
section=$script_dir/changelog-section.sh
packages=$script_dir/readme-runtime-packages.sh

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null

failures=0
cases=0
repo_count=0

pass() { cases=$((cases + 1)); }
fail() {
    cases=$((cases + 1))
    failures=$((failures + 1))
    echo "FAIL: $*" >&2
}

# new_repo CARGO_VERSION CHANGELOG_VERSION
# Makes a repo whose main branch has a parent commit and a tip commit, and
# sets $repo and $tip. CHANGELOG_VERSION "" leaves the section out.
new_repo() {
    repo_count=$((repo_count + 1))
    repo=$tmp/repo$repo_count
    git init -q -b main "$repo"
    echo base >"$repo/base.txt"
    git -C "$repo" add base.txt
    git -C "$repo" commit -q -m base
    parent=$(git -C "$repo" rev-parse HEAD)
    cat >"$repo/Cargo.toml" <<TOML
[workspace]
members = []

[workspace.package]
version = "$1"
edition = "2024"

[workspace.dependencies]
version = "${2:-9.9.9}"
TOML
    {
        echo "# Changelog"
        echo
        if [[ -n $2 ]]; then
            echo "## $2 - 2026-01-01"
            echo
            echo "Notes for $2."
            echo
        fi
        echo "## 0.1.0 - 2025-01-01"
        echo
        echo "Old notes."
    } >"$repo/CHANGELOG.md"
    git -C "$repo" add Cargo.toml CHANGELOG.md
    git -C "$repo" commit -q -m release
    tip=$(git -C "$repo" rev-parse HEAD)
}

# run_guard TAG SHA RELEASES [EVENT]
# Runs the guard in $repo as a push of refs/tags/TAG at SHA.
run_guard() {
    local tag=$1 sha=$2 releases=$3 event=${4:-push}
    GITHUB_EVENT_NAME=$event GITHUB_REF=refs/tags/$tag GITHUB_SHA=$sha \
        RELEASE_GUARD_REPO=$repo RELEASE_GUARD_MAIN_REF=main \
        RELEASE_GUARD_RELEASES=$releases \
        "$guard" >"$tmp/out" 2>&1 && status=0 || status=$?
}

# run_guard_gh TAG SHA
# Like run_guard, but without RELEASE_GUARD_RELEASES, so the guard calls
# `gh` from the fake bin directory.
run_guard_gh() {
    env -u RELEASE_GUARD_RELEASES \
        PATH="$tmp/bin:$PATH" GH_REPO=owner/repo \
        GITHUB_EVENT_NAME=push GITHUB_REF=refs/tags/"$1" GITHUB_SHA="$2" \
        RELEASE_GUARD_REPO="$repo" RELEASE_GUARD_MAIN_REF=main \
        "$guard" >"$tmp/out" 2>&1 && status=0 || status=$?
}

# expect_refusal NAME REASON_SUBSTRING (after run_guard)
expect_refusal() {
    if [[ $status -ne 1 ]]; then
        fail "$1: expected exit 1, got $status ($(cat "$tmp/out"))"
    elif [[ $(wc -l <"$tmp/out") -ne 1 ]]; then
        fail "$1: reason is not one line: $(cat "$tmp/out")"
    elif ! grep -qF -- "$2" "$tmp/out"; then
        fail "$1: reason lacks '$2': $(cat "$tmp/out")"
    else
        pass
    fi
}

expect_ok() {
    if [[ $status -eq 0 ]]; then
        pass
    else
        fail "$1: expected to pass, got $status ($(cat "$tmp/out"))"
    fi
}

annotate() { git -C "$repo" tag -a -m "tag $1" "$1" "${2:-$tip}"; }

# Correct v1.0.0.
new_repo 1.0.0 1.0.0
annotate v1.0.0
run_guard v1.0.0 "$tip" ""
expect_ok "correct v1.0.0"

# Releases that are lower or not versions at all don't block it.
run_guard v1.0.0 "$tip" $'v0.39.1\nv0.9.0\nnightly'
expect_ok "lower Releases"

# Not a tag push.
run_guard v1.0.0 "$tip" "" workflow_dispatch
expect_refusal "workflow_dispatch" "not a tag push"
GITHUB_EVENT_NAME=push GITHUB_REF=refs/heads/main GITHUB_SHA=$tip \
    RELEASE_GUARD_REPO=$repo RELEASE_GUARD_MAIN_REF=main RELEASE_GUARD_RELEASES="" \
    "$guard" >"$tmp/out" 2>&1 && status=0 || status=$?
expect_refusal "branch ref" "not a vX.Y.Z tag"

# Not a version.
new_repo 1.0.0 1.0.0
annotate v-test
run_guard v-test "$tip" ""
expect_refusal "v-test" "not a vX.Y.Z tag"

# Leading zero.
new_repo 1.2.3 1.2.3
annotate v01.2.3
run_guard v01.2.3 "$tip" ""
expect_refusal "v01.2.3" "not a vX.Y.Z tag"

# Prerelease suffix.
new_repo 1.0.0 1.0.0
annotate v1.0.0-rc1
run_guard v1.0.0-rc1 "$tip" ""
expect_refusal "v1.0.0-rc1" "not a vX.Y.Z tag"

# Below 1.0.0, even with everything else right.
new_repo 0.39.1 0.39.1
annotate v0.39.1
run_guard v0.39.1 "$tip" ""
expect_refusal "below 1.0.0" "below 1.0.0"

# Lightweight tag.
new_repo 1.0.0 1.0.0
git -C "$repo" tag v1.0.0
run_guard v1.0.0 "$tip" ""
expect_refusal "lightweight" "not annotated"

# Tag not at this run's commit.
new_repo 1.0.0 1.0.0
annotate v1.0.0
run_guard v1.0.0 "$parent" ""
expect_refusal "other commit" "not at this run's commit"
run_guard v1.0.0 "${tip:0:12}" ""
expect_refusal "short sha" "full commit hash"

# Commit not on main.
new_repo 1.0.0 1.0.0
git -C "$repo" checkout -q -b side
echo side >"$repo/side.txt"
git -C "$repo" add side.txt
git -C "$repo" commit -q -m side
side=$(git -C "$repo" rev-parse HEAD)
git -C "$repo" checkout -q main
annotate v1.0.0 "$side"
run_guard v1.0.0 "$side" ""
expect_refusal "not on main" "is not on main"

# Cargo.toml version differs from the tag. The decoy version under
# [workspace.dependencies] matches the tag and must not count.
new_repo 1.0.1 1.0.0
annotate v1.0.0
run_guard v1.0.0 "$tip" ""
expect_refusal "Cargo mismatch" "does not match tag"

# Cargo.toml in the working tree is irrelevant: the tagged commit decides.
new_repo 1.0.0 1.0.0
annotate v1.0.0
sed -i 's/^version = "1.0.0"/version = "7.0.0"/' "$repo/Cargo.toml"
run_guard v1.0.0 "$tip" ""
expect_ok "tagged Cargo.toml decides, not the working tree"

# No changelog section.
new_repo 1.0.0 ""
annotate v1.0.0
run_guard v1.0.0 "$tip" ""
expect_refusal "no changelog" "no '## 1.0.0 - ' section"

# A heading without the " - " separator doesn't count.
new_repo 1.0.0 ""
printf '## 1.0.0\n\nText.\n' >>"$repo/CHANGELOG.md"
git -C "$repo" commit -q -am "heading without date"
tip=$(git -C "$repo" rev-parse HEAD)
annotate v1.0.0
run_guard v1.0.0 "$tip" ""
expect_refusal "bare heading" "no '## 1.0.0 - ' section"

# A Release already exists for the tag.
new_repo 1.0.0 1.0.0
annotate v1.0.0
run_guard v1.0.0 "$tip" $'v0.9.0\nv1.0.0'
expect_refusal "Release exists" "already exists"

# Not higher than the newest Release.
new_repo 1.1.0 1.1.0
annotate v1.1.0
run_guard v1.1.0 "$tip" $'v1.0.0\nv1.2.0'
expect_refusal "not higher" "not higher than the existing Release v1.2.0"

# Versions compare as numbers, not text.
new_repo 1.10.0 1.10.0
annotate v1.10.0
run_guard v1.10.0 "$tip" $'v1.9.0\nv1.2.0'
expect_ok "1.10.0 after 1.9.0"
new_repo 1.9.0 1.9.0
annotate v1.9.0
run_guard v1.9.0 "$tip" $'v1.10.0'
expect_refusal "1.9.0 after 1.10.0" "not higher than the existing Release v1.10.0"

# The real `gh` path, with a fake gh on PATH. The fake prints the tag names
# in FAKE_GH_RELEASES (a file) the way `gh api --paginate --jq` would. It
# fails when FAKE_GH_FAIL is set, and without --paginate it prints only the
# first 100 lines, one page, so a guard that forgets to paginate misses
# anything after that.
mkdir "$tmp/bin"
cat >"$tmp/bin/gh" <<'GH'
#!/bin/bash
[[ -z ${FAKE_GH_FAIL:-} ]] || { echo "gh: HTTP 502" >&2; exit 1; }
[[ $1 == api && $2 == --paginate && $3 == "repos/owner/repo/releases"* ]] || {
    echo "fake gh: unexpected arguments: $*" >&2
    exit 1
}
cat "$FAKE_GH_RELEASES"
GH
chmod +x "$tmp/bin/gh"
export FAKE_GH_RELEASES=$tmp/releases.txt

new_repo 1.5.0 1.5.0
annotate v1.5.0

: >"$FAKE_GH_RELEASES"
# gh's own error line stays visible above the guard's one-line reason.
FAKE_GH_FAIL=1 run_guard_gh v1.5.0 "$tip"
tail -n 1 "$tmp/out" >"$tmp/out.last"
grep -qF "gh: HTTP 502" "$tmp/out" && mv "$tmp/out.last" "$tmp/out"
expect_refusal "gh fails" "could not list GitHub Releases"

# 250 Releases is more than two pages; the higher one is last.
for i in $(seq 1 249); do echo "v1.0.$i"; done >"$FAKE_GH_RELEASES"
run_guard_gh v1.5.0 "$tip"
expect_ok "gh list of 249 lower Releases"

echo v2.0.0 >>"$FAKE_GH_RELEASES"
run_guard_gh v1.5.0 "$tip"
expect_refusal "gh higher Release on a late page" "not higher than the existing Release v2.0.0"

echo v1.5.0 >"$FAKE_GH_RELEASES"
run_guard_gh v1.5.0 "$tip"
expect_refusal "gh Release for the tag" "already exists"

# changelog-section.sh.
new_repo 1.0.0 1.0.0
if [[ $("$section" 1.0.0 "$repo/CHANGELOG.md") == "Notes for 1.0.0." ]]; then
    pass
else
    fail "changelog-section: wrong text for 1.0.0"
fi
if [[ $("$section" 0.1.0 "$repo/CHANGELOG.md") == "Old notes." ]]; then
    pass
else
    fail "changelog-section: wrong text for the last section"
fi
if "$section" 1.0.0 - <"$repo/CHANGELOG.md" >/dev/null; then
    pass
else
    fail "changelog-section: stdin"
fi
if "$section" 1.0 "$repo/CHANGELOG.md" >/dev/null 2>&1; then
    fail "changelog-section: matched a prefix of a version"
else
    pass
fi
if "$section" 1x0x0 "$repo/CHANGELOG.md" >/dev/null 2>&1; then
    fail "changelog-section: treated dots as a regex"
else
    pass
fi

# readme-runtime-packages.sh.
cat >"$tmp/README.md" <<'MD'
# Test

<!-- ubuntu-runtime-packages:start -->
```
libx11-6
  libxkbcommon0

libegl1
```
<!-- ubuntu-runtime-packages:end -->

Other text.
MD
if [[ $("$packages" "$tmp/README.md" | tr '\n' ' ') == "libx11-6 libxkbcommon0 libegl1 " ]]; then
    pass
else
    fail "readme-runtime-packages: wrong list"
fi
printf '# No block\n' >"$tmp/README.md"
if "$packages" "$tmp/README.md" >/dev/null 2>&1; then
    fail "readme-runtime-packages: no markers should fail"
else
    pass
fi
printf '%s\n' '<!-- ubuntu-runtime-packages:start -->' '```' '```' '<!-- ubuntu-runtime-packages:end -->' >"$tmp/README.md"
if "$packages" "$tmp/README.md" >/dev/null 2>&1; then
    fail "readme-runtime-packages: empty block should fail"
else
    pass
fi

echo "$((cases - failures)) of $cases cases behaved correctly"
[[ $failures -eq 0 ]]
