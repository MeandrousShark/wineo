#!/bin/bash
# Tests scripts/write-checksums.sh and scripts/pack-vendored.sh. The checksum
# tests use plain temporary directories. The vendoring tests build a
# throwaway git repository holding a tiny Cargo workspace and a copy of
# pack-vendored.sh, so the script archives that repository instead of this
# one. The workspace depends on a crate in a second local git repository:
# `cargo vendor` has nothing to print for a workspace without dependencies,
# ignores source replacement of crates.io, and a path dependency isn't
# vendored, so a local git dependency is what lets the tests run offline. They
# check the tarball's name, layout, owners, and times, which ref is packed,
# that the unpacked tree resolves with an empty CARGO_HOME once the original
# dependency is gone, and the refusals. Nothing here writes outside a
# temporary directory, and cargo and git get an isolated CARGO_HOME and
# git config.
#
# Needs git, cargo, tar, and xz; without them the vendoring tests print a
# "skipped" line and only the checksum tests run.
#
# Usage: scripts/test-vendored.sh
# Exits non-zero if any case behaves wrongly.
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
checksums=$script_dir/write-checksums.sh

# Modes below assume this, whatever the caller's umask is.
umask 022

tmp=$(cd -P "$(mktemp -d)" && pwd -P)
trap 'rm -rf "${tmp:?}"' EXIT

failures=0
cases=0

pass() { cases=$((cases + 1)); }
fail() {
    cases=$((cases + 1))
    failures=$((failures + 1))
    echo "FAIL: $*" >&2
}

# same NAME WANT GOT: pass when the two strings are equal.
same() {
    if [[ $2 == "$3" ]]; then
        pass
    else
        fail "$1: wanted '$2', got '$3'"
    fi
}

# status NAME WANT CMD...: pass when the command exits WANT. Its output is
# kept in $tmp/stdout and $tmp/err.
status() {
    local name=$1 want=$2 got
    shift 2
    "$@" >"$tmp/stdout" 2>"$tmp/err" && got=0 || got=$?
    if [[ $got -eq $want ]]; then
        pass
    else
        fail "$name: expected exit $want, got $got ($(cat "$tmp/err"))"
    fi
}

# --- write-checksums.sh ---

sums=$tmp/sums
mkdir -p "$sums/wineo-1.2.3-x86_64-linux" "$sums/dir.tar.gz"
echo staged >"$sums/wineo-1.2.3-x86_64-linux/wineo"
echo nested >"$sums/dir.tar.gz/inner"
echo tarball >"$sums/wineo-1.2.3-x86_64-linux.tar.gz"
echo source >"$sums/wineo-1.2.3-vendored.tar.xz"
echo deb >"$sums/wineo_1.2.3_amd64.deb"
echo rpm >"$sums/wineo-1.2.3-1.x86_64.rpm"
echo dash >"$sums/-odd.rpm"
echo notes >"$sums/notes.txt"
ln -s wineo_1.2.3_amd64.deb "$sums/link.deb"
ln -s /nonexistent/asset "$sums/dead.rpm"

status "checksums" 0 "$checksums" "$sums"
want_names=$(printf '%s\n' -odd.rpm wineo-1.2.3-1.x86_64.rpm wineo-1.2.3-vendored.tar.xz \
    wineo-1.2.3-x86_64-linux.tar.gz wineo_1.2.3_amd64.deb | LC_ALL=C sort)
got_names=$(sed 's/^[0-9a-f]\{64\}  //' "$sums/SHA256SUMS" | LC_ALL=C sort)
same "checksum names" "$want_names" "$got_names"
if (cd "$sums" && sha256sum -c SHA256SUMS >/dev/null 2>&1); then
    pass
else
    fail "sha256sum -c failed on the written file"
fi
echo changed >"$sums/wineo_1.2.3_amd64.deb"
if (cd "$sums" && sha256sum -c SHA256SUMS >/dev/null 2>&1); then
    fail "sha256sum -c passed after an asset changed"
else
    pass
fi

before=$(cksum <"$sums/SHA256SUMS")
status "checksums refuse an existing SHA256SUMS" 1 "$checksums" "$sums"
same "existing SHA256SUMS kept" "$before" "$(cksum <"$sums/SHA256SUMS")"

dangling=$tmp/dangling
mkdir "$dangling"
echo asset >"$dangling/wineo.deb"
ln -s "$tmp/never-made" "$dangling/SHA256SUMS"
status "checksums refuse a dangling SHA256SUMS" 1 "$checksums" "$dangling"
if [[ -e $tmp/never-made || -L $tmp/never-made ]]; then
    fail "checksums wrote through a dangling SHA256SUMS symlink"
else
    pass
fi

empty=$tmp/empty
mkdir -p "$empty/wineo-1.2.3-x86_64-linux" "$empty/dir.tar.gz"
echo notes >"$empty/notes.txt"
ln -s /nonexistent/asset "$empty/dead.rpm"
status "checksums refuse a directory with no assets" 1 "$checksums" "$empty"
if [[ -e $empty/SHA256SUMS ]]; then
    fail "checksums left a SHA256SUMS in a directory with no assets"
else
    pass
fi
status "checksums refuse a missing directory" 1 "$checksums" "$tmp/no-such-dir"

mkdir "$tmp/rel"
echo asset >"$tmp/rel/wineo.rpm"
pushd "$tmp" >/dev/null
status "checksums with a relative directory" 0 "$checksums" rel
popd >/dev/null
same "relative directory result" "wineo.rpm" "$(sed 's/^[0-9a-f]\{64\}  //' "$tmp/rel/SHA256SUMS")"

status "checksums with no arguments" 2 "$checksums"
status "checksums with two arguments" 2 "$checksums" "$sums" "$sums"

# --- pack-vendored.sh ---

for tool in git cargo tar xz; do
    if ! command -v "$tool" >/dev/null; then
        echo "skipped: no $tool, so the pack-vendored.sh checks did not run"
        echo "$((cases - failures)) of $cases cases behaved correctly"
        [[ $failures -eq 0 ]]
        exit
    fi
done

# The rustup proxy finds its toolchain through RUSTUP_HOME, which defaults to
# a directory under the HOME that the empty CARGO_HOME below would hide.
if [[ -z ${RUSTUP_HOME:-} && -d $HOME/.rustup ]]; then
    export RUSTUP_HOME=$HOME/.rustup
fi
export CARGO_HOME=$tmp/cargo-home
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.com
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.com

# commit_at DIR EPOCH MESSAGE: commit everything staged, at a fixed time.
commit_at() {
    GIT_AUTHOR_DATE="$2 +0000" GIT_COMMITTER_DATE="$2 +0000" git -C "$1" commit -q -m "$3"
}

# The dependency repository.
dep=$tmp/dep
mkdir -p "$dep/src"
cat >"$dep/Cargo.toml" <<'TOML'
[package]
name = "tiny"
version = "1.0.0"
edition = "2021"
TOML
echo 'pub fn tiny() {}' >"$dep/src/lib.rs"
git init -q -b main "$dep"
git -C "$dep" add Cargo.toml src/lib.rs
commit_at "$dep" 900000000 "tiny crate"

# The repository the script archives, with its own copy of the script.
repo=$tmp/repo
mkdir -p "$repo/app/src" "$repo/scripts"
cat >"$repo/Cargo.toml" <<'TOML'
[workspace]
members = ["app"]
resolver = "2"
TOML
cat >"$repo/app/Cargo.toml" <<TOML
[package]
name = "app"
version = "0.1.0"
edition = "2021"

[dependencies]
tiny = { git = "file://$dep" }
TOML
echo 'fn main() {}' >"$repo/app/src/main.rs"
echo one >"$repo/MARK"
cp "$script_dir/pack-vendored.sh" "$repo/scripts/pack-vendored.sh"
printf '/target/\n' >"$repo/.gitignore"
git init -q -b main "$repo"
(cd "$repo" && cargo generate-lockfile >/dev/null 2>&1)
git -C "$repo" add .gitignore Cargo.toml Cargo.lock app scripts MARK
commit_at "$repo" 1000000000 first
git -C "$repo" tag v1
first=$(git -C "$repo" rev-parse HEAD)

# Side branches that make the script fail: no lock file, and no dependency.
git -C "$repo" checkout -q -b nolock
git -C "$repo" rm -q Cargo.lock
commit_at "$repo" 1000000100 "drop the lock file"
git -C "$repo" tag nolock
git -C "$repo" checkout -q main
git -C "$repo" checkout -q -b nodeps
sed -i '/^tiny = /d' "$repo/app/Cargo.toml"
(cd "$repo" && cargo generate-lockfile >/dev/null 2>&1)
git -C "$repo" add app/Cargo.toml Cargo.lock
commit_at "$repo" 1000000200 "drop the dependency"
git -C "$repo" tag nodeps
git -C "$repo" checkout -q main
# A branch that already tracks .cargo, which the script must not overwrite.
git -C "$repo" checkout -q -b withcargo
mkdir "$repo/.cargo"
printf '[build]\njobs = 1\n' >"$repo/.cargo/config.toml"
git -C "$repo" add .cargo/config.toml
commit_at "$repo" 1000000300 "track a cargo config"
git -C "$repo" tag withcargo
git -C "$repo" checkout -q main
# An annotated tag names the same commit as v1.
git -C "$repo" -c user.name=test -c user.email=test@example.invalid tag -a v1a -m annotated v1

echo two >"$repo/MARK"
git -C "$repo" add MARK
commit_at "$repo" 1100000000 second

# Untracked files and build output that must stay out of the tarball.
echo junk >"$repo/junk.txt"
mkdir -p "$repo/target/debug"
echo junk >"$repo/target/debug/artifact"

repo_state_before=$(git -C "$repo" status --porcelain -uall --ignored)

pack=$repo/scripts/pack-vendored.sh
version=1.2.3
name=wineo-$version
work=$tmp/work
mkdir "$work"

# pack_here WANT ARGS...: runs the script from $work, which must exit WANT.
pack_here() {
    local want=$1
    shift
    # pushd, not a subshell, so status's pass and fail counts survive.
    pushd "$work" >/dev/null
    status "pack $*" "$want" "$pack" "$@"
    popd >/dev/null
}

# No tarball may be left in OUTDIR after a failed run.
leftovers() { find "$1" -name '*.tar.xz' 2>/dev/null | wc -l; }

out1=$tmp/out1
pack_here 0 "$version" v1 "$out1"
tarball=$out1/$name-vendored.tar.xz
same "output" "$name-vendored.tar.xz" "$(ls -A "$out1")"
same "top directory" "$name" "$(tar -tf "$tarball" | cut -d/ -f1 | sort -u)"

# One owner and one time for every entry.
same "owners" "0/0" "$(tar --full-time --utc -tvf "$tarball" | awk '{ print $2 }' | sort -u)"
same "times" "2001-09-09 01:46:40" "$(tar --full-time --utc -tvf "$tarball" | awk '{ print $4, $5 }' | sort -u)"

x1=$tmp/x1
mkdir "$x1"
tar -xJf "$tarball" -C "$x1"
tree=$x1/$name
same "unpacked top level" "$name" "$(ls -A "$x1")"

# The tracked files, and only those, plus the two additions.
want_files=$(
    {
        git -C "$repo" ls-tree -r --name-only v1
        echo .cargo/config.toml
        echo vendor/tiny/Cargo.toml
    } | LC_ALL=C sort
)
got_files=$(cd "$tree" && find . -type f ! -path './vendor/*' -printf '%P\n' | LC_ALL=C sort)
want_main=$(grep -v '^vendor/' <<<"$want_files")
same "files outside vendor" "$want_main" "$got_files"
for path in junk.txt target .git; do
    if [[ -e $tree/$path ]]; then
        fail "the tarball holds $path, which is not tracked"
    else
        pass
    fi
done
same "MARK at v1" one "$(cat "$tree/MARK")"
same "vendored crate" 'name = "tiny"' "$(grep -m1 '^name' "$tree/vendor/tiny/Cargo.toml")"

cat >"$tmp/want-config" <<TOML
[source."git+file://$dep"]
git = "file://$dep"
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
TOML
if cmp -s "$tmp/want-config" "$tree/.cargo/config.toml"; then
    pass
else
    fail ".cargo/config.toml differs: $(diff "$tmp/want-config" "$tree/.cargo/config.toml" || true)"
fi

# The completeness check the release build runs. With the dependency's
# repository gone and an empty CARGO_HOME, only the vendored copy can supply
# it, and without the config the same command must fail.
metadata() { (cd "$tree" && CARGO_HOME=$tmp/empty-cargo-home cargo metadata --format-version 1 --frozen >/dev/null 2>&1); }
mkdir "$tmp/empty-cargo-home"
mv "$dep" "$dep.away"
if metadata; then
    pass
else
    fail "cargo metadata --frozen failed in the unpacked tree"
fi
mv "$tree/.cargo" "$tree/.cargo.off"
if metadata; then
    fail "cargo metadata --frozen passed without the source replacement config"
else
    pass
fi
mv "$tree/.cargo.off" "$tree/.cargo"
mv "$dep.away" "$dep"

# Which ref is packed.
mark_in() { tar -xJOf "$1" "$name/MARK"; }
out_head=$tmp/out-head
pack_here 0 "$version" HEAD "$out_head"
same "MARK at HEAD" two "$(mark_in "$out_head/$name-vendored.tar.xz")"
same "time at HEAD" "2004-11-09 11:33:20" \
    "$(tar --full-time --utc -tvf "$out_head/$name-vendored.tar.xz" | awk '{ print $4, $5 }' | sort -u)"
out_branch=$tmp/out-branch
pack_here 0 "$version" main "$out_branch"
same "MARK at main" two "$(mark_in "$out_branch/$name-vendored.tar.xz")"
out_hash=$tmp/out-hash
pack_here 0 "$version" "$first" "$out_hash"
same "MARK at a commit hash" one "$(mark_in "$out_hash/$name-vendored.tar.xz")"

# A relative OUTDIR is relative to the caller's directory.
pack_here 0 "$version" v1 rel-out
if [[ -f $work/rel-out/$name-vendored.tar.xz ]]; then
    pass
else
    fail "a relative OUTDIR was not resolved against the working directory"
fi

# The caller's checkout is untouched.
same "repository state" "$repo_state_before" "$(git -C "$repo" status --porcelain -uall --ignored)"
for path in vendor .cargo; do
    if [[ -e $repo/$path ]]; then
        fail "the script left $path in the repository"
    else
        pass
    fi
done

# Refusals leave an existing output alone.
before=$(cksum <"$tarball")
pack_here 1 "$version" v1 "$out1"
same "existing output kept" "$before" "$(cksum <"$tarball")"

out_link=$tmp/out-link
mkdir "$out_link"
ln -s "$tmp/never-made" "$out_link/$name-vendored.tar.xz"
pack_here 1 "$version" v1 "$out_link"
if [[ -e $tmp/never-made || -L $tmp/never-made ]]; then
    fail "the script wrote through a dangling output symlink"
else
    pass
fi

# Failed runs leave no tarball.
out_bad=$tmp/out-bad
pack_here 1 "$version" no-such-ref "$out_bad"
same "no tarball for an unknown ref" 0 "$(leftovers "$out_bad")"
pack_here 1 "$version" nolock "$out_bad"
same "no tarball without a lock file" 0 "$(leftovers "$out_bad")"
pack_here 1 "$version" nodeps "$out_bad"
same "no tarball without dependencies" 0 "$(leftovers "$out_bad")"
pack_here 1 "$version" withcargo "$out_bad"
same "no tarball when the ref tracks .cargo" 0 "$(leftovers "$out_bad")"

# An annotated tag packs its commit, like the lightweight one.
out_ann=$tmp/out-ann
pack_here 0 "$version" v1a "$out_ann"
same "annotated tag packs the same files" \
    "$(tar -tf "$tarball" | LC_ALL=C sort | cksum)" \
    "$(tar -tf "$out_ann/$name-vendored.tar.xz" | LC_ALL=C sort | cksum)"

# Bad usage.
pack_here 2
pack_here 2 "$version" v1
pack_here 2 "$version" v1 "$out_bad" extra
pack_here 2 "" v1 "$out_bad"
pack_here 2 ../escape v1 "$out_bad"
pack_here 2 "$version" "" "$out_bad"
pack_here 2 "$version" --output=/dev/null "$out_bad"
same "no tarball after bad usage" 0 "$(leftovers "$out_bad")"

echo "$((cases - failures)) of $cases cases behaved correctly"
[[ $failures -eq 0 ]]
