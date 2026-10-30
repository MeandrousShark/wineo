#!/bin/bash
# Runs scripts/pack-tarball.sh against fake `wineo` and `wineo-gui` binaries,
# so it needs no build, then extracts the tarball and exercises its
# install.sh: the install and uninstall file lists and modes, the rewritten
# Exec line (including prefixes that need quoting), relative and default
# prefixes, and the failure cases. Nothing here writes outside a temporary
# directory.
#
# Usage: scripts/test-tarball.sh
# Exits non-zero if any case behaves wrongly.
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
pack=$script_dir/pack-tarball.sh
root=$(cd "$script_dir/.." && pwd)

# Modes below assume this, whatever the caller's umask is.
umask 022

# Physical, so paths the tests expect match what install.sh resolves.
tmp=$(cd -P "$(mktemp -d)" && pwd -P)
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
# all stage-install.sh relies on.
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

# usr_listing: the files the tarball ships under usr/, with modes.
usr_listing() {
    sort <<LIST
bin/wineo 755
bin/wineo-gui 755
share/licenses/wineo/LICENSE 644
share/licenses/wineo/THIRD_PARTY.md 644
share/applications/wineo-gui.desktop 644
share/icons/hicolor/scalable/apps/wineo-gui.svg 644
share/doc/wineo/environment.toml.example 644
share/doc/wineo/native-environment.toml.example 644
share/doc/wineo/proxied-environment.toml.example 644
share/bash-completion/completions/wineo 644
share/zsh/site-functions/_wineo 644
share/fish/vendor_completions.d/wineo.fish 644
share/man/man1/wineo.1 644
share/man/man1/wineo-start.1 644
LIST
}
usr_count=$(usr_listing | wc -l)

# check_install NAME PREFIX: PREFIX holds exactly the usr tree.
check_install() {
    if diff <(usr_listing) <(listing "$2") >"$tmp/diff"; then
        pass
    else
        fail "$1: file list or modes differ: $(cat "$tmp/diff")"
    fi
}

# check_exec NAME PREFIX WANT: the installed desktop entry's Exec line.
check_exec() {
    local got
    got=$(grep -c '^Exec=' "$2/share/applications/wineo-gui.desktop" || true)
    if [[ $got -ne 1 ]]; then
        fail "$1: expected one Exec line, found $got"
        return
    fi
    got=$(grep '^Exec=' "$2/share/applications/wineo-gui.desktop")
    if [[ $got == "$3" ]]; then
        pass
    else
        fail "$1: Exec line is '$got', wanted '$3'"
    fi
    # Only the Exec line may differ from the shipped entry.
    if diff <(grep -v '^Exec=' "$tree/usr/share/applications/wineo-gui.desktop") \
        <(grep -v '^Exec=' "$2/share/applications/wineo-gui.desktop") >/dev/null; then
        pass
    else
        fail "$1: more than the Exec line changed in the desktop entry"
    fi
}

# run_ok NAME ARGS...: runs install.sh, which must exit 0. Output goes to
# $tmp/stdout and $tmp/err.
run_ok() {
    local name=$1
    shift
    "$installer" "$@" >"$tmp/stdout" 2>"$tmp/err" && status=0 || status=$?
    if [[ $status -eq 0 ]]; then
        pass
    else
        fail "$name: expected exit 0, got $status ($(cat "$tmp/err"))"
    fi
}

# expect_line NAME WANT: $tmp/stdout has WANT as a whole line.
expect_line() {
    if grep -qxF -- "$2" "$tmp/stdout"; then
        pass
    else
        fail "$1: no line '$2' in: $(cat "$tmp/stdout")"
    fi
}

# expect_no_line NAME UNWANTED: no line of $tmp/stdout starts with it.
expect_no_line() {
    if grep -qF -- "$2" "$tmp/stdout"; then
        fail "$1: unexpected '$2' in: $(cat "$tmp/stdout")"
    else
        pass
    fi
}

# expect_usage_error NAME UNTOUCHED ARGS...
# install.sh must exit 2, print one line to stderr, and not create UNTOUCHED.
expect_usage_error() {
    local name=$1 dir=$2
    shift 2
    "$installer" "$@" >"$tmp/stdout" 2>"$tmp/err" && status=0 || status=$?
    if [[ $status -ne 2 ]]; then
        fail "$name: expected exit 2, got $status"
    elif [[ $(wc -l <"$tmp/err") -ne 1 ]]; then
        fail "$name: message is not one line: $(cat "$tmp/err")"
    elif [[ -s $tmp/stdout ]]; then
        fail "$name: wrote to stdout: $(cat "$tmp/stdout")"
    elif [[ -e $dir ]]; then
        fail "$name: created $dir"
    else
        pass
    fi
}

fake_bins "$tmp/bin"

# --- pack-tarball.sh ---

version=9.9.9
name=wineo-$version-x86_64-linux
"$pack" "$version" "$tmp/bin" "$tmp/out" >"$tmp/stdout" 2>"$tmp/err" && status=0 || status=$?
if [[ $status -eq 0 && -f $tmp/out/$name.tar.gz ]]; then
    pass
else
    fail "pack: expected exit 0 and a tarball, got $status ($(cat "$tmp/err"))"
fi

# Files only, as the tarball names them.
tar -tzf "$tmp/out/$name.tar.gz" | grep -v '/$' | sort >"$tmp/tar-list"
if diff <(
    {
        for top in install.sh README.md LICENSE; do echo "$name/$top"; done
        usr_listing | sed "s|^\\(.*\\) [0-9]*\$|$name/usr/\\1|"
    } | sort
) "$tmp/tar-list" >"$tmp/diff"; then
    pass
else
    fail "tarball file list differs: $(cat "$tmp/diff")"
fi
# Every entry sits under the one top-level directory.
if tar -tzf "$tmp/out/$name.tar.gz" | grep -v "^$name/" | grep -qv "^$name\$"; then
    fail "tarball has entries outside $name/"
else
    pass
fi
# Reproducible ownership.
if tar -tvzf "$tmp/out/$name.tar.gz" | awk '$2 != "0/0"' | grep -q .; then
    fail "tarball entries are not owned by 0/0"
else
    pass
fi

# Extract under a strict umask: install.sh must not depend on it.
mkdir "$tmp/extract"
(umask 077 && tar -xzf "$tmp/out/$name.tar.gz" -C "$tmp/extract")
tree=$tmp/extract/$name
installer=$tree/install.sh

if [[ $(stat -c %a "$tmp/out/$name/install.sh") == 755 &&
    $(stat -c %a "$tmp/out/$name/README.md") == 644 &&
    $(stat -c %a "$tmp/out/$name/LICENSE") == 644 ]]; then
    pass
else
    fail "top-level install.sh, README.md, and LICENSE modes are not 755, 644, 644"
fi
if cmp -s "$root/packaging/tarball/install.sh" "$tmp/out/$name/install.sh" &&
    cmp -s "$root/README.md" "$tmp/out/$name/README.md" &&
    cmp -s "$root/LICENSE" "$tmp/out/$name/LICENSE"; then
    pass
else
    fail "top-level files differ from their sources"
fi
# The staging directory is what was packed.
if diff -r "$tmp/out/$name" "$tree" >/dev/null; then
    pass
else
    fail "tarball contents differ from the staging directory"
fi

# An existing staging directory is refused and left as it was.
echo keep >"$tmp/out/$name/marker"
"$pack" "$version" "$tmp/bin" "$tmp/out" >"$tmp/stdout" 2>"$tmp/err" && status=0 || status=$?
if [[ $status -eq 1 && $(wc -l <"$tmp/err") -eq 1 && $(cat "$tmp/out/$name/marker") == keep ]]; then
    pass
else
    fail "pack over an existing staging dir: status $status, err '$(cat "$tmp/err")'"
fi
rm "$tmp/out/$name/marker"

# Relative BINDIR and OUTDIR, from a directory that isn't the repository.
mkdir "$tmp/elsewhere"
fake_bins "$tmp/elsewhere/mybin"
(cd "$tmp/elsewhere" && "$pack" 1.2.3-rc1 mybin dist) >"$tmp/stdout" 2>"$tmp/err" && status=0 || status=$?
if [[ $status -eq 0 && -f $tmp/elsewhere/dist/wineo-1.2.3-rc1-x86_64-linux.tar.gz &&
    -x $tmp/elsewhere/dist/wineo-1.2.3-rc1-x86_64-linux/usr/bin/wineo ]]; then
    pass
else
    fail "pack with relative paths: status $status ($(cat "$tmp/err"))"
fi

# A failed run leaves no staging directory or tarball behind.
mkdir "$tmp/empty"
"$pack" 1.0.0 "$tmp/empty" "$tmp/out-fail" >"$tmp/stdout" 2>"$tmp/err" && status=0 || status=$?
if [[ $status -eq 1 && ! -e $tmp/out-fail/wineo-1.0.0-x86_64-linux &&
    ! -e $tmp/out-fail/wineo-1.0.0-x86_64-linux.tar.gz ]]; then
    pass
else
    fail "pack without binaries: status $status, left files in out-fail"
fi

# A failure after staging (tar can't write its tarball) removes the staging
# directory too.
mkdir -p "$tmp/out-tarfail/wineo-1.0.0-x86_64-linux.tar.gz"
"$pack" 1.0.0 "$tmp/bin" "$tmp/out-tarfail" >"$tmp/stdout" 2>"$tmp/err" && status=0 || status=$?
if [[ $status -ne 0 && ! -e $tmp/out-tarfail/wineo-1.0.0-x86_64-linux ]]; then
    pass
else
    fail "pack with tar failing: status $status, staging dir left behind"
fi

# Usage errors.
for args in "" "1.0.0 $tmp/bin" "1.0.0 $tmp/bin $tmp/out-usage extra" \
    "../evil $tmp/bin $tmp/out-usage" "a/b $tmp/bin $tmp/out-usage" "-x $tmp/bin $tmp/out-usage"; do
    # Word splitting is intended; none of these paths hold spaces.
    # shellcheck disable=SC2086
    "$pack" $args >"$tmp/stdout" 2>"$tmp/err" && status=0 || status=$?
    if [[ $status -eq 2 && $(wc -l <"$tmp/err") -eq 1 && ! -e $tmp/out-usage ]]; then
        pass
    else
        fail "pack usage '$args': status $status, err '$(cat "$tmp/err")'"
    fi
done

# --- install.sh ---

# Install into a fresh prefix. PATH lacks it, so the hints show.
prefix=$tmp/p1
run_ok "install" "$prefix"
expect_line "install" "Installed $usr_count files to $prefix."
expect_line "install PATH hint" "Add $prefix/bin to your PATH to run wineo."
expect_line "install zsh hint" "For zsh completions, add $prefix/share/zsh/site-functions to your fpath."
check_install "install" "$prefix"
check_exec "install" "$prefix" "Exec=$prefix/bin/wineo-gui"
# Content matches the shipped tree, so installs can't swap files.
# Only the desktop entry may differ.
diff -rq "$tree/usr" "$prefix" >"$tmp/diff" || true
if [[ $(cat "$tmp/diff") == "Files $tree/usr/share/applications/wineo-gui.desktop and $prefix/share/applications/wineo-gui.desktop differ" ]]; then
    pass
else
    fail "installed content differs from usr/: $(cat "$tmp/diff")"
fi

# No PATH hint when PREFIX/bin is on PATH.
PATH=$tmp/p1b/bin:$PATH run_ok "install with bin on PATH" "$tmp/p1b"
expect_no_line "no PATH hint" "Add "
expect_line "still the zsh hint" "For zsh completions, add $tmp/p1b/share/zsh/site-functions to your fpath."

# A trailing slash and a `..` don't leak into the output or the Exec line.
run_ok "install with odd prefix" "$tmp/p1c/sub/../"
expect_line "odd prefix count" "Installed $usr_count files to $tmp/p1c."
check_exec "odd prefix" "$tmp/p1c" "Exec=$tmp/p1c/bin/wineo-gui"

# Installing again overwrites, with the same count, even over damaged files.
echo junk >"$prefix/bin/wineo"
chmod 600 "$prefix/bin/wineo"
run_ok "second install" "$prefix"
expect_line "second install count" "Installed $usr_count files to $prefix."
check_install "second install" "$prefix"
check_exec "second install" "$prefix" "Exec=$prefix/bin/wineo-gui"
if cmp -s "$tree/usr/bin/wineo" "$prefix/bin/wineo"; then
    pass
else
    fail "second install did not restore bin/wineo"
fi

# A prefix with a space: Exec is quoted, files land under the prefix.
prefix="$tmp/my prefix"
run_ok "install into a prefix with a space" "$prefix"
check_install "space prefix" "$prefix"
check_exec "space prefix" "$prefix" "Exec=\"$tmp/my prefix/bin/wineo-gui\""

# Characters the spec escapes inside quotes: " ` $ and \ (each escape
# backslash is doubled again by the file's own string escaping), and % is
# doubled even without quotes.
prefix="$tmp/we\"ird \`x\` \$y \\z"
run_ok "install into a prefix with special characters" "$prefix"
check_install "special prefix" "$prefix"
# shellcheck disable=SC2016
check_exec "special prefix" "$prefix" \
    'Exec="'"$tmp"'/we\\"ird \\`x\\` \\$y \\\\z/bin/wineo-gui"'
prefix=$tmp/100%
run_ok "install into a prefix with a percent sign" "$prefix"
check_install "percent prefix" "$prefix"
check_exec "percent prefix" "$prefix" "Exec=$tmp/100%%/bin/wineo-gui"

# Uninstall removes exactly the shipped files and empty doc/license dirs,
# and leaves unrelated files with their directories.
prefix=$tmp/p2
mkdir -p "$prefix/bin" "$prefix/share/doc/other" "$prefix/share/man/man1"
echo mine >"$prefix/bin/other"
echo mine >"$prefix/share/doc/other/notes.txt"
echo mine >"$prefix/share/man/man1/other.1"
run_ok "install beside unrelated files" "$prefix"
run_ok "uninstall" --uninstall "$prefix"
expect_line "uninstall count" "Removed $usr_count files from $prefix."
if diff <(printf '%s\n' "bin/other 644" "share/doc/other/notes.txt 644" "share/man/man1/other.1 644") \
    <(listing "$prefix") >"$tmp/diff"; then
    pass
else
    fail "uninstall left the wrong files: $(cat "$tmp/diff")"
fi
if [[ -d $prefix/bin && -d $prefix/share/doc/other && -d $prefix/share/man/man1 &&
    ! -e $prefix/share/doc/wineo && ! -e $prefix/share/licenses/wineo ]]; then
    pass
else
    fail "uninstall: wrong directories left (doc/wineo and licenses/wineo should be gone, the rest kept)"
fi

# A file of the user's in doc/wineo keeps that directory.
prefix=$tmp/p3
run_ok "install for doc/wineo test" "$prefix"
echo mine >"$prefix/share/doc/wineo/my-config.toml"
run_ok "uninstall with a file in doc/wineo" --uninstall "$prefix"
if [[ $(listing "$prefix") == "share/doc/wineo/my-config.toml 644" && ! -e $prefix/share/licenses/wineo ]]; then
    pass
else
    fail "uninstall with a user file in doc/wineo: $(listing "$prefix")"
fi

# Uninstall skips files that are already gone.
prefix=$tmp/p4
run_ok "install for partial uninstall" "$prefix"
rm "$prefix/bin/wineo-gui" "$prefix/share/zsh/site-functions/_wineo" "$prefix/share/licenses/wineo/LICENSE"
run_ok "partial uninstall" --uninstall "$prefix"
expect_line "partial uninstall count" "Removed $((usr_count - 3)) files from $prefix."
if [[ -z $(listing "$prefix") ]]; then
    pass
else
    fail "partial uninstall left files: $(listing "$prefix")"
fi

# Uninstalling again, and uninstalling from a prefix that doesn't exist,
# remove nothing and create nothing.
run_ok "repeat uninstall" --uninstall "$prefix"
expect_line "repeat uninstall count" "Removed 0 files from $prefix."
run_ok "uninstall from a missing prefix" --uninstall "$tmp/never"
expect_line "missing prefix count" "Removed 0 files from $tmp/never."
if [[ ! -e $tmp/never ]]; then
    pass
else
    fail "uninstall created a missing prefix"
fi

# Uninstall from a prefix with a space and from a relative one.
prefix="$tmp/my prefix"
run_ok "uninstall from a prefix with a space" --uninstall "$prefix"
expect_line "space prefix uninstall count" "Removed $usr_count files from $prefix."

# A target that is a directory (or a symlink to one) is refused with nothing
# written, and left as it was. The late-sorting target would otherwise let
# earlier files be written first.
prefix=$tmp/p5
mkdir -p "$prefix/share/zsh/site-functions/_wineo" "$prefix/bin"
"$installer" "$prefix" >"$tmp/stdout" 2>"$tmp/err" && status=0 || status=$?
if [[ $status -eq 1 && $(wc -l <"$tmp/err") -eq 1 && -z $(listing "$prefix") &&
    $(cat "$tmp/err") == *"$prefix/share/zsh/site-functions/_wineo"* && ! -s $tmp/stdout ]]; then
    pass
else
    fail "install over a directory target: status $status, err '$(cat "$tmp/err")', files: $(listing "$prefix")"
fi
prefix=$tmp/p6
mkdir -p "$prefix/bin" "$tmp/elsewhere-dir"
ln -s "$tmp/elsewhere-dir" "$prefix/bin/wineo-gui"
"$installer" "$prefix" >"$tmp/stdout" 2>"$tmp/err" && status=0 || status=$?
if [[ $status -eq 1 && $(wc -l <"$tmp/err") -eq 1 && -z $(listing "$tmp/elsewhere-dir") &&
    ! -e $prefix/bin/wineo && -L $prefix/bin/wineo-gui ]]; then
    pass
else
    fail "install over a symlink to a directory: status $status, err '$(cat "$tmp/err")'"
fi

# Uninstall skips a directory at a shipped path, and removes a symlink there
# without touching what it points at.
prefix=$tmp/p7
run_ok "install for skip test" "$prefix"
rm "$prefix/bin/wineo-gui" "$prefix/share/doc/wineo/environment.toml.example"
mkdir "$prefix/bin/wineo-gui"
echo mine >"$prefix/bin/wineo-gui/keep"
echo outside >"$tmp/outside-file"
ln -s "$tmp/outside-file" "$prefix/share/doc/wineo/environment.toml.example"
run_ok "uninstall with a directory at a shipped path" --uninstall "$prefix"
expect_line "skip count" "Removed $((usr_count - 1)) files from $prefix."
if [[ $(listing "$prefix") == "bin/wineo-gui/keep 644" && $(cat "$tmp/outside-file") == outside ]]; then
    pass
else
    fail "uninstall skip test: $(listing "$prefix")"
fi

# Dots after a symlink are taken physically: logical/link -> physical/child,
# so logical/link/.. is physical, not logical.
mkdir -p "$tmp/logical/bin" "$tmp/physical/child"
ln -s "$tmp/physical/child" "$tmp/logical/link"
echo mine >"$tmp/logical/bin/wineo"
run_ok "install through a symlink and .." "$tmp/logical/link/.."
expect_line "symlink install" "Installed $usr_count files to $tmp/physical."
check_exec "symlink install" "$tmp/physical" "Exec=$tmp/physical/bin/wineo-gui"
run_ok "uninstall through a symlink and .." --uninstall "$tmp/logical/link/.."
expect_line "symlink uninstall" "Removed $usr_count files from $tmp/physical."
if [[ $(cat "$tmp/logical/bin/wineo") == mine && ! -e $tmp/physical/bin/wineo ]]; then
    pass
else
    fail "uninstall through a symlink touched the wrong tree"
fi

# `/` can't be written to here, so check what the root prefix is built from:
# no trailing slashes, and an empty base that still joins to absolute paths.
# HOME is redirected so a broken source guard can't install into the real one.
got=$(HOME=$tmp/source-home bash -c '
    source "$1"
    printf "[%s]" "$(base_of /)" "$(base_of //)" "$(base_of /a//)" "$(base_of /usr/local)"
' _ "$installer" 2>&1) || true
if [[ $got == '[][][/a][/usr/local]' ]]; then
    pass
else
    fail "base_of gave $got"
fi
got=$(HOME=$tmp/source-home bash -c '
    source "$1"
    base=$(base_of /)
    echo "$base/bin/wineo"
' _ "$installer" 2>&1) || true
if [[ $got == /bin/wineo ]]; then
    pass
else
    fail "the root prefix joins to '$got', not /bin/wineo"
fi

# From another directory, with a relative PREFIX.
mkdir "$tmp/cwd"
(cd "$tmp/cwd" && "$installer" rel/prefix) >"$tmp/stdout" 2>"$tmp/err" && status=0 || status=$?
if [[ $status -eq 0 ]]; then pass; else fail "relative install: status $status ($(cat "$tmp/err"))"; fi
expect_line "relative install" "Installed $usr_count files to $tmp/cwd/rel/prefix."
check_install "relative install" "$tmp/cwd/rel/prefix"
check_exec "relative install" "$tmp/cwd/rel/prefix" "Exec=$tmp/cwd/rel/prefix/bin/wineo-gui"
(cd "$tmp/cwd" && "$installer" --uninstall rel/prefix) >"$tmp/stdout" 2>"$tmp/err" && status=0 || status=$?
if [[ $status -eq 0 ]]; then pass; else fail "relative uninstall: status $status ($(cat "$tmp/err"))"; fi
expect_line "relative uninstall" "Removed $usr_count files from $tmp/cwd/rel/prefix."
if [[ -z $(listing "$tmp/cwd/rel/prefix") ]]; then
    pass
else
    fail "relative uninstall left files: $(listing "$tmp/cwd/rel/prefix")"
fi

# Default PREFIX is $HOME/.local.
mkdir "$tmp/home"
HOME=$tmp/home run_ok "default prefix"
expect_line "default prefix" "Installed $usr_count files to $tmp/home/.local."
check_install "default prefix" "$tmp/home/.local"
check_exec "default prefix" "$tmp/home/.local" "Exec=$tmp/home/.local/bin/wineo-gui"
HOME=$tmp/home run_ok "default prefix uninstall" --uninstall
expect_line "default prefix uninstall" "Removed $usr_count files from $tmp/home/.local."

# The unextracted source script has no tree beside it.
"$root/packaging/tarball/install.sh" "$tmp/from-source" >"$tmp/stdout" 2>"$tmp/err" && status=0 || status=$?
if [[ $status -eq 1 && $(wc -l <"$tmp/err") -eq 1 && ! -e $tmp/from-source ]]; then
    pass
else
    fail "install.sh without a usr/ tree: status $status, err '$(cat "$tmp/err")'"
fi

# Usage errors write nothing, including the default prefix.
mkdir "$tmp/home2"
HOME=$tmp/home2 expect_usage_error "unknown option" "$tmp/home2/.local" --nope
HOME=$tmp/home2 expect_usage_error "unknown option with prefix" "$tmp/u1" --nope "$tmp/u1"
HOME=$tmp/home2 expect_usage_error "too many arguments" "$tmp/u2" "$tmp/u2" extra
HOME=$tmp/home2 expect_usage_error "uninstall with too many arguments" "$tmp/u3" --uninstall "$tmp/u3" extra
HOME=$tmp/home2 expect_usage_error "empty prefix" "$tmp/home2/.local" ""
HOME=$tmp/home2 expect_usage_error "prefix with a newline" "$tmp/u4" "$tmp/u4"$'\n'x
HOME=$tmp/home2 expect_usage_error "prefix with a tab" "$tmp/u6" "$tmp/u6"$'\t'x
HOME=$tmp/home2 expect_usage_error "prefix with a carriage return" "$tmp/u7" "$tmp/u7"$'\r'x
HOME=$tmp/home2 expect_usage_error "uninstall prefix with a tab" "$tmp/u8" --uninstall "$tmp/u8"$'\t'x
saved_home=$HOME
unset HOME
expect_usage_error "no HOME and no prefix" "$tmp/u5"
HOME=$saved_home

echo "$((cases - failures)) of $cases cases behaved correctly"
[[ $failures -eq 0 ]]
