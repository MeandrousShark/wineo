#!/bin/bash
# Runs scripts/stage-install.sh against fake `wineo` and `wineo-gui`
# binaries, so it needs no build, and checks the exact file list and modes it
# writes, the --license-dir option, relative paths, and the failure cases.
# Nothing here writes outside a temporary directory.
#
# Usage: scripts/test-stage-install.sh
# Exits non-zero if any case behaves wrongly.
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
stage=$script_dir/stage-install.sh
root=$(cd "$script_dir/.." && pwd)

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

# fake_bins DIR
# The fakes print a marker naming the shell and write two man pages, which is
# all the script relies on.
fake_bins() {
    mkdir -p "$1"
    cat >"$1/wineo" <<'FAKE'
#!/bin/bash
case $1 in
    completions) echo "fake completions for $2" ;;
    manpages)
        mkdir -p "$2"
        echo fake >"$2/wineo.1"
        echo fake >"$2/wineo-start.1"
        ;;
    *) exit 1 ;;
esac
FAKE
    printf '#!/bin/bash\nexit 0\n' >"$1/wineo-gui"
    chmod +x "$1/wineo" "$1/wineo-gui"
}

# listing DIR: every file under DIR with its mode, sorted.
listing() { (cd "$1" && find . -mindepth 1 ! -type d -printf '%P %m\n' | sort); }

# expected_listing LICENSE_DIR: the whole tree for that license dir.
expected_listing() {
    local lic=${1#/}
    sort <<LIST
usr/bin/wineo 755
usr/bin/wineo-gui 755
$lic/LICENSE 644
$lic/THIRD_PARTY.md 644
usr/share/applications/wineo-gui.desktop 644
usr/share/icons/hicolor/scalable/apps/wineo-gui.svg 644
usr/share/doc/wineo/environment.toml.example 644
usr/share/doc/wineo/native-environment.toml.example 644
usr/share/doc/wineo/proxied-environment.toml.example 644
usr/share/bash-completion/completions/wineo 644
usr/share/zsh/site-functions/_wineo 644
usr/share/fish/vendor_completions.d/wineo.fish 644
usr/share/man/man1/wineo.1 644
usr/share/man/man1/wineo-start.1 644
LIST
}

# check_listing NAME DIR LICENSE_DIR
check_listing() {
    if diff <(expected_listing "$3") <(listing "$2") >"$tmp/diff"; then
        pass
    else
        fail "$1: file list or modes differ: $(cat "$tmp/diff")"
    fi
}

# expect_success NAME (after a run that set $status)
expect_success() {
    if [[ $status -eq 0 ]]; then
        pass
    else
        fail "$1: expected exit 0, got $status ($(cat "$tmp/err"))"
    fi
}

# expect_failure NAME EXPECTED_STATUS UNTOUCHED_DIR ARGS...
# Runs the script, which must exit with that status, print one line to
# stderr, and not create UNTOUCHED_DIR.
expect_failure() {
    local name=$1 want=$2 dir=$3
    shift 3
    "$stage" "$@" >"$tmp/stdout" 2>"$tmp/err" && status=0 || status=$?
    if [[ $status -ne $want ]]; then
        fail "$name: expected exit $want, got $status"
    elif [[ $(wc -l <"$tmp/err") -ne 1 ]]; then
        fail "$name: message is not one line: $(cat "$tmp/err")"
    elif [[ -e $dir ]]; then
        fail "$name: wrote $dir"
    else
        pass
    fi
}

fake_bins "$tmp/bin"

# Default license dir.
"$stage" "$tmp/dest1" "$tmp/bin" 2>"$tmp/err" && status=0 || status=$?
expect_success "default run"
check_listing "default license dir" "$tmp/dest1" /usr/share/licenses/wineo

# Contents come from the right sources.
if cmp -s "$root/LICENSE" "$tmp/dest1/usr/share/licenses/wineo/LICENSE" &&
    cmp -s "$root/docs/environment.toml.example" \
        "$tmp/dest1/usr/share/doc/wineo/environment.toml.example"; then
    pass
else
    fail "installed files differ from the repository's"
fi
if [[ $(cat "$tmp/dest1/usr/share/zsh/site-functions/_wineo") == "fake completions for zsh" &&
    $(cat "$tmp/dest1/usr/share/fish/vendor_completions.d/wineo.fish") == "fake completions for fish" &&
    $(cat "$tmp/dest1/usr/share/bash-completion/completions/wineo") == "fake completions for bash" ]]; then
    pass
else
    fail "completions are not the matching shell's output"
fi

# --license-dir moves exactly the two license files.
"$stage" --license-dir /usr/share/licenses/wineo-git "$tmp/dest2" "$tmp/bin" 2>"$tmp/err" && status=0 || status=$?
expect_success "--license-dir run"
check_listing "--license-dir" "$tmp/dest2" /usr/share/licenses/wineo-git
if diff <(listing "$tmp/dest1" | grep -v 'licenses/') <(listing "$tmp/dest2" | grep -v 'licenses/') >/dev/null; then
    pass
else
    fail "--license-dir changed more than the license files"
fi

# A license dir outside /usr is used as given, under DESTDIR.
"$stage" --license-dir /opt/lic/wineo "$tmp/dest3" "$tmp/bin" 2>"$tmp/err" && status=0 || status=$?
expect_success "--license-dir /opt/lic/wineo"
if [[ -f $tmp/dest3/opt/lic/wineo/LICENSE && -f $tmp/dest3/opt/lic/wineo/THIRD_PARTY.md &&
    ! -e $tmp/dest3/usr/share/licenses ]]; then
    pass
else
    fail "--license-dir /opt/lic/wineo: license files not under DESTDIR/opt/lic/wineo"
fi

# Relative DESTDIR and BINDIR, from a directory that isn't the repository.
mkdir "$tmp/elsewhere"
fake_bins "$tmp/elsewhere/mybin"
(cd "$tmp/elsewhere" && "$stage" out mybin) 2>"$tmp/err" && status=0 || status=$?
expect_success "relative paths"
check_listing "relative DESTDIR and BINDIR" "$tmp/elsewhere/out" /usr/share/licenses/wineo

# BINDIR defaults to target/release in the caller's directory.
mkdir "$tmp/default-bin"
fake_bins "$tmp/default-bin/target/release"
(cd "$tmp/default-bin" && "$stage" out) 2>"$tmp/err" && status=0 || status=$?
expect_success "default BINDIR"
check_listing "default BINDIR" "$tmp/default-bin/out" /usr/share/licenses/wineo

# Missing binaries: absent, or present without the executable bit.
mkdir "$tmp/empty"
expect_failure "missing wineo" 1 "$tmp/dest-missing" "$tmp/dest-missing" "$tmp/empty"
fake_bins "$tmp/nogui"
rm "$tmp/nogui/wineo-gui"
expect_failure "missing wineo-gui" 1 "$tmp/dest-nogui" "$tmp/dest-nogui" "$tmp/nogui"
fake_bins "$tmp/noexec"
chmod -x "$tmp/noexec/wineo"
expect_failure "non-executable wineo" 1 "$tmp/dest-noexec" "$tmp/dest-noexec" "$tmp/noexec"

# Usage errors.
expect_failure "no arguments" 2 "$tmp/none"
expect_failure "unknown option" 2 "$tmp/dest-opt" --nope "$tmp/dest-opt" "$tmp/bin"
expect_failure "license dir without a value" 2 "$tmp/none" --license-dir
expect_failure "relative license dir" 2 "$tmp/dest-rel" --license-dir share/lic "$tmp/dest-rel" "$tmp/bin"
expect_failure "too many arguments" 2 "$tmp/dest-many" "$tmp/dest-many" "$tmp/bin" extra

echo "$((cases - failures)) of $cases cases behaved correctly"
[[ $failures -eq 0 ]]
