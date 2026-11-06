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
trap 'rm -rf "$tmp"' EXIT

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

# A second run into the same folder refuses and changes nothing.
before=$(find "$tmp/one" -type f -printf '%P %s\n' | sort)
expect_exit "non-empty folder" 1 "$tmp/one"
if grep -q 'not empty' "$tmp/err"; then pass; else fail "refusal does not say the folder is not empty"; fi
if [[ $before == "$(find "$tmp/one" -type f -printf '%P %s\n' | sort)" ]]; then
    pass
else
    fail "a refused run changed the folder"
fi

# A file in the way, and an empty existing folder.
: >"$tmp/file"
expect_exit "a file in the way" 1 "$tmp/file"
mkdir "$tmp/empty"
expect_exit "an empty existing folder" 0 "$tmp/empty"

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
