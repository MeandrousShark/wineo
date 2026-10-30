#!/bin/bash
# Runs scripts/pack-packages.sh against fake `wineo` and `wineo-gui` binaries,
# so it needs no build, then checks the .deb and .rpm it makes: their names,
# their metadata, and that their files match the staged install tree. It
# also covers bad usage and failed runs. Nothing here writes outside a
# temporary directory.
#
# Needs nfpm, from $NFPM or PATH (scripts/fetch-nfpm.sh fetches the pinned
# one). The deb checks need dpkg-deb and the rpm checks need rpm; a missing
# one skips its checks with a "skipped" line.
#
# Usage: scripts/test-packages.sh
# Exits non-zero if any case behaves wrongly or nfpm is missing.
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
pack=$script_dir/pack-packages.sh
root=$(cd "$script_dir/.." && pwd)

nfpm=${NFPM:-nfpm}
if ! command -v "$nfpm" >/dev/null; then
    echo "${0##*/}: nfpm not found (set NFPM or put it on PATH)" >&2
    exit 1
fi
NFPM=$(command -v "$nfpm")
export NFPM

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

# run NAME WANT ARGS...: runs pack-packages.sh, which must exit WANT.
run() {
    local name=$1 want=$2
    shift 2
    "$pack" "$@" >"$tmp/stdout" 2>"$tmp/err" && status=0 || status=$?
    if [[ $status -eq $want ]]; then
        pass
    else
        fail "$name: expected exit $want, got $status ($(cat "$tmp/err"))"
    fi
}

version=1.2.3
bins=$tmp/bins
fake_bins "$bins"

# Both lists come from the README, the same way the script reads them.
ubuntu=$("$root/scripts/readme-runtime-packages.sh" --list ubuntu "$root/README.md")
fedora=$("$root/scripts/readme-runtime-packages.sh" --list fedora "$root/README.md")
# README.md's opening paragraph, which is the packages' long description.
paragraph=$(awk '/^A launcher and library for Linux/ { on = 1 } on && !NF { exit } on' "$root/README.md")
summary='Launcher for Wine and Proton programs, native games, Steam, and GOG'
if [[ -z $paragraph || -z $ubuntu || -z $fedora ]]; then
    echo "${0##*/}: could not read the README's description or package lists" >&2
    exit 1
fi

out=$tmp/out
run "pack" 0 "$version" "$bins" "$out"
deb=$out/wineo_${version}_amd64.deb
rpm=$out/wineo-$version-1.x86_64.rpm

same "outputs" "$(printf '%s\n' "${deb##*/}" "${rpm##*/}" | LC_ALL=C sort)" "$(find "$out" -mindepth 1 -printf '%f\n' | LC_ALL=C sort)"

# The trees the packages must match, from the same script the packager uses.
# The deb also gets a copyright file, and its man pages are gzipped.
stage_rpm=$tmp/stage-rpm
stage_deb=$tmp/stage-deb
"$root/scripts/stage-install.sh" "$stage_rpm" "$bins" >/dev/null
"$root/scripts/stage-install.sh" --license-dir /usr/share/doc/wineo "$stage_deb" "$bins" >/dev/null
want_deb=$(
    {
        listing "$stage_deb/usr" | sed -E 's#^(share/man/man1/.*\.1) #\1.gz #'
        echo "share/doc/wineo/copyright 644"
    } | sort
)

if command -v dpkg-deb >/dev/null; then
    field() { dpkg-deb -f "$deb" "$1"; }
    join() { paste -sd, - | sed 's/,/, /g'; }
    same "deb Package" wineo "$(field Package)"
    same "deb Version" "$version" "$(field Version)"
    same "deb Architecture" amd64 "$(field Architecture)"
    same "deb Maintainer" "Michael Hannon <mphannon@pm.me>" "$(field Maintainer)"
    same "deb Section" otherosfs "$(field Section)"
    same "deb Priority" optional "$(field Priority)"
    same "deb Homepage" https://github.com/MeandrousShark/wineo "$(field Homepage)"
    same "deb Depends" "libc6 (>= 2.35), libgcc-s1, $(join <<<"$ubuntu")" "$(field Depends)"
    same "deb Recommends" "wine, winetricks" "$(field Recommends)"
    same "deb Suggests" "gamemode, gamescope, mangohud" "$(field Suggests)"
    same "deb summary" "$summary" "$(field Description | head -n 1)"
    same "deb long description" "$paragraph" "$(field Description | tail -n +2 | sed 's/^ //')"

    # Every entry is root-owned, and nothing is outside /usr.
    if dpkg-deb -c "$deb" | awk '$2 != "root/root" || $6 !~ /^\.\/usr(\/|$)/ { bad = 1 } END { exit bad }'; then
        pass
    else
        fail "deb: an entry is not root-owned or not under /usr"
    fi
    extracted=$tmp/extracted-deb
    mkdir "$extracted"
    dpkg-deb -x "$deb" "$extracted"
    same "deb top level" usr "$(ls -A "$extracted")"
    if diff <(echo "$want_deb") <(listing "$extracted/usr") >"$tmp/diff"; then
        pass
    else
        fail "deb: file list or modes differ from the staged tree: $(cat "$tmp/diff")"
    fi
    for file in bin/wineo bin/wineo-gui share/doc/wineo/LICENSE share/doc/wineo/THIRD_PARTY.md \
        share/applications/wineo-gui.desktop share/bash-completion/completions/wineo; do
        if cmp -s "$stage_deb/usr/$file" "$extracted/usr/$file"; then
            pass
        else
            fail "deb: $file differs from the staged file"
        fi
    done
    copyright=$extracted/usr/share/doc/wineo/copyright
    same "copyright format" "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/" \
        "$(head -n 1 "$copyright")"
    for line in "Upstream-Name: wineo" "Source: https://github.com/MeandrousShark/wineo" \
        "Files: *" "Copyright: 2026 Michael Hannon" "License: GPL-3.0-or-later"; do
        if grep -qxF "$line" "$copyright"; then
            pass
        else
            fail "copyright lacks the line '$line'"
        fi
    done
    for text in /usr/share/common-licenses/GPL-3 THIRD_PARTY.md; do
        if grep -qF "$text" "$copyright"; then
            pass
        else
            fail "copyright does not mention $text"
        fi
    done
    if zcat "$extracted/usr/share/man/man1/wineo.1.gz" | grep -qx fake; then
        pass
    else
        fail "deb: the man page did not survive gzip"
    fi
else
    echo "skipped: no dpkg-deb, so the deb checks did not run"
fi

if command -v rpm >/dev/null; then
    query() { rpm -qp --qf "$1" "$rpm"; }
    same "rpm name" wineo "$(query '%{NAME}')"
    same "rpm version" "$version" "$(query '%{VERSION}')"
    same "rpm release" 1 "$(query '%{RELEASE}')"
    same "rpm arch" x86_64 "$(query '%{ARCH}')"
    same "rpm summary" "$summary" "$(query '%{SUMMARY}')"
    same "rpm license" GPL-3.0-or-later "$(query '%{LICENSE}')"
    same "rpm packager" "Michael Hannon <mphannon@pm.me>" "$(query '%{PACKAGER}')"
    same "rpm url" https://github.com/MeandrousShark/wineo "$(query '%{URL}')"
    same "rpm description" "$summary
$paragraph" "$(query '%{DESCRIPTION}')"
    # rpm adds its own rpmlib(...) requirements.
    same "rpm requires" "$(printf '%s\n' 'glibc >= 2.35' libgcc "$fedora")" \
        "$(rpm -qp --requires "$rpm" | grep -v '^rpmlib(')"
    same "rpm recommends" "$(printf '%s\n' wine winetricks umu-launcher)" \
        "$(rpm -qp --recommends "$rpm")"

    # Regular files with mode and SHA-256, against the staged tree.
    want_rpm=$(
        cd "$stage_rpm/usr"
        find . -mindepth 1 ! -type d -printf '%P\n' | sort | while read -r file; do
            printf '/usr/%s %s %s\n' "$file" "$(stat -c %a "$file")" "$(sha256sum <"$file" | cut -d' ' -f1)"
        done
    )
    got_rpm=$(rpm -qp --qf '[%{FILENAMES} %{FILEMODES:octal} %{FILEDIGESTS} %{FILEUSERNAME}:%{FILEGROUPNAME}\n]' "$rpm" |
        awk '$2 ~ /^100/ { print $1, substr($2, 4), $3, $4 }' | sort)
    # The user:group column is checked on its own so the diff stays readable.
    if [[ -z $(awk '$4 != "root:root"' <<<"$got_rpm") ]]; then
        pass
    else
        fail "rpm: a file is not owned by root:root"
    fi
    if diff <(echo "$want_rpm") <(awk '{ print $1, $2, $3 }' <<<"$got_rpm") >"$tmp/diff"; then
        pass
    else
        fail "rpm: file list, modes, or contents differ from the staged tree: $(cat "$tmp/diff")"
    fi
    # The directories only wineo uses belong to the package, so removing it
    # leaves none behind.
    dirs=$(rpm -qp --qf '[%{FILENAMES} %{FILEMODES:octal}\n]' "$rpm" | awk '$2 ~ /^40/ { print $1 }')
    for dir in /usr/share/doc/wineo /usr/share/licenses/wineo; do
        if grep -qxF "$dir" <<<"$dirs"; then
            pass
        else
            fail "rpm: $dir is not owned by the package"
        fi
    done
else
    echo "skipped: no rpm, so the rpm checks did not run"
fi

# Bad usage exits 2 and writes nothing.
run "no arguments" 2
run "two arguments" 2 "$version" "$bins"
run "four arguments" 2 "$version" "$bins" "$tmp/x" extra
run "empty version" 2 "" "$bins" "$tmp/x"
run "version with a slash" 2 "1/2" "$bins" "$tmp/x"
run "version with a dash" 2 "1.0-rc1" "$bins" "$tmp/x"
run "version without a leading digit" 2 "v1.0" "$bins" "$tmp/x"
if [[ ! -e $tmp/x ]]; then
    pass
else
    fail "bad usage created an output directory"
fi

# A second run into the same directory is refused and changes nothing.
before=$(sha256sum "$deb" "$rpm")
run "existing outputs" 1 "$version" "$bins" "$out"
same "existing outputs untouched" "$before" "$(sha256sum "$deb" "$rpm")"
# One existing output is enough to refuse.
rm "$rpm"
run "existing deb only" 1 "$version" "$bins" "$out"
same "existing deb only: nothing added" "${deb##*/}" "$(ls -A "$out")"

# Different versions can share a directory.
run "second version in the same directory" 0 1.2.4 "$bins" "$out"
same "second version outputs" 3 "$(find "$out" -type f | wc -l)"

# A relative BINDIR and OUTDIR are relative to the working directory.
mkdir "$tmp/cwd"
(cd "$tmp/cwd" && "$pack" "$version" ../bins rel-out >/dev/null 2>"$tmp/err") && status=0 || status=$?
if [[ $status -eq 0 && -f $tmp/cwd/rel-out/wineo_${version}_amd64.deb && -f $tmp/cwd/rel-out/wineo-$version-1.x86_64.rpm ]]; then
    pass
else
    fail "relative paths: exit $status ($(cat "$tmp/err"))"
fi

# Binaries that are missing fail before any package is made.
mkdir "$tmp/nobins"
run "missing binaries" 1 "$version" "$tmp/nobins" "$tmp/out-nobins"
if [[ -z $(ls -A "$tmp/out-nobins") ]]; then
    pass
else
    fail "missing binaries left files: $(ls -A "$tmp/out-nobins")"
fi

# No nfpm is a clear failure that writes nothing.
NFPM=$tmp/no-such-nfpm run "missing nfpm" 1 "$version" "$bins" "$tmp/out-nonfpm"
if grep -q 'nfpm not found' "$tmp/err" && [[ ! -e $tmp/out-nonfpm ]]; then
    pass
else
    fail "missing nfpm: wrong message or output ($(cat "$tmp/err"))"
fi

# An nfpm that writes the deb and then fails on the rpm leaves no package
# behind, but files that were already there stay.
cat >"$tmp/fake-nfpm" <<'FAKE'
#!/bin/bash
while [[ $# -gt 0 ]]; do
    case $1 in
        --packager) packager=$2 ;;
        --target) target=$2 ;;
    esac
    shift
done
echo partial >"$target"
[[ $packager == deb ]]
FAKE
chmod +x "$tmp/fake-nfpm"
mkdir "$tmp/out-fail"
echo mine >"$tmp/out-fail/notes.txt"
NFPM=$tmp/fake-nfpm run "failing nfpm" 1 "$version" "$bins" "$tmp/out-fail"
same "failing nfpm cleanup" notes.txt "$(ls -A "$tmp/out-fail")"

echo "$((cases - failures)) of $cases cases behaved correctly"
[[ $failures -eq 0 ]]
