#!/bin/bash
# Start BINARY (a built wineo-gui) under a private Xvfb and check that it
# stays up. Exit 124 from timeout means the app was still running when the
# time ran out; any other exit, or a panic in the output, is a failure.
#
# Everything lives in a throwaway HOME and XDG tree, with no display but the
# private Xvfb. iced loads X11, xkbcommon, and graphics libraries at
# runtime, which ldd can't see; the software renderer is forced because CI
# containers have no GPU. Needs xvfb-run (and xauth) on PATH.
#
# Usage: scripts/check-gui-starts.sh BINARY
#
# Prints the app's log. Exits 2 on bad usage and 1 when the app fails.
set -euo pipefail

usage() {
    echo "usage: ${0##*/} BINARY" >&2
    exit 2
}

[[ $# -eq 1 ]] || usage
gui=$1
if [[ ! -f $gui || ! -x $gui ]]; then
    echo "${0##*/}: $gui is missing or not executable" >&2
    exit 1
fi
# Absolute before the environment changes anything.
[[ $gui == /* ]] || gui=$PWD/$gui

home=$(mktemp -d)
trap 'rm -rf "${home:?}"' EXIT
mkdir -p "$home"/{config,data,state,cache,run}
chmod 700 "$home/run"
log=$home/gui.log

unset DISPLAY WAYLAND_DISPLAY XAUTHORITY DBUS_SESSION_BUS_ADDRESS
export HOME=$home XDG_CONFIG_HOME=$home/config XDG_DATA_HOME=$home/data \
    XDG_STATE_HOME=$home/state XDG_CACHE_HOME=$home/cache \
    XDG_RUNTIME_DIR=$home/run ICED_BACKEND=tiny-skia

set +e
xvfb-run -a -s '-screen 0 1280x800x24' timeout 10 "$gui" >"$log" 2>&1
status=$?
set -e

cat "$log"
if [[ $status -ne 124 ]]; then
    echo "::error::${gui##*/} exited with status $status instead of staying up"
    exit 1
fi
if grep -qi 'panic' "$log"; then
    echo "::error::${gui##*/} panicked"
    exit 1
fi
