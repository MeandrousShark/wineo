#!/bin/bash
# Print the Ubuntu runtime package list from the README, one package per
# line. The list lives in a plain code fence between two marker comments,
# so the README and the release verify job read the same source:
#
#   <!-- ubuntu-runtime-packages:start -->
#   ```
#   libx11-6
#   ```
#   <!-- ubuntu-runtime-packages:end -->
#
# Usage: scripts/readme-runtime-packages.sh [FILE]   (default README.md)
#
# Exits 1 when a marker is missing or the block holds no packages.
set -euo pipefail

file=${1:-README.md}

awk '
    $0 == "<!-- ubuntu-runtime-packages:start -->" { in_block = 1; started = 1; next }
    $0 == "<!-- ubuntu-runtime-packages:end -->" { if (in_block) ended = 1; in_block = 0; next }
    in_block && /^```/ { next }
    in_block && NF { gsub(/^[[:space:]]+|[[:space:]]+$/, ""); print; count++ }
    END { if (!started || !ended || !count) exit 1 }
' "$file"
