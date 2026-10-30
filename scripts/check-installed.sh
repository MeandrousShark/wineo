#!/bin/bash
# Check that a wineo install under PREFIX is complete: both binaries
# executable in bin/, the desktop entry and icon, the wineo man page plus at
# least one other, the bash, zsh, and fish completions, and LICENSE and
# THIRD_PARTY.md in LICENSE_DIR. Man pages may be plain (.1) or gzipped
# (.1.gz), since the deb gzips them and the rpm and tarball don't.
#
# Usage: scripts/check-installed.sh PREFIX LICENSE_DIR
#
# PREFIX is /usr for a package or the temp prefix for install.sh. LICENSE_DIR
# is an absolute path (/usr/share/licenses/wineo for the rpm) or one under
# PREFIX. Prints one line per missing item. Exits 2 on bad usage and 1 when
# anything is missing.
set -euo pipefail

usage() {
    echo "usage: ${0##*/} PREFIX LICENSE_DIR" >&2
    exit 2
}

[[ $# -eq 2 ]] || usage
prefix=${1%/}
license_dir=${2%/}
[[ -n $prefix && -n $license_dir ]] || usage

missing=0
miss() {
    echo "missing: $*"
    missing=$((missing + 1))
}

for bin in wineo wineo-gui; do
    if [[ ! -f $prefix/bin/$bin ]]; then
        miss "$prefix/bin/$bin"
    elif [[ ! -x $prefix/bin/$bin ]]; then
        miss "$prefix/bin/$bin is not executable"
    fi
done

for file in \
    share/applications/wineo-gui.desktop \
    share/icons/hicolor/scalable/apps/wineo-gui.svg \
    share/bash-completion/completions/wineo \
    share/zsh/site-functions/_wineo \
    share/fish/vendor_completions.d/wineo.fish; do
    [[ -f $prefix/$file ]] || miss "$prefix/$file"
done

[[ -f $license_dir/LICENSE ]] || miss "$license_dir/LICENSE"
[[ -f $license_dir/THIRD_PARTY.md ]] || miss "$license_dir/THIRD_PARTY.md"

man=$prefix/share/man/man1
if [[ ! -f $man/wineo.1 && ! -f $man/wineo.1.gz ]]; then
    miss "$man/wineo.1 or wineo.1.gz"
fi
others=0
for page in "$man"/wineo-*.1 "$man"/wineo-*.1.gz; do
    [[ -f $page ]] && others=$((others + 1))
done
if [[ $others -eq 0 ]]; then
    miss "a second man page in $man (wineo-*.1 or wineo-*.1.gz)"
fi

if [[ $missing -gt 0 ]]; then
    echo "${0##*/}: $missing item(s) missing under $prefix" >&2
    exit 1
fi
echo "installed tree under $prefix is complete"
