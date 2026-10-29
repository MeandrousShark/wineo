#!/bin/bash
# Refuse a release unless everything about it checks out. The release
# workflow runs this in its first job and again at the start of publish,
# so a bad tag or a stale rerun never gets as far as a GitHub Release.
# Workflow tag filters are globs, so this is the real check.
#
# Passes only when all of these hold:
#   - the event is a tag push and the ref is a strict vX.Y.Z tag
#   - the version is at least 1.0.0
#   - [workspace.package] version in the tagged commit's Cargo.toml matches
#   - the tag is annotated, points at GITHUB_SHA, and that commit is on main
#   - the tagged commit's CHANGELOG.md has a "## X.Y.Z - " section
#   - no GitHub Release exists for the tag, and the version is higher than
#     every existing Release's version
#
# Environment:
#   GITHUB_EVENT_NAME, GITHUB_REF, GITHUB_SHA   set by GitHub Actions
#   GH_TOKEN, GH_REPO                           for `gh api` (Release list)
#   RELEASE_GUARD_REPO      the git repository (default: current directory)
#   RELEASE_GUARD_MAIN_REF  the main branch ref (default: origin/main)
#   RELEASE_GUARD_RELEASES  newline-separated Release tags, used instead of
#                           asking GitHub (for tests; may be empty)
#
# Exits 1 with a one-line reason on stderr when it refuses.
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

refuse() {
    echo "release-guard: refused: $*" >&2
    exit 1
}

semver='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
ref_re='^refs/tags/v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'

# Compares two numbers without leading zeros as strings, so a huge number
# can't overflow shell arithmetic. Prints -1, 0, or 1.
num_cmp() {
    if ((${#1} != ${#2})); then
        ((${#1} < ${#2})) && echo -1 || echo 1
    elif [[ $1 == "$2" ]]; then
        echo 0
    elif [[ $1 < $2 ]]; then
        echo -1
    else
        echo 1
    fi
}

# Prints -1, 0, or 1 for version $1 compared with version $2.
ver_cmp() {
    local -a a b
    local i c
    IFS=. read -r -a a <<<"$1"
    IFS=. read -r -a b <<<"$2"
    for i in 0 1 2; do
        c=$(num_cmp "${a[i]}" "${b[i]}")
        if [[ $c != 0 ]]; then
            echo "$c"
            return
        fi
    done
    echo 0
}

event=${GITHUB_EVENT_NAME:-}
ref=${GITHUB_REF:-}
sha=${GITHUB_SHA:-}
repo=${RELEASE_GUARD_REPO:-.}
main_ref=${RELEASE_GUARD_MAIN_REF:-origin/main}

[[ $event == push ]] || refuse "the event is '$event', not a tag push"
[[ $ref =~ $ref_re ]] || refuse "'$ref' is not a vX.Y.Z tag"
tag=${ref#refs/tags/}
version=${tag#v}

major=${version%%.*}
[[ $major != 0 ]] || refuse "$tag is below 1.0.0"

[[ $sha =~ ^[0-9a-f]{40}$ ]] || refuse "GITHUB_SHA is not a full commit hash"

git_() { git -C "$repo" "$@"; }

git_ rev-parse -q --verify "refs/tags/$tag" >/dev/null 2>&1 ||
    refuse "tag $tag does not exist in the checkout"
[[ $(git_ cat-file -t "refs/tags/$tag") == tag ]] ||
    refuse "tag $tag is not annotated"
peeled=$(git_ rev-parse "refs/tags/$tag^{commit}")
[[ $peeled == "$sha" ]] ||
    refuse "tag $tag points at ${peeled:0:12}, not at this run's commit ${sha:0:12}"
git_ rev-parse -q --verify "$main_ref^{commit}" >/dev/null 2>&1 ||
    refuse "$main_ref does not exist in the checkout"
git_ merge-base --is-ancestor "$sha" "$main_ref" ||
    refuse "commit ${sha:0:12} is not on $main_ref"

cargo_toml=$(git_ show "$sha:Cargo.toml" 2>/dev/null) ||
    refuse "no Cargo.toml in the tagged commit"
cargo_version=$(awk -F '"' '
    /^\[workspace\.package\]$/ { in_workspace = 1; next }
    /^\[/ { in_workspace = 0 }
    in_workspace && /^version = / { print $2; exit }
' <<<"$cargo_toml")
[[ $cargo_version == "$version" ]] ||
    refuse "Cargo.toml version '$cargo_version' does not match tag $tag"

changelog=$(git_ show "$sha:CHANGELOG.md" 2>/dev/null) ||
    refuse "no CHANGELOG.md in the tagged commit"
"$script_dir/changelog-section.sh" "$version" - <<<"$changelog" >/dev/null ||
    refuse "CHANGELOG.md has no '## $version - ' section"

if [[ -n ${RELEASE_GUARD_RELEASES+set} ]]; then
    releases=$RELEASE_GUARD_RELEASES
else
    # Every page, so "higher than every Release" really means every one.
    gh_repo=${GH_REPO:-'{owner}/{repo}'}
    releases=$(gh api --paginate "repos/$gh_repo/releases?per_page=100" --jq '.[].tag_name') ||
        refuse "could not list GitHub Releases"
fi

# Releases whose tags aren't strict versions can't be compared and are
# skipped.
while IFS= read -r existing; do
    [[ -n $existing ]] || continue
    [[ $existing != "$tag" ]] || refuse "a Release for $tag already exists"
    existing_version=${existing#v}
    if [[ $existing == v* && $existing_version =~ $semver ]]; then
        [[ $(ver_cmp "$version" "$existing_version") == 1 ]] ||
            refuse "$tag is not higher than the existing Release $existing"
    fi
done <<<"$releases"

echo "release-guard: ok: $tag"
