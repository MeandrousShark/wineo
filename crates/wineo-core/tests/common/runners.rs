/// Stands in for `cmd /c FILE.bat` with one of wineo's Galaxy setup batch
/// files: `@echo off`, `cd /d "DIR"`, `"PROGRAM" ARGS`, and
/// `>"CODE" echo %ERRORLEVEL%`. Like cmd in a batch file, it expands each
/// `%NAME%` once from its environment, an unset one to nothing, so a missing
/// variable fails as it would for real. It appends the program, folder, and
/// arguments, tab-separated, to `${FAKE_CMD_LOG:-$WINEPREFIX/fake-cmd.log}`.
/// A program that isn't there gets 9009; otherwise the code is in
/// `PROGRAM.exit` beside it (0 without one), and `PROGRAM.nocode` beside it
/// means no code file is written. When the program is msiexec, the package
/// after `/i` counts as it. The host gets the low 8 bits.
///
/// A macro, so `FAKE_WINE` and `FAKE_UMU` can include it with `concat!`.
macro_rules! fake_cmd {
    () => {
        r#"fake_cmd() {
    # Wine sets it for every program.
    : "${SystemRoot:=C:\\windows}"
    fake_unix() { printf '%s' "$1" | sed 's/^[Zz]://; s/\\/\//g'; }
    # Expands each %NAME% once from the environment, as cmd does in a batch
    # file: an unset one is empty, `%%` is `%`, and what a value holds is
    # never expanded again.
    fake_expand() {
        rest=$1
        expanded=
        while :; do
            case $rest in
            *%*%*) ;;
            *) break ;;
            esac
            expanded=$expanded${rest%%\%*}
            rest=${rest#*%}
            name=${rest%%\%*}
            rest=${rest#*%}
            case $name in
            '') expanded=$expanded% ;;
            *[!A-Za-z0-9_]* | [0-9]*) expanded=$expanded%$name% ;;
            *)
                value=
                eval "value=\${$name-}"
                expanded=$expanded$value
                ;;
            esac
        done
        printf '%s' "$expanded$rest"
    }
    fake_line() { fake_expand "$(sed -n "$1p" "$2" | tr -d '\r')"; }
    bat=$(fake_unix "$1")
    dir=$(fake_line 2 "$bat" | sed 's/^cd \/d "\(.*\)"$/\1/')
    line=$(fake_line 3 "$bat")
    out=$(sed -n '4p' "$bat" | tr -d '\r' | sed 's/^>"\(.*\)" echo %ERRORLEVEL%$/\1/')
    out=$(fake_expand "$out")
    program=${line#\"}
    program=${program%%\"*}
    args=${line#*\"}
    args=${args#*\"}
    args=${args# }
    target=$program
    case $program in
    *[\\/]msiexec.exe)
        target=${args#/i \"}
        target=${target%%\"*}
        ;;
    esac
    target=$(fake_unix "$target")
    printf '%s\t%s\t%s\n' "$program" "$dir" "$args" >>"${FAKE_CMD_LOG:-$WINEPREFIX/fake-cmd.log}"
    if [ -n "$target" ] && [ -f "$target" ]; then
        code=$(cat "$target.exit" 2>/dev/null || echo 0)
    else
        code=9009
    fi
    if [ -n "$out" ] && [ ! -e "$target.nocode" ]; then
        printf '%s\r\n' "$code" >"$(fake_unix "$out")"
    fi
    return $((code & 255))
}
"#
    };
}

/// The `cmd /c FILE.bat` stand-in on its own, for a test's own fake runner.
pub const FAKE_CMD: &str = fake_cmd!();

/// A program named exit-N exits with N right away, a program named stubborn
/// ignores requests to close, a program named slow takes a moment before
/// wineserver can see it, noisy writes megabytes in bursts, noisy-exit leaves
/// a background writer behind, handoff exits and leaves something running for
/// a couple of seconds the way an installer hands off to its child,
/// break-wineserver deletes the wineserver beside it, a program whose name ends
/// in install-game.exe installs a game into the prefix, and any other program
/// runs until `wineboot --end-session` or `wineserver -k` ends it.
pub const FAKE_WINE: &str = concat!(
    "#!/bin/sh\n",
    fake_cmd!(),
    r#"pids="$WINEPREFIX/fake-pids"
# wineo lowers the core limit right after spawning, so wait for that to land.
record_core_when_one() {
    i=0
    while [ "$i" -lt 100 ] && ! grep -Eq '^Max core file size +1 ' /proc/self/limits; do
        sleep 0.1
        i=$((i + 1))
    done
    grep 'Max core file size' /proc/self/limits >"$WINEPREFIX/core-limit"
}
burst() {
    line=$(printf 'x%.0s' $(seq 1023))
    i=0
    while [ "$i" -lt 2048 ]; do
        echo "$line"
        i=$((i + 1))
    done
}
case "$1" in
cmd)
    case "$3" in *.bat)
        fake_cmd "$3"
        exit
        ;;
    esac
    ;;
--version)
    echo wine-fake
    exit 0
    ;;
wineboot)
    if [ "$2" = --init ]; then
        mkdir -p "$WINEPREFIX/drive_c"
        printf 'WINE REGISTRY Version 2\n\n#arch=win64\n' >"$WINEPREFIX/system.reg"
        if [ -n "${WINEO_TEST_RECORD_CORE-}" ]; then
            record_core_when_one
        fi
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
quiet-corelimit)
    record_core_when_one
    exit 0
    ;;
corelimit)
    # Records its core limit for a test, then runs like any other program.
    sleep 0.5
    grep 'Max core file size' /proc/self/limits >"$WINEPREFIX/core-limit"
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
*fail-game.exe)
    # Installs the game, then fails anyway.
    mkdir -p "$WINEPREFIX/drive_c/Games/Game"
    : >"$WINEPREFIX/drive_c/Games/Game/Game.exe"
    exit 3
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
"#
);

/// Treats the fake wine program as the only process in the prefix.
pub const FAKE_WINESERVER: &str = r#"#!/bin/sh
pids="$WINEPREFIX/fake-pids"
case "$1" in
-w)
    # Gives up after a minute, so a recorded pid that never goes away can't hold
    # it forever.
    i=0
    while [ "$i" -lt 600 ] && read -r program _ 2>/dev/null <"$pids" && kill -0 "$program" 2>/dev/null; do
        sleep 0.1
        i=$((i + 1))
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
pub const FAKE_UMU: &str = concat!(
    "#!/bin/sh\n",
    fake_cmd!(),
    r#"echo "umu-run: $*"
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
cmd)
    case "$3" in *.bat)
        fake_cmd "$3"
        exit
        ;;
    esac
    ;;
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
    # Gives up after a minute, so a killed test can't leave it looping.
    i=0
    while [ "$i" -lt 600 ]; do
        sleep 0.1
        i=$((i + 1))
    done
    ;;
handoff)
    # Leaves something in umu's process group after umu itself has exited.
    (sleep 2; touch "$WINEPREFIX/handoff-done") &
    exit 0
    ;;
handoff-held)
    # Like handoff, but what is left runs until the test creates
    # handoff-release, or for a minute at most. Its pid stays in the pids
    # file until then, so the fixture can kill it.
    (
        i=0
        while [ ! -e "$WINEPREFIX/handoff-release" ] && [ "$i" -lt 600 ]; do
            sleep 0.1
            i=$((i + 1))
        done
        touch "$WINEPREFIX/handoff-done"
        rm -f "$pids"
    ) &
    echo "$!" >"$pids"
    exit 0
    ;;
stubborn-child)
    # umu closes when asked, but leaves something in its group that ignores
    # the request, for a minute at most. The pids file outlives umu, so the
    # fixture can still kill what it left.
    (
        trap '' TERM
        i=0
        while [ "$i" -lt 600 ]; do
            sleep 0.1
            i=$((i + 1))
        done
    ) &
    trap 'exit 0' TERM
    echo "$$ $!" >"$pids"
    wait
    ;;
esac
sleep 60 &
trap 'kill "$!" 2>/dev/null; rm -f "$pids"; exit 0' TERM
echo "$$ $!" >"$pids"
wait
rm -f "$pids"
"#
);
