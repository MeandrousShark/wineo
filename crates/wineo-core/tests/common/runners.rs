/// A program named exit-N exits with N right away, a program named stubborn
/// ignores requests to close, a program named slow takes a moment before
/// wineserver can see it, noisy writes megabytes in bursts, noisy-exit leaves
/// a background writer behind, handoff exits and leaves something running for
/// a couple of seconds the way an installer hands off to its child,
/// break-wineserver deletes the wineserver beside it, a program whose name ends
/// in install-game.exe installs a game into the prefix, and any other program
/// runs until `wineboot --end-session` or `wineserver -k` ends it.
pub const FAKE_WINE: &str = r#"#!/bin/sh
pids="$WINEPREFIX/fake-pids"
burst() {
    line=$(printf 'x%.0s' $(seq 1023))
    i=0
    while [ "$i" -lt 2048 ]; do
        echo "$line"
        i=$((i + 1))
    done
}
case "$1" in
--version)
    echo wine-fake
    exit 0
    ;;
wineboot)
    if [ "$2" = --init ]; then
        mkdir -p "$WINEPREFIX/drive_c"
        printf 'WINE REGISTRY Version 2\n\n#arch=win64\n' >"$WINEPREFIX/system.reg"
        if [ -n "${WINEO_TEST_REMOVE_INSTALLER-}" ]; then
            rm -f "$WINEO_TEST_REMOVE_INSTALLER"
        fi
    elif [ "$2" = --end-session ] && read -r program _ 2>/dev/null <"$pids"; then
        kill "$program" 2>/dev/null
    fi
    exit 0
    ;;
reg)
    echo "$*" >>"$WINEPREFIX/reg.log"
    if [ -n "${WINEO_TEST_REMOVE_WINE-}" ]; then
        rm -f "$WINEO_TEST_REMOVE_WINE"
    fi
    exit 0
    ;;
winemenubuilder.exe)
    echo "$*" >>"$WINEPREFIX/menubuilder.log"
    exit 1
    ;;
esac
echo "fake wine: $*"
echo "fake wine: FAKE_VAR=${FAKE_VAR-}"
case "$1" in
exit-*)
    exit "${1#exit-}"
    ;;
slow)
    sleep 0.5
    ;;
noisy)
    # Two megabytes, a pause long enough for the size check to run, then two
    # more, so the log is cleared while the program is still going.
    burst
    sleep 2
    burst
    ;;
noisy-exit)
    # Exits right away, leaving something in the prefix still writing.
    (sleep 0.5; burst; burst; rm -f "$pids") &
    echo "$! $!" >"$pids"
    exit 0
    ;;
handoff)
    (sleep 2; touch "$WINEPREFIX/handoff-done"; rm -f "$pids") &
    echo "$! $!" >"$pids"
    exit 0
    ;;
break-wineserver)
    rm -f "${0%/*}/wineserver"
    exit 0
    ;;
*install-game.exe)
    mkdir -p "$WINEPREFIX/drive_c/Games/Game"
    : >"$WINEPREFIX/drive_c/Games/Game/Game.exe"
    exit 0
    ;;
esac
sleep 60 &
if [ "$1" = stubborn ]; then
    trap '' TERM
else
    trap 'kill "$!" 2>/dev/null; rm -f "$pids"; exit 0' TERM
fi
echo "$$ $!" >"$pids"
wait
rm -f "$pids"
"#;

/// Treats the fake wine program as the only process in the prefix.
pub const FAKE_WINESERVER: &str = r#"#!/bin/sh
pids="$WINEPREFIX/fake-pids"
case "$1" in
-w)
    while read -r program _ 2>/dev/null <"$pids" && kill -0 "$program" 2>/dev/null; do
        sleep 0.1
    done
    ;;
-k)
    if read -r program sleeper 2>/dev/null <"$pids"; then
        kill -9 "$program" "$sleeper" 2>/dev/null
        rm -f "$pids"
    fi
    ;;
esac
exit 0
"#;

/// Announces itself into the run's output, then becomes what it was asked to
/// wrap, so a test can see both that it ran and that the program still did.
pub const FAKE_GAMEMODERUN: &str = r#"#!/bin/sh
echo "gamemoderun wrapped: $1"
exec "$@"
"#;

/// Stands in for umu-launcher: it records what it was asked to do, makes `pfx/`
/// under the compatdata directory when handed an empty program, which is how umu
/// creates a prefix, and otherwise runs like the fake wine so the same programs
/// work under both kinds. Its handoff leaves something in umu's process group.
pub const FAKE_UMU: &str = r#"#!/bin/sh
echo "umu-run: $*"
echo "umu-run: WINEPREFIX=$WINEPREFIX PROTONPATH=$PROTONPATH"
echo "umu-run: GAMEID=${GAMEID-unset} STORE=${STORE-unset}"
if [ -z "$1" ]; then
    mkdir -p "$WINEPREFIX/pfx/drive_c"
    printf 'WINE REGISTRY Version 2\n\n#arch=win64\n' >"$WINEPREFIX/pfx/system.reg"
    # Like the real one, which makes the prefix and then fails to run the
    # empty program.
    echo "ShellExecuteEx failed: File not found."
    exit 1
fi
echo "umu-run: FAKE_VAR=${FAKE_VAR-}"
pids="$WINEPREFIX/fake-pids"
case "$1" in
winetricks)
    shift
    if [ "${1-}" = -q ]; then
        shift
        for verb in "$@"; do
            if [ "$verb" = "${WINEO_TEST_FAIL_VERB-}" ]; then
                exit 1
            fi
            printf '%s\n' "$verb" >>"$WINEPREFIX/pfx/winetricks.log"
        done
    fi
    exit 0
    ;;
reg | winecfg)
    exit 0
    ;;
exit-*)
    exit "${1#exit-}"
    ;;
stubborn)
    # Nothing outside the container can ask it to close, so only SIGKILL to
    # the process group ends this.
    trap '' TERM
    echo "$$ $$" >"$pids"
    while :; do sleep 0.1; done
    ;;
handoff)
    # Leaves something in umu's process group after umu itself has exited.
    (sleep 2; touch "$WINEPREFIX/handoff-done") &
    exit 0
    ;;
esac
sleep 60 &
trap 'kill "$!" 2>/dev/null; rm -f "$pids"; exit 0' TERM
echo "$$ $!" >"$pids"
wait
rm -f "$pids"
"#;
