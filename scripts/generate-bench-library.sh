#!/bin/bash
# Builds a throwaway wineo library for benchmarking the GUI's library read,
# or mutates one to look busy. Nothing here touches the real HOME or XDG
# directories: everything lives under ROOT.
#
# Usage:
#   scripts/generate-bench-library.sh ROOT [COUNT]      build COUNT environments (default 300)
#   scripts/generate-bench-library.sh --busy ROOT       append a session to every stats log
#
# ROOT is HOME. The XDG directories are its defaults (ROOT/.config,
# ROOT/.local/share, ROOT/.local/state, ROOT/.cache), and ROOT/env.sh exports
# all of them, so `. ROOT/env.sh` points any wineo command at the library:
#   . ROOT/env.sh && wineo list && wineo validate
#
# Each environment gets a config, a fake prefix (so `validate` has no
# prefix_missing warning), a 600x900 grid poster, and a stats log of 150 to
# 450 sessions in the format core reads (see `stats::Session`). The mix is
# about 70% Wine, 17% native, and the rest Wine with a gog_id (with the GOG
# folder state `validate` expects), a heroic_id (with an `imported` playtime
# line), or a store hint. The output is deterministic for a given COUNT, apart
# from the posters' pool of random images and the timestamps, which are
# relative to the time of generation.
#
# `--busy` is the "everything changed" case for the rescan: it appends one
# session to every stats log, which changes each card's playtime and session
# count. Plan 0054 adds a library.toml edit touching every entry to this case
# once that file exists. A rescan only looks different from the last one if
# something really changed, so call it between timed rescans, not once.
#
# Needs bash, awk (any POSIX awk), sha256sum, and ImageMagick (`magick`) for
# the posters. At the end it runs `validate --fix` with the wineo in WINEO_BIN
# (or on PATH) to write the desktop entries, then `validate`, which should
# print "No problems in COUNT environments". The runner is the system `wine`,
# so a machine without it gets runner_missing for every Wine environment.

set -euo pipefail

POSTER_POOL=40
SEED=54

die() {
    echo "generate-bench-library: $*" >&2
    exit 1
}

# Appends one session ending a few seconds ago to every stats log.
busy() {
    local root=$1 log
    [[ -f $root/.bench-library ]] || die "$root is not a generated bench library"
    for log in "$root"/.local/share/wineo/stats/*.jsonl; do
        awk -v now="$(date +%s)" -f /dev/stdin <<'AWK' >>"$log"
function iso(t,   days, secs, z, era, doe, yoe, y, doy, mp, d, m) {
    days = int(t / 86400); secs = t - days * 86400
    z = days + 719468; era = int(z / 146097); doe = z - era * 146097
    yoe = int((doe - int(doe / 1460) + int(doe / 36524) - int(doe / 146096)) / 365)
    y = yoe + era * 400; doy = doe - (365 * yoe + int(yoe / 4) - int(yoe / 100))
    mp = int((5 * doy + 2) / 153); d = doy - int((153 * mp + 2) / 5) + 1
    m = (mp < 10) ? mp + 3 : mp - 9; if (m <= 2) y++
    return sprintf("%04d-%02d-%02dT%02d:%02d:%02dZ", y, m, d, int(secs / 3600), int(secs % 3600 / 60), secs % 60)
}
# Generated sessions all end more than an hour ago, so this one is the newest.
BEGIN { s = now - 600; printf "{\"target\":\"game\",\"started_at\":\"%s\",\"ended_at\":\"%s\",\"seconds\":540,\"end\":\"exited\"}\n", iso(s), iso(s + 540) }
AWK
    done
}

if [[ ${1:-} == --busy ]]; then
    [[ $# -eq 2 ]] || die "usage: $0 --busy ROOT"
    busy "$(realpath -- "$2")"
    exit 0
fi

[[ $# -ge 1 && $# -le 2 ]] || die "usage: $0 ROOT [COUNT] | --busy ROOT"
count=${2:-300}
[[ $count =~ ^[1-9][0-9]*$ ]] || die "COUNT must be a positive number"
mkdir -p -- "$1"
root=$(realpath -- "$1")
command -v magick >/dev/null || die "ImageMagick (magick) is needed for the posters"

# Refuse to wipe anything that isn't a previous run's output.
if [[ -n $(ls -A -- "$root") && ! -f $root/.bench-library ]]; then
    die "$root is not empty and not a bench library"
fi
for sub in .config .local .cache games pool; do
    rm -rf -- "${root:?}/$sub"
done
touch "$root/.bench-library"

config=$root/.config/wineo
data=$root/.local/share/wineo
envs=$config/environments
mkdir -p "$envs" "$data/stats" "$data/art" "$data/prefixes" "$data/gog/folders" \
    "$root/.local/state" "$root/.cache" "$root/games" "$root/pool"

cat >"$root/env.sh" <<EOF
export HOME='$root'
export XDG_CONFIG_HOME='$root/.config'
export XDG_DATA_HOME='$root/.local/share'
export XDG_STATE_HOME='$root/.local/state'
export XDG_CACHE_HOME='$root/.cache'
unset WINEO_PORTABLE_DIR
EOF

# A small pool of distinct posters copied around: decoding cost depends on
# size, not on every file being unique, and magick is slow.
echo "Making $POSTER_POOL posters" >&2
for ((i = 0; i < POSTER_POOL; i++)); do
    magick -seed $((SEED + i)) -size 600x900 plasma:fractal -blur 0x1.5 \
        -quality 82 "$root/pool/$i.jpg"
done

adjectives=(iron silent crimson hollow ember frozen gilded lost wild neon
    broken ancient stellar rusty velvet shattered hidden emerald pale burning
    quiet savage distant paper copper)
nouns=(harbor kingdom signal garden empire orchard citadel voyage engine
    meridian frontier archive lantern horizon outpost sanctum circuit
    harvest tundra odyssey reactor bastion carnival lagoon foundry)
tag_pool=(rpg strategy co-op action puzzle indie retro sim)

# Exact fields of an environment config: see docs/environment.toml.example.
RANDOM=$SEED
bs=\\
now=$(date +%s)
echo "Writing $count environments" >&2
for ((i = 0; i < count; i++)); do
    adjective=${adjectives[RANDOM % ${#adjectives[@]}]}
    noun=${nouns[(RANDOM + i) % ${#nouns[@]}]}
    name=$(printf '%s-%s-%03d' "$adjective" "$noun" "$i")
    title="${adjective^} ${noun^} $((i % 7 + 1))"
    tag=${tag_pool[RANDOM % ${#tag_pool[@]}]}
    kind=wine
    ((i % 6 == 0)) && kind=native
    hint=
    if [[ $kind == wine ]]; then
        case $((i % 25)) in
            1) hint=gog ;;
            2) hint=heroic ;;
            3 | 4) hint=store ;;
        esac
    fi

    {
        echo 'schema_version = 1'
        [[ $kind == native ]] && echo 'kind = "native"'
        if [[ $kind == wine ]]; then
            echo "prefix = \"$data/prefixes/$name\""
            echo 'runner = "wine"'
            echo 'arch = "win64"'
        fi
        # Most are games, as in a real library, so Play and Launch both show.
        if ((i % 5 == 4)); then
            echo "tags = [\"$tag\"]"
        else
            echo "tags = [\"game\", \"$tag\"]"
        fi
        case $hint in
            gog)
                echo "gog_id = \"$((1207658000 + i))\""
                echo 'store = "gog"'
                ;;
            heroic)
                printf 'heroic_id = "%016x"\n' $((0xabc0000000000 + i * 7919))
                echo 'store = "epic"'
                ;;
            store) echo 'store = "steam"' ;;
        esac
        echo
        echo '[targets.game]'
        if [[ $kind == native ]]; then
            echo "exe = \"$root/games/$name/start.sh\""
        elif [[ $hint == gog ]]; then
            echo "exe = '$root/games/$name/game.exe'"
        else
            # A TOML literal string, so the backslashes stay as they are.
            echo "exe = 'C:${bs}Games${bs}${name}${bs}game.exe'"
        fi
        echo 'default = true'
        # Two thirds have a desktop entry title, so card titles differ from names.
        if ((i % 3 != 2)); then
            echo 'desktop = true'
            echo "name = \"$title\""
        fi
    } >"$envs/$name.toml"

    if [[ $kind == native ]]; then
        mkdir -p "$root/games/$name"
        printf '#!/bin/sh\nexit 0\n' >"$root/games/$name/start.sh"
        chmod +x "$root/games/$name/start.sh"
    else
        prefix=$data/prefixes/$name
        mkdir -p "$prefix/dosdevices" "$prefix/drive_c/Games/$name"
        printf 'WINE REGISTRY Version 2\n;; All keys relative to \\\\Machine\n\n#arch=win64\n' \
            >"$prefix/system.reg"
        printf 'WINE REGISTRY Version 2\n' >"$prefix/user.reg"
        printf 'WINE REGISTRY Version 2\n' >"$prefix/userdef.reg"
        ln -s ../drive_c "$prefix/dosdevices/c:"
        : >"$prefix/drive_c/Games/$name/game.exe"
    fi

    if [[ $hint == gog ]]; then
        folder=$root/games/$name
        mkdir -p "$folder"
        : >"$folder/game.exe"
        key=$(printf '%s' "$folder" | sha256sum | cut -d' ' -f1)
        cat >"$data/gog/folders/$key.toml" <<EOF
version = 1
owner = "$name"
folder = "$folder"

[committed]
gog_id = $((1207658000 + i))
platform = "windows"
language = "en-US"
track = "installer"
products = []
EOF
    fi

    mkdir -p "$data/art/$name"
    cp -- "$root/pool/$((i % POSTER_POOL)).jpg" "$data/art/$name/grid.jpg"

    # One awk process per log. A heroic game's log starts with the imported
    # playtime line, as an import writes it.
    heroic=0
    [[ $hint == heroic ]] && heroic=1
    awk -v seed=$((SEED * 1000 + i)) -v now="$now" -v heroic="$heroic" -f /dev/stdin <<'AWK' >"$data/stats/$name.jsonl"
function iso(t,   days, secs, z, era, doe, yoe, y, doy, mp, d, m) {
    days = int(t / 86400); secs = t - days * 86400
    z = days + 719468; era = int(z / 146097); doe = z - era * 146097
    yoe = int((doe - int(doe / 1460) + int(doe / 36524) - int(doe / 146096)) / 365)
    y = yoe + era * 400; doy = doe - (365 * yoe + int(yoe / 4) - int(yoe / 100))
    mp = int((5 * doy + 2) / 153); d = doy - int((153 * mp + 2) / 5) + 1
    m = (mp < 10) ? mp + 3 : mp - 9; if (m <= 2) y++
    return sprintf("%04d-%02d-%02dT%02d:%02d:%02dZ", y, m, d, int(secs / 3600), int(secs % 3600 / 60), secs % 60)
}
BEGIN {
    srand(seed * 104729)
    # Neighboring seeds start alike, so skip the first draws.
    for (j = 0; j < 8; j++) rand()
    n = 150 + int(rand() * 301)
    gap = 86400 * 1.2
    # Sessions average 1.6 hours; the 8% margin keeps the last one in the past.
    # Half the games were played this week or so, the rest up to a year ago,
    # so "last played" varies like a real library's.
    lag = (rand() < 0.5) ? rand() * 10 * 86400 : rand() * 365 * 86400
    t = now - int(n * (gap + 5760) * 1.08 + lag) - 7200
    if (heroic) printf "{\"imported\":{\"seconds\":%d,\"last_played\":\"%s\"}}\n", 3600 * (5 + int(rand() * 200)), iso(t - 86400 * 30)
    for (k = 0; k < n; k++) {
        t += int(rand() * 2 * gap)
        d = 600 + int(rand() * 10200)
        e = rand()
        end = (e < 0.90) ? "exited" : (e < 0.98) ? "stopped" : "failed"
        if (t + d > now - 3600) break
        printf "{\"target\":\"game\",\"started_at\":\"%s\",\"ended_at\":\"%s\",\"seconds\":%d,\"end\":\"%s\"}\n", iso(t), iso(t + d), d, end
        t += d
    }
}
AWK
done

rm -rf -- "${root:?}/pool"

# The desktop entries cards with a title ask for are wineo's to write, in a
# format that changes, so let wineo's own fix write them rather than copying
# its output here. Without it, `validate` warns that they are out of date.
wineo=${WINEO_BIN:-$(command -v wineo || true)}
if [[ -n $wineo ]]; then
    (
        # shellcheck disable=SC1091
        . "$root/env.sh"
        "$wineo" validate --fix >/dev/null || true
        "$wineo" validate >&2 || true
    )
else
    echo "No wineo found (set WINEO_BIN): run \`wineo validate --fix\` with env.sh" \
        "sourced to write the desktop entries, or validate will warn about them." >&2
fi
echo "Done: $count environments under $root" >&2
echo "Use it with: . $root/env.sh" >&2
