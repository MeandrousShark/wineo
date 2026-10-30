#!/bin/bash
# Print a runtime package list from the README, one package per line. Each
# list lives in a plain code fence between two marker comments, so the
# README, the release verify job, and the packages read the same source:
#
#   <!-- ubuntu-runtime-packages:start -->
#   ```
#   libx11-6
#   ```
#   <!-- ubuntu-runtime-packages:end -->
#
# Usage: scripts/readme-runtime-packages.sh [--list ubuntu|fedora] [FILE]
#
# The list defaults to ubuntu and FILE to README.md. Exits 2 on bad usage
# and 1 when a marker is missing or the block holds no packages.
set -euo pipefail

usage() {
    echo "usage: ${0##*/} [--list ubuntu|fedora] [FILE]" >&2
    exit 2
}

list=ubuntu
if [[ ${1:-} == --list ]]; then
    [[ $# -ge 2 ]] || usage
    list=$2
    shift 2
fi
[[ $list == ubuntu || $list == fedora ]] || usage
[[ $# -le 1 ]] || usage

file=${1:-README.md}

awk -v list="$list" '
    $0 == "<!-- " list "-runtime-packages:start -->" { in_block = 1; started = 1; next }
    $0 == "<!-- " list "-runtime-packages:end -->" { if (in_block) ended = 1; in_block = 0; next }
    in_block && /^```/ { next }
    in_block && NF { gsub(/^[[:space:]]+|[[:space:]]+$/, ""); print; count++ }
    END { if (!started || !ended || !count) exit 1 }
' "$file"
