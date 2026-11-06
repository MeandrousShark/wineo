#!/bin/bash
# Runs scripts/acceptance-library.sh into temporary folders and checks the
# refusals, the game count, that every game is described in the summary, and
# that two runs write identical libraries. Nothing here touches the real HOME
# or XDG directories, and no wineo binary is needed.
#
# Usage: scripts/test-acceptance-library.sh
# Exits non-zero if any case behaves wrongly.
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
script=$script_dir/acceptance-library.sh

tmp=$(mktemp -d)
# A run that stops mid-case can leave the 0300 folder behind, which rm can't
# enter.
trap 'chmod -R u+rwx "$tmp" 2>/dev/null; rm -rf "$tmp"' EXIT

failures=0
cases=0

pass() { cases=$((cases + 1)); }
fail() {
    cases=$((cases + 1))
    failures=$((failures + 1))
    echo "FAIL: $*" >&2
}

# expect_exit LABEL CODE ARG...: runs the script with ARGs and checks its exit
# status, leaving stdout and stderr in $tmp/out and $tmp/err.
expect_exit() {
    local label=$1 want=$2 got=0
    shift 2
    "$script" "$@" >"$tmp/out" 2>"$tmp/err" || got=$?
    if [[ $got -eq $want ]]; then
        pass
    else
        fail "$label: exit $got, wanted $want"
    fi
}

expect_exit "--help" 0 --help
if grep -q '^Usage:' "$tmp/out"; then pass; else fail "--help prints no usage"; fi
expect_exit "no arguments" 2
expect_exit "too many arguments" 2 "$tmp/a" "$tmp/b"

# A fresh folder works, and the game count matches the files and the summary.
expect_exit "new folder" 0 "$tmp/one"
envs=$(find "$tmp/one/config/environments" -name '*.toml' | wc -l)
if [[ $envs -ge 40 ]]; then pass; else fail "only $envs games, wanted at least 40"; fi
rows=$(awk '/^Name /{t=1; next} t && NF==0{exit} t' "$tmp/out" | wc -l)
if grep -q "^Wrote $envs games to " "$tmp/out" && [[ $rows -eq $envs ]]; then
    pass
else
    fail "summary does not match $envs games"
fi
if [[ -f $tmp/one/config/library.toml && -x $tmp/one/games/long-running-test.sh ]]; then
    pass
else
    fail "library.toml or the long-running stub is missing"
fi

# Every source detection input is present at least once.
for pattern in 'kind = "proxied"' 'kind = "native"' '^gog_id' '^heroic_id' 'store = "egs"' 'store = "amazon"'; do
    if grep -lq "$pattern" "$tmp"/one/config/environments/*.toml; then
        pass
    else
        fail "no game has $pattern"
    fi
done

# no_leftovers LABEL PARENT: a refusal must not leave its work folder behind.
no_leftovers() {
    if [[ -z $(find "$2" -maxdepth 1 -name '.acceptance-library.*') ]]; then
        pass
    else
        fail "$1: a work folder was left in $2"
    fi
}

# fingerprint DIR: every path, mode, and content hash under DIR, so a change
# to a single byte shows.
fingerprint() { (cd "$1" && find . -printf '%p %m %l\n' -type f -exec sha256sum {} + | sort); }

# refused LABEL PARENT TARGET...: the script refuses TARGET (and leaves it
# untouched), without leaving a work folder in PARENT.
refused() {
    local label=$1 parent=$2 target=$3 before after
    before=$(fingerprint "$target")
    expect_exit "$label" 1 "$target"
    after=$(fingerprint "$target")
    if [[ $before == "$after" ]]; then pass; else fail "$label: the folder changed"; fi
    no_leftovers "$label" "$parent"
}

# A second run into the same folder refuses and changes nothing, byte for byte.
refused "non-empty folder" "$tmp" "$tmp/one"
if grep -q 'not empty' "$tmp/err"; then pass; else fail "refusal does not say the folder is not empty"; fi

# The generated library has no path of the work folder baked in.
if grep -rqF '.acceptance-library.' "$tmp/one"; then
    fail "the work folder's name ended up in the library"
else
    pass
fi

# A folder holding only a hidden file is not empty.
mkdir "$tmp/hidden"
echo keep >"$tmp/hidden/.keep"
refused "hidden file only" "$tmp" "$tmp/hidden"

# A folder that can't be listed fails closed. (As root the listing works, and
# the file inside still makes it non-empty, so it is refused either way.)
mkdir "$tmp/locked"
echo keep >"$tmp/locked/sentinel"
chmod 0300 "$tmp/locked"
expect_exit "unreadable folder" 1 "$tmp/locked"
if [[ $EUID -ne 0 ]]; then
    if grep -q "can't list" "$tmp/err"; then pass; else fail "an unreadable folder was not refused as unlistable"; fi
fi
chmod 0700 "$tmp/locked"
if [[ $(cat "$tmp/locked/sentinel") == keep && $(find "$tmp/locked" -mindepth 1 | wc -l) -eq 1 ]]; then
    pass
else
    fail "an unreadable folder was changed"
fi
no_leftovers "unreadable folder" "$tmp"

# A symlink is refused even when it points at an empty folder, since the
# rename wouldn't replace it, and what it points at is left alone.
mkdir "$tmp/target-full" "$tmp/target-empty"
echo keep >"$tmp/target-full/sentinel"
ln -s target-full "$tmp/link-full"
ln -s target-empty "$tmp/link-empty"
ln -s nowhere "$tmp/link-dangling"
before=$(fingerprint "$tmp/target-full")
expect_exit "symlink to a non-empty folder" 1 "$tmp/link-full"
expect_exit "symlink to a non-empty folder, trailing slash" 1 "$tmp/link-full/"
expect_exit "symlink to an empty folder" 1 "$tmp/link-empty"
expect_exit "dangling symlink" 1 "$tmp/link-dangling"
if [[ $before == "$(fingerprint "$tmp/target-full")" && -z $(ls -A "$tmp/target-empty") && ! -e $tmp/nowhere ]]; then
    pass
else
    fail "a symlink target was changed"
fi
no_leftovers "symlinks" "$tmp"

# A parent that doesn't exist is refused and not created.
expect_exit "missing parent" 1 "$tmp/no/such/parent/lib"
if [[ ! -e $tmp/no ]]; then pass; else fail "the missing parent was created"; fi

# Names with nothing to create.
expect_exit "dot" 1 .
expect_exit "dot dot" 1 ..
expect_exit "root" 1 /

# A race, reproduced by an exported `ls` that changes DIR right after the
# listing, which is the moment the early check can no longer be trusted. The
# rename still refuses, and the new content in DIR and what a swapped-in
# symlink points at survive.
# shellcheck disable=SC2016 # expanded later, by the shell that evals it
race_ls='ls() { command ls "$@"; local rc=$?; case ${RACE:-} in
    link) rmdir -- "$RACE_DIR"; ln -s -- "$RACE_TARGET" "$RACE_DIR" ;;
    file) echo late >"$RACE_DIR/late" ;;
    esac; return $rc; }'
mkdir "$tmp/race-victim"
echo keep >"$tmp/race-victim/sentinel"
before=$(fingerprint "$tmp/race-victim")
mkdir "$tmp/race-link"
got=0
(
    eval "$race_ls"
    export -f ls
    RACE=link RACE_DIR=$tmp/race-link RACE_TARGET=$tmp/race-victim "$script" "$tmp/race-link" >/dev/null 2>"$tmp/err"
) || got=$?
if [[ $got -eq 1 && -L $tmp/race-link && $before == "$(fingerprint "$tmp/race-victim")" ]]; then
    pass
else
    fail "a symlink swapped in after the check: exit $got, victim or link changed"
fi
mkdir "$tmp/race-file"
got=0
(
    eval "$race_ls"
    export -f ls
    RACE=file RACE_DIR=$tmp/race-file "$script" "$tmp/race-file" >/dev/null 2>"$tmp/err"
) || got=$?
if [[ $got -eq 1 && $(cat "$tmp/race-file/late") == late && $(find "$tmp/race-file" -mindepth 1 | wc -l) -eq 1 ]]; then
    pass
else
    fail "a file added after the check: exit $got, folder changed"
fi
no_leftovers "races" "$tmp"

# A file in the way, and an empty existing folder (trailing slash too).
: >"$tmp/file"
expect_exit "a file in the way" 1 "$tmp/file"
mkdir "$tmp/empty"
expect_exit "an empty existing folder" 0 "$tmp/empty/"
if [[ -f $tmp/empty/config/library.toml && -d $tmp/empty/games ]]; then pass; else fail "the empty folder was not filled"; fi
no_leftovers "success" "$tmp"

# Fixed dates: two runs write the same bytes.
if diff -r "$tmp/one" "$tmp/empty" >/dev/null; then
    pass
else
    fail "two runs wrote different libraries"
fi

# A relative path works from another folder and lands there.
mkdir "$tmp/cwd"
if (cd "$tmp/cwd" && "$script" rel >/dev/null 2>&1); then pass; else fail "a relative folder failed"; fi
if [[ -f $tmp/cwd/rel/config/library.toml ]]; then pass; else fail "a relative folder was not written in place"; fi

echo "$((cases - failures)) of $cases cases behaved correctly"
[[ $failures -eq 0 ]]
