#!/bin/bash
# Print the body of one version's CHANGELOG.md section: the lines after
# "## X.Y.Z - " up to the next "## " heading, without the heading itself
# and with blank lines trimmed from both ends. Used by the release guard
# (is there a section?) and by the publish job (the release notes), so the
# two can't disagree.
#
# Usage: scripts/changelog-section.sh VERSION [FILE]
#   FILE   defaults to CHANGELOG.md; "-" reads standard input
#
# Exits 1 when there is no "## VERSION - " heading. An existing section
# with no text under it prints nothing and exits 0.
set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 ]]; then
    echo "usage: ${0##*/} VERSION [FILE]" >&2
    exit 2
fi

version=$1
file=${2:-CHANGELOG.md}
[[ $file == - ]] && file=/dev/stdin

# The version is matched as plain text, not as a regex, so dots stay dots.
awk -v prefix="## $version - " '
    !found && index($0, prefix) == 1 { found = 1; next }
    found && index($0, "## ") == 1 { exit }
    found {
        if ($0 ~ /^[[:space:]]*$/) { blanks++; next }
        if (started) for (; blanks > 0; blanks--) print ""
        blanks = 0
        started = 1
        print
    }
    END { if (!found) exit 1 }
' "$file"
