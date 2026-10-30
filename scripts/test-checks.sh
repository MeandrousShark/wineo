#!/bin/bash
# Tests for scripts/check-installed.sh and scripts/check-gui-starts.sh.
# check-installed runs against fake install trees: a complete one passes,
# each missing item is named, and both man page forms are accepted.
# check-gui-starts runs with a fake xvfb-run on PATH that just runs the
# command it is given, and fake GUIs that stay up, exit early, or panic.
# Nothing here needs a display or writes outside a temporary directory.
#
# Usage: scripts/test-checks.sh
# Exits non-zero if any case behaves wrongly.
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
installed=$script_dir/check-installed.sh
gui_check=$script_dir/check-gui-starts.sh

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

# make_tree PREFIX LICENSE_DIR [MAN_SUFFIX]
# A complete install; the man pages end in MAN_SUFFIX (default empty, so
# wineo.1, or ".gz" for wineo.1.gz).
make_tree() {
    local prefix=$1 licenses=$2 gz=${3:-}
    mkdir -p "$prefix/bin" "$licenses" "$prefix/share/man/man1"
    printf '#!/bin/bash\n' >"$prefix/bin/wineo"
    printf '#!/bin/bash\n' >"$prefix/bin/wineo-gui"
    chmod 755 "$prefix/bin/wineo" "$prefix/bin/wineo-gui"
    local file
    for file in \
        share/applications/wineo-gui.desktop \
        share/icons/hicolor/scalable/apps/wineo-gui.svg \
        share/bash-completion/completions/wineo \
        share/zsh/site-functions/_wineo \
        share/fish/vendor_completions.d/wineo.fish \
        "share/man/man1/wineo.1$gz" \
        "share/man/man1/wineo-start.1$gz"; do
        mkdir -p "$prefix/$(dirname "$file")"
        echo fake >"$prefix/$file"
    done
    echo fake >"$licenses/LICENSE"
    echo fake >"$licenses/THIRD_PARTY.md"
}

# run_installed PREFIX LICENSE_DIR: sets out and status.
run_installed() {
    set +e
    out=$("$installed" "$@" 2>&1)
    status=$?
    set -e
}

# expect_installed_ok NAME PREFIX LICENSE_DIR
expect_installed_ok() {
    run_installed "${@:2}"
    if [[ $status -eq 0 ]]; then pass; else fail "$1: exit $status: $out"; fi
}

# expect_installed_missing NAME PREFIX LICENSE_DIR NEEDLE
# Fails unless the check exits 1 and names NEEDLE exactly once.
expect_installed_missing() {
    run_installed "$2" "$3"
    if [[ $status -ne 1 ]]; then
        fail "$1: exit $status, want 1: $out"
    elif [[ $(grep -cF -- "$4" <<<"$out") -ne 1 ]]; then
        fail "$1: output does not name '$4' once: $out"
    else
        pass
    fi
}

# --- check-installed.sh ---

prefix=$tmp/a/usr
licenses=$prefix/share/licenses/wineo
make_tree "$prefix" "$licenses"
expect_installed_ok "complete tree, plain man pages" "$prefix" "$licenses"

prefix=$tmp/b/usr
make_tree "$prefix" "$tmp/b/doc" .gz
expect_installed_ok "complete tree, gzipped man pages, license dir elsewhere" \
    "$prefix" "$tmp/b/doc"

# A trailing slash on either argument changes nothing.
expect_installed_ok "trailing slashes" "$prefix/" "$tmp/b/doc/"

# Each item, removed alone from a complete tree, is named and fails.
removals=(
    "bin/wineo"
    "bin/wineo-gui"
    "share/applications/wineo-gui.desktop"
    "share/icons/hicolor/scalable/apps/wineo-gui.svg"
    "share/bash-completion/completions/wineo"
    "share/zsh/site-functions/_wineo"
    "share/fish/vendor_completions.d/wineo.fish"
)
n=0
for item in "${removals[@]}"; do
    n=$((n + 1))
    prefix=$tmp/r$n/usr
    make_tree "$prefix" "$tmp/r$n/lic"
    rm "$prefix/$item"
    expect_installed_missing "missing $item" "$prefix" "$tmp/r$n/lic" "$prefix/$item"
done

for item in LICENSE THIRD_PARTY.md; do
    n=$((n + 1))
    prefix=$tmp/r$n/usr
    make_tree "$prefix" "$tmp/r$n/lic"
    rm "$tmp/r$n/lic/$item"
    expect_installed_missing "missing $item" "$prefix" "$tmp/r$n/lic" "$tmp/r$n/lic/$item"
done

n=$((n + 1))
prefix=$tmp/r$n/usr
make_tree "$prefix" "$tmp/r$n/lic"
chmod 644 "$prefix/bin/wineo-gui"
expect_installed_missing "non-executable binary" "$prefix" "$tmp/r$n/lic" \
    "$prefix/bin/wineo-gui is not executable"

n=$((n + 1))
prefix=$tmp/r$n/usr
make_tree "$prefix" "$tmp/r$n/lic"
rm "$prefix/share/man/man1/wineo.1"
expect_installed_missing "missing main man page" "$prefix" "$tmp/r$n/lic" \
    "share/man/man1/wineo.1 or wineo.1.gz"

n=$((n + 1))
prefix=$tmp/r$n/usr
make_tree "$prefix" "$tmp/r$n/lic" .gz
rm "$prefix/share/man/man1/wineo-start.1.gz"
expect_installed_missing "only one man page" "$prefix" "$tmp/r$n/lic" "a second man page"

# Several things missing at once: one line each.
n=$((n + 1))
prefix=$tmp/r$n/usr
make_tree "$prefix" "$tmp/r$n/lic"
rm "$prefix/bin/wineo" "$prefix/share/zsh/site-functions/_wineo" "$tmp/r$n/lic/LICENSE"
run_installed "$prefix" "$tmp/r$n/lic"
if [[ $status -eq 1 && $(grep -c '^missing: ' <<<"$out") -eq 3 ]]; then
    pass
else
    fail "three missing items: exit $status, output: $out"
fi

# A prefix that doesn't exist reports everything.
run_installed "$tmp/nothing/usr" "$tmp/nothing/lic"
if [[ $status -eq 1 && $(grep -c '^missing: ' <<<"$out") -ge 9 ]]; then
    pass
else
    fail "missing prefix: exit $status, output: $out"
fi

# Usage errors exit 2.
usage_cases=("" "only-one" "a b c")
for args in "${usage_cases[@]}"; do
    read -r -a argv <<<"$args"
    set +e
    "$installed" "${argv[@]}" >/dev/null 2>&1
    status=$?
    set -e
    if [[ $status -eq 2 ]]; then pass; else fail "check-installed usage '$args': exit $status, want 2"; fi
done
set +e
"$installed" "" b >/dev/null 2>&1
status=$?
set -e
if [[ $status -eq 2 ]]; then pass; else fail "check-installed empty prefix: exit $status, want 2"; fi

# --- check-gui-starts.sh ---

# A fake xvfb-run: skips its own options (-a, -s ARG) and runs the command.
fakebin=$tmp/fakebin
mkdir -p "$fakebin"
cat >"$fakebin/xvfb-run" <<'FAKE'
#!/bin/bash
while [[ $1 == -* ]]; do
    case $1 in
        -s) shift 2 ;;
        *) shift ;;
    esac
done
exec "$@"
FAKE
chmod +x "$fakebin/xvfb-run"

# fake_gui NAME BODY: an executable script in $tmp/guis.
fake_gui() {
    mkdir -p "$tmp/guis"
    printf '#!/bin/bash\n%s\n' "$2" >"$tmp/guis/$1"
    chmod 755 "$tmp/guis/$1"
}

# run_gui BINARY...: sets out and status.
run_gui() {
    set +e
    out=$(PATH="$fakebin:$PATH" "$gui_check" "$@" 2>&1)
    status=$?
    set -e
}

# The GUI sees the throwaway environment and no display variables.
# It is started by a relative path, which the script makes absolute.
# shellcheck disable=SC2016 # the fake GUI expands these when it runs
fake_gui stays 'echo "home=$HOME backend=$ICED_BACKEND display=${DISPLAY:-unset}"; sleep 30'
cd "$tmp/guis"
DISPLAY=:0 WAYLAND_DISPLAY=wayland-9 run_gui stays
cd "$script_dir"
if [[ $status -eq 0 ]]; then pass; else fail "gui that stays up: exit $status: $out"; fi
if grep -q 'backend=tiny-skia display=unset' <<<"$out"; then
    pass
else
    fail "gui environment wrong: $out"
fi
# The log is printed, and the throwaway home is gone afterwards.
home=$(sed -n 's/^home=\([^ ]*\) .*/\1/p' <<<"$out" | head -n 1)
if [[ -n $home && $home != "${HOME:-}" && ! -e $home ]]; then
    pass
else
    fail "throwaway home '$home' not removed or not isolated"
fi

fake_gui exits-early 'echo "no display"; exit 1'
run_gui "$tmp/guis/exits-early"
if [[ $status -eq 1 ]] && grep -q 'exited with status 1' <<<"$out" && grep -q 'no display' <<<"$out"; then
    pass
else
    fail "gui that exits early: exit $status: $out"
fi

fake_gui exits-clean 'exit 0'
run_gui "$tmp/guis/exits-clean"
if [[ $status -eq 1 ]]; then pass; else fail "gui that exits 0 early: exit $status, want 1"; fi

fake_gui panics 'echo "thread main PANICKED at src/x.rs"; sleep 30'
run_gui "$tmp/guis/panics"
if [[ $status -eq 1 ]] && grep -q 'panicked' <<<"$out"; then
    pass
else
    fail "gui that panics: exit $status: $out"
fi

# Argument handling.
run_gui
if [[ $status -eq 2 ]]; then pass; else fail "no arguments: exit $status, want 2"; fi
run_gui a b
if [[ $status -eq 2 ]]; then pass; else fail "two arguments: exit $status, want 2"; fi
run_gui "$tmp/guis/missing"
if [[ $status -eq 1 ]] && grep -q 'missing or not executable' <<<"$out"; then
    pass
else
    fail "missing binary: exit $status: $out"
fi
printf '#!/bin/bash\n' >"$tmp/guis/plain"
chmod 644 "$tmp/guis/plain"
run_gui "$tmp/guis/plain"
if [[ $status -eq 1 ]]; then pass; else fail "non-executable binary: exit $status, want 1"; fi

echo "$((cases - failures)) of $cases cases behaved correctly"
[[ $failures -eq 0 ]]
