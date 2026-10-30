#!/bin/bash
# Print the oldest Rust that can build wineo: the highest rust-version
# across the locked dependency graph, followed by the crate that sets it,
# like "1.90 ordered-float 5.5.0". Repository builds use each distro's own
# Rust, so this decides which releases they can target, and a dependency
# update can raise it.
#
# Dev-dependencies count, since package builds run the test suite. Only
# x86_64 Linux dependencies count, since that's all wineo ships for.
#
# Usage: scripts/rust-floor.sh
#
# Exits 1 when no dependency declares a rust-version.
set -euo pipefail

cd "$(dirname "$0")/.."

cargo metadata --format-version 1 --locked --offline \
    --filter-platform x86_64-unknown-linux-gnu |
    jq -r '
        [.packages[] | select(.rust_version)]
        | max_by(.rust_version | split(".") | map(tonumber) | . + [0, 0, 0] | .[:3])
        | if . == null then
            "no dependency declares a rust-version\n" | halt_error(1)
          else
            "\(.rust_version) \(.name) \(.version)"
          end
    '
