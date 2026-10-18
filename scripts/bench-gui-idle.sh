#!/bin/bash
# Idle CPU and scroll smoothness of wineo-gui on a generated library, in a
# private Xvfb with software rendering, the way plan 0051 measured it. It
# never touches the real display, HOME, XDG directories, or keyring, and it
# only kills the processes it started. The whole script runs with the
# library's HOME and XDG variables, no session bus, and no DISPLAY,
# WAYLAND_DISPLAY, or XAUTHORITY of its own.
#
# Usage: scripts/bench-gui-idle.sh ROOT [RUNS]
#   ROOT   a library from scripts/generate-bench-library.sh (becomes HOME)
#   RUNS   fresh app launches to measure (default 5), so the spread shows
#
# Environment:
#   WINEO_GUI  the binary (default target/release/wineo-gui, a `release`
#              build, as the plan measures)
#   WARMUP     seconds after launch before focusing (default 15): art decodes
#              and the first scans settle
#   SETTLE     seconds after focusing before sampling (default 5)
#   SAMPLE     seconds of idle sampled (default 20)
#   SCROLL     0 skips the scroll measurement
#   ICED_BACKEND  the renderer (default tiny-skia, iced's CPU renderer).
#              Without it iced's wgpu may drive the real GPU through Vulkan
#              even under Xvfb, which is not software rendering.
#
# Per run it prints which renderer's libraries the app loaded, idle CPU (all threads, share of one core), main-thread
# wakeups per second, RSS and threads, then scroll: 180 wheel clicks sent
# 16 ms apart while ffmpeg grabs Xvfb at 60 fps. The frames that differ from
# the one before are the frames the app actually presented, so "frames/s"
# against the ~60 clicks/s sent, and the longest gap between changed frames,
# are the smoothness numbers. The grab is sampled, so gaps are good to about
# 17 ms, and software rendering shows relative costs, not what a GPU feels.
#
# "Focused" matters: the 2 s rescan only runs while the window has focus,
# and on X11 without a window manager a click counts as focus, so the run
# clicks an empty spot in the header before sampling.
#
# A run fails loudly, instead of reporting zero activity, if the display is
# not the script's own, ffmpeg is missing or captures nothing, or the scroll
# changed no frames.
#
# Needs Xvfb, xdpyinfo, xdotool, ffmpeg (for scroll), and GNU awk (asort).

set -euo pipefail

repo=$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)
[[ $# -ge 1 && $# -le 2 ]] || {
    echo "usage: $0 ROOT [RUNS]" >&2
    exit 1
}
root=$(realpath -- "$1")
runs=${2:-5}
gui=${WINEO_GUI:-$repo/target/release/wineo-gui}
warmup=${WARMUP:-15}
settle=${SETTLE:-5}
sample=${SAMPLE:-20}
scroll=${SCROLL:-1}
[[ -f $root/.bench-library ]] || {
    echo "$root was not made by generate-bench-library.sh" >&2
    exit 1
}
[[ -x $gui ]] || {
    echo "no binary at $gui (cargo build --release -p wineo-gui)" >&2
    exit 1
}

need=(Xvfb xdpyinfo xdotool)
[[ $scroll != 0 ]] && need+=(ffmpeg)
for tool in "${need[@]}"; do
    command -v "$tool" >/dev/null || {
        echo "$tool is needed but not on PATH" >&2
        exit 1
    }
done

# Everything below, including xdotool, xdpyinfo, and ffmpeg, sees only the
# library's home. The bus address points nowhere, so the keyring is out of
# reach, and the real display and its authority are gone.
unset DISPLAY WAYLAND_DISPLAY XAUTHORITY
# shellcheck disable=SC1091
. "$root/env.sh"
export DBUS_SESSION_BUS_ADDRESS="unix:path=$root/no-bus"
export ICED_BACKEND=${ICED_BACKEND:-tiny-skia}

work=$(mktemp -d)
xvfb_pid=
gui_pid=
helper_pids=()
napper=

# TERM, then KILL after a couple of seconds, for each process this script
# started, so cleanup is bounded even if one ignores TERM.
reap() {
    local pid
    for pid in "$@"; do
        [[ -n $pid ]] && kill "$pid" 2>/dev/null || true
    done
    for _ in $(seq 20); do
        local alive=0
        for pid in "$@"; do
            [[ -n $pid ]] && kill -0 "$pid" 2>/dev/null && alive=1
        done
        ((alive)) || break
        sleep 0.1
    done
    for pid in "$@"; do
        [[ -n $pid ]] && kill -9 "$pid" 2>/dev/null || true
    done
}
cleanup() {
    trap - EXIT INT TERM
    reap "${helper_pids[@]}" "$napper" "$gui_pid" "$xvfb_pid"
    rm -rf -- "$work"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

clk=$(getconf CLK_TCK)

# A sleep the INT and TERM traps can interrupt: bash runs a trap only after
# a foreground command ends, but `wait` returns as soon as a signal lands.
nap() {
    sleep "$1" &
    napper=$!
    wait "$napper" || true
    napper=
}

# CPU ticks (user + system, every thread) and the main thread's context
# switches, read together so one window covers both.
snapshot() {
    local stat
    stat=$(<"/proc/$gui_pid/stat")
    # Fields after the command name, which may hold spaces.
    stat=${stat##*) }
    # shellcheck disable=SC2086
    set -- $stat
    local cpu=$((${12} + ${13}))
    local switches
    switches=$(awk '/ctxt_switches/ { n += $2 } END { print n }' "/proc/$gui_pid/task/$gui_pid/status")
    echo "$cpu $switches"
}

# A display number nothing uses. Xvfb's own -displayfd search is not enough:
# a compositor's Xwayland may hold :0 without Xvfb noticing, and
# Xvfb then replaces its socket, cutting the real session off from X11.
# Numbers from 90 up are left alone by desktop sessions, and each is checked
# against its lock file, socket, and an open attempt.
free_display() {
    local n
    for n in $(seq 90 140); do
        [[ -e /tmp/.X$n-lock || -e /tmp/.X11-unix/X$n ]] && continue
        DISPLAY=:$n xdpyinfo >/dev/null 2>&1 && continue
        echo "$n"
        return 0
    done
    echo "no free X display from :90 to :140" >&2
    return 1
}

start() {
    local number
    number=$(free_display)
    Xvfb ":$number" -screen 0 1280x800x24 -nolisten tcp >/dev/null 2>&1 &
    xvfb_pid=$!
    # The display is ours only if our server is alive, holds the lock, and
    # answers. Xvfb names its lock file after the number and writes its pid.
    local ready=0 lock
    for _ in $(seq 100); do
        lock=$(cat "/tmp/.X$number-lock" 2>/dev/null || true)
        if kill -0 "$xvfb_pid" 2>/dev/null \
            && [[ ${lock//[[:space:]]/} == "$xvfb_pid" ]] \
            && DISPLAY=:$number xdpyinfo >/dev/null 2>&1; then
            ready=1
            break
        fi
        sleep 0.1
    done
    if ((!ready)); then
        echo "display :$number is not this script's Xvfb (pid $xvfb_pid), giving up" >&2
        exit 1
    fi
    display=:$number
    export DISPLAY=$display
    "$gui" >"$work/gui.log" 2>&1 &
    gui_pid=$!
}

stop() {
    reap "$gui_pid" "$xvfb_pid"
    gui_pid=
    xvfb_pid=
}

# Which renderer ran, from what the process mapped and opened: tiny-skia
# draws on the CPU and loads no GPU stack, while wgpu maps Vulkan or EGL and
# opens /dev/dri, even for a software Vulkan device.
renderer() {
    local libs dri
    libs=$(grep -oE '(libvulkan|libEGL|libGLX|libgallium|libLLVM|libamdgpu|libdrm_amdgpu|libnvidia)[^ /]*' "/proc/$gui_pid/maps" | sort -u | paste -sd, - || true)
    dri=$(find "/proc/$gui_pid/fd" -lname '/dev/dri/*' 2>/dev/null | wc -l)
    if [[ -z $libs && $dri == 0 ]]; then
        echo "cpu"
    else
        echo "gpu-stack(${libs:-none},dri_fds=$dri)"
    fi
}

# Prints "frames/s p90_gap_ms max_gap_ms" from ffmpeg's framemd5 lines.
frame_stats() {
    awk -F', *' '
        /^#/ { next }
        {
            # The first frame has nothing before it to differ from.
            if (++frame > 1 && $6 != last) { n++; t = $3 + 0; if (prev != "") { gaps[++g] = t - prev }; prev = t; if (first == "") first = t }
            last = $6
        }
        END {
            if (g < 2) { exit 1 }
            m = asort(gaps)
            span = prev - first
            printf "%.1f %.0f %.0f\n", g / (span / 60), gaps[int(m * 0.9) + 1] * 1000 / 60, gaps[m] * 1000 / 60
        }' "$1"
}

fail() {
    echo "bench-gui-idle: $*" >&2
    exit 1
}

results="$work/results"
: >"$results"
for ((run = 1; run <= runs; run++)); do
    start
    nap "$warmup"
    kill -0 "$gui_pid" 2>/dev/null || fail "the app exited early: $(tail -n 5 "$work/gui.log")"
    shown=$(renderer)
    if [[ $ICED_BACKEND == tiny-skia && $shown != cpu ]]; then
        fail "ICED_BACKEND=tiny-skia but the app loaded a GPU stack: $shown"
    fi
    # An empty spot in the header: focuses the window without selecting a card.
    xdotool mousemove 900 24 click 1
    nap "$settle"
    read -r cpu0 sw0 < <(snapshot)
    nap "$sample"
    read -r cpu1 sw1 < <(snapshot)
    rss=$(awk '/VmRSS/ { printf "%.0f", $2 / 1024 }' "/proc/$gui_pid/status")
    threads=$(awk '/Threads/ { print $2 }' "/proc/$gui_pid/status")
    idle=$(awk -v a="$cpu0" -v b="$cpu1" -v s="$sample" -v c="$clk" 'BEGIN { printf "%.2f", (b - a) / c / s * 100 }')
    wake=$(awk -v a="$sw0" -v b="$sw1" -v s="$sample" 'BEGIN { printf "%.1f", (b - a) / s }')
    line="$run renderer=$shown idle_cpu=${idle}% main_wakeups=${wake}/s rss=${rss}MiB threads=$threads"
    if [[ $scroll != 0 ]]; then
        xdotool mousemove 600 400
        read -r cpu0 _ < <(snapshot)
        rm -f "$work/sent"
        (
            sleep 1
            start=$(date +%s.%N)
            xdotool click --repeat 180 --delay 16 5 || exit 1
            awk -v a="$start" -v b="$(date +%s.%N)" 'BEGIN { printf "%.1f\n", 180 / (b - a) }' >"$work/sent"
        ) &
        scroller=$!
        helper_pids+=("$scroller")
        ffmpeg -nostdin -loglevel error -f x11grab -draw_mouse 0 -framerate 60 -video_size 1200x800 -t 6 \
            -i "$display" -vf scale=320:200 -f framemd5 - >"$work/frames" 2>"$work/ffmpeg.log" \
            || fail "ffmpeg failed to capture $display: $(<"$work/ffmpeg.log")"
        wait "$scroller" || fail "xdotool could not send the scroll"
        helper_pids=()
        read -r cpu1 _ < <(snapshot)
        frames=$(grep -vc '^#' "$work/frames" || true)
        ((frames >= 200)) || fail "ffmpeg captured only $frames frames in 6 s (about 360 expected)"
        scroll_cpu=$(awk -v a="$cpu0" -v b="$cpu1" -v c="$clk" 'BEGIN { printf "%.2f", (b - a) / c / 6 * 100 }')
        read -r fps p90 max < <(frame_stats "$work/frames") || fail "scrolling changed no frames on screen"
        line+=" scroll_cpu=${scroll_cpu}% clicks_sent=$(<"$work/sent")/s scroll_frames=${fps}/s gap_p90=${p90}ms gap_max=${max}ms"
    fi
    echo "$line"
    echo "$line" >>"$results"
    stop
done

# min / median / p90 / max of one key's numbers across runs.
summarize() {
    local key=$1
    { grep -o "$key=[0-9.]*" "$results" || true; } | cut -d= -f2 | sort -n | awk -v key="$key" '
        { v[NR] = $1 }
        END {
            if (NR == 0) exit
            printf "%-16s min %8s  median %8s  p90 %8s  max %8s\n", key, v[1], v[int((NR + 1) / 2)], v[int(NR * 0.9 + 0.999)], v[NR]
        }'
}
echo
echo "Across $runs runs:"
for key in idle_cpu main_wakeups rss threads scroll_cpu clicks_sent scroll_frames gap_p90 gap_max; do
    summarize "$key"
done
