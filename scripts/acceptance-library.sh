#!/bin/bash
# Writes the standard library every acceptance checkpoint starts from, so the
# desktop app is always judged against the same games. Nothing here touches
# the real HOME or XDG directories: everything lands in DIR, which is a
# WINEO_PORTABLE_DIR folder.
#
# Usage:
#   scripts/acceptance-library.sh DIR
#   env WINEO_PORTABLE_DIR=DIR wineo-gui
#
# DIR must not exist or must be an empty folder, and nothing in DIR is ever
# changed or deleted. An empty DIR is replaced by a new folder, so its own
# mode, owner, and ACLs don't carry over. The library is built in a private folder (mode 700) made
# next to DIR, then moved into place with one rename, which replaces an empty
# DIR or a missing one and fails on anything else, including a symlink or a
# folder something filled in the meantime. The up-front check only gives a
# friendly message; the rename is what keeps DIR safe. To start over, remove
# the folder yourself and run this again.
#
# Layout of a portable root (see crates/wineo-core/src/paths.rs):
#   config/environments/NAME.toml   the games
#   config/library.toml             favorites, hidden, added dates, [view]
#   stats/NAME.jsonl                playtime and last played
#   art/NAME/                       posters (grid.png and art.toml)
#   prefixes/NAME                   minimal fake Wine prefixes
#   games/NAME.sh                   native stubs
# Every path inside a config is relative to DIR, so the folder can move.
#
# Each game below has a `# source:` comment saying what it covers, and the
# summary table printed at the end is built from those comments, so the two
# can't disagree. Dates are fixed, so every run writes the same library; the
# "ago" labels drift as days pass (the library was designed on 2026-11-06).
#
# What this can't fake, because it lives outside DIR:
#   - Steam install state, name, size, Proton build, and platform. They are
#     read from the real Steam under the real HOME. The made-up app ids below
#     aren't installed there, so the Steam games show "not installed" with no
#     Native or Windows label. In a portable folder the app skips Steam
#     discovery too, so no "new installs" offer appears.
#   - Steam playtime, which also comes from Steam (shown as unknown).
#   - A GOG "Update available" badge. The app never checks GOG in a portable
#     folder, and a check needs a GOG login in the host keyring plus network.
#     The GOG folder state behind "Finish update" is host-only as well.
#   - Run state (exited, failed, stopped, lost). It lives in host XDG state.
#     Starting the long-running game below creates a small folder there.
#
# Needs bash, GNU date, base64, mktemp, realpath, and mv -T (coreutils).

set -euo pipefail

die() {
    echo "acceptance-library: $*" >&2
    exit 1
}

usage() {
    cat <<'EOF'
Usage: acceptance-library.sh DIR

Writes the standard acceptance-test library into DIR, a WINEO_PORTABLE_DIR
folder with about fifty games covering every source, tag, favorite, hidden,
playtime, title, and art case the desktop app shows. DIR must not exist or
must be an empty folder, and its parent must exist; nothing is deleted or
overwritten. Then run:

  env WINEO_PORTABLE_DIR=DIR wineo-gui

Options:
  -h, --help   show this help
EOF
}

case ${1:-} in
    -h | --help)
        usage
        exit 0
        ;;
esac
[[ $# -eq 1 && -n $1 ]] || {
    usage >&2
    exit 2
}

dir=$1
while [[ $dir == */ && $dir != / ]]; do dir=${dir%/}; done
base=$(basename -- "$dir")
case $base in
    / | . | ..) die "$1 has no folder name to create; give a new folder name" ;;
esac
parent=$(dirname -- "$dir")
[[ -d $parent ]] || die "the parent folder $parent doesn't exist (it is not created for you)"

# A friendly early refusal. It fails closed: a folder that can't be listed
# counts as not empty. The rename at the end is what actually protects DIR.
if [[ -L $dir ]]; then
    die "$dir is a symlink; give the name of a new or empty folder instead"
elif [[ -e $dir ]]; then
    [[ -d $dir ]] || die "$dir exists and is not a folder"
    listing=$(ls -A -- "$dir" 2>&1) || die "can't list $dir, so can't tell it is empty (nothing was changed)"
    [[ -z $listing ]] || die "$dir is not empty; use a new or empty folder (nothing is deleted)"
fi

# Everything is written into a private folder beside DIR, so nothing else can
# write into it and a symlink swapped into DIR can't redirect a write. Only
# this folder is ever removed, and only until the final rename succeeds.
root=$(mktemp -d -- "$parent/.acceptance-library.XXXXXX") || die "can't make a work folder in $parent"
moved=0
cleanup() {
    if [[ $moved -eq 0 && -n $root ]]; then
        rm -rf -- "$root"
    fi
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

config=$root/config
mkdir -p "$config/environments" "$root/games" "$root/stats" "$root/art" "$root/prefixes"

# 40x60 posters in three color bands, enough to tell cards apart. Embedded so
# no image tool is needed and the bytes never change.
posters=(
    iVBORw0KGgoAAAANSUhEUgAAACgAAAA8CAIAAACb22+3AAAARUlEQVR42u3NMQ0AIAwAsBlBAcEJBxdyZgxzyNixJv0bc2eJEIvFYrFYLG4U33wlxGKxWCwWizvFY50SYrFYLBaLxY3iD8e2vkjZDZQAAAAAAElFTkSuQmCC
    iVBORw0KGgoAAAANSUhEUgAAACgAAAA8CAIAAACb22+3AAAARklEQVR42u3NMQ0AIAwAsBnBAIjAAhciEIExzCFjx5r0b9wxU4RYLBaLxWJxofjtk0IsFovFYrG4UrxaTyEWi8VisVhcKP7QVE3IIwEwZQAAAABJRU5ErkJggg==
    iVBORw0KGgoAAAANSUhEUgAAACgAAAA8CAIAAACb22+3AAAARUlEQVR42u3NMQ0AIAwAsBlBAcEJBxdyZgxzyNixJv0bM3eJEIvFYrFYLG4U35clxGKxWCwWizvF46wSYrFYLBaLxY3iD5AUvkh6ylX/AAAAAElFTkSuQmCC
    iVBORw0KGgoAAAANSUhEUgAAACgAAAA8CAIAAACb22+3AAAASElEQVR42u3NMQ0AIAwAsBlBATYQwIUIROAGFZhDxo416d84u6cIsVgsFovF4kLxuyuFWCwWi8VicaV4jpZCLBaLxWKxuFD8AfJU3dfBXwo6AAAAAElFTkSuQmCC
    iVBORw0KGgoAAAANSUhEUgAAACgAAAA8CAIAAACb22+3AAAAR0lEQVR42u3NMQ0AIAwAsBlBAQpwwcWBGARgCXPI2LEm/Ru7nxQhFovFYrFYXCi+66UQi8VisVgsrhSPNlOIxWKxWCwWF4o/plU1F66xJoIAAAAASUVORK5CYII=
    iVBORw0KGgoAAAANSUhEUgAAACgAAAA8CAIAAACb22+3AAAAQ0lEQVR42u3NMQ0AMAgAMIxMwcxwTQ7GMDcZHDTp37hVI0IsFovFYrF4Ufy6R4jFYrFYLBZvik/mCLFYLBaLxeJF8Qf11IamV3JpQQAAAABJRU5ErkJggg==
)

bs=\\
games=0
library=''

# iso EPOCH: a UTC timestamp in the form the logs and library.toml use.
iso() { date -u -d "@$1" +%Y-%m-%dT%H:%M:%SZ; }

# daily FIRST_DAY COUNT SECONDS: COUNT sessions at 18:00 UTC on consecutive
# days, in the START@SECONDS form `add` takes, for big playtimes.
daily() {
    local i out=''
    for ((i = 0; i < $2; i++)); do
        out+="$(iso "$(date -u -d "$1 18:00 UTC + $i days" +%s)")@$3 "
    done
    printf '%s' "$out"
}

# add NAME [OPTION...]
# Writes one game. Options:
#   kind=wine|native|steam   (default wine)
#   title=TEXT               the card title (no quote or backslash)
#   tags="a b c"
#   gog_id=ID heroic_id=ID store=egs|gog|amazon|steam   source facts
#   app_id=ID                a Steam game's id
#   favorite hidden          library.toml flags
#   added=TIMESTAMP          library.toml date added; left out for none
#   sessions="START@SECONDS ..."  play sessions in the stats log
#   imported=SECONDS[@LAST_PLAYED]  a Heroic playtime import
#   art=0..5                 a poster from the embedded list
#   noprefix                 leave a Wine game's prefix unbuilt
#   run=sleep                a native stub that sleeps 600 s instead of exiting
add() {
    local name=$1
    shift
    local kind=wine title='' tags='' gog_id='' heroic_id='' store='' app_id='' added=''
    local sessions='' imported='' art='' prefix=1 favorite=0 hidden=0 run=''
    local arg tag first
    for arg in "$@"; do
        case $arg in
            kind=*) kind=${arg#kind=} ;;
            title=*) title=${arg#title=} ;;
            tags=*) tags=${arg#tags=} ;;
            gog_id=*) gog_id=${arg#gog_id=} ;;
            heroic_id=*) heroic_id=${arg#heroic_id=} ;;
            store=*) store=${arg#store=} ;;
            app_id=*) app_id=${arg#app_id=} ;;
            added=*) added=${arg#added=} ;;
            sessions=*) sessions=${arg#sessions=} ;;
            imported=*) imported=${arg#imported=} ;;
            art=*) art=${arg#art=} ;;
            run=*) run=${arg#run=} ;;
            noprefix) prefix=0 ;;
            favorite) favorite=1 ;;
            hidden) hidden=1 ;;
            *) die "bug: unknown option for $name: $arg" ;;
        esac
    done
    [[ $name =~ ^[A-Za-z0-9_-]+$ ]] || die "bug: bad name $name"
    if [[ -e $config/environments/$name.toml ]]; then die "bug: $name is listed twice"; fi
    [[ $title != *[\"\\]* ]] || die "bug: $name's title needs TOML escaping"
    games=$((games + 1))

    {
        echo 'schema_version = 1'
        case $kind in
            wine)
                echo "prefix = \"prefixes/$name\""
                echo 'runner = "wine"'
                echo 'arch = "win64"'
                ;;
            native) echo 'kind = "native"' ;;
            steam)
                echo 'kind = "proxied"'
                echo 'launcher = "steam"'
                echo "app_id = \"$app_id\""
                ;;
            *) die "bug: unknown kind $kind" ;;
        esac
        if [[ -n $title ]]; then
            echo "title = \"$title\""
        fi
        if [[ -n $tags ]]; then
            first=1
            printf 'tags = ['
            for tag in $tags; do
                if ((first)); then first=0; else printf ', '; fi
                printf '"%s"' "$tag"
            done
            echo ']'
        fi
        if [[ -n $heroic_id ]]; then
            echo "heroic_id = \"$heroic_id\""
        fi
        if [[ -n $gog_id ]]; then
            echo "gog_id = \"$gog_id\""
        fi
        if [[ -n $store ]]; then
            echo "store = \"$store\""
        fi
        case $kind in
            wine)
                echo
                echo '[targets.game]'
                # A TOML literal string, so the backslashes stay as they are.
                echo "exe = 'C:${bs}Games${bs}${name}${bs}game.exe'"
                echo 'default = true'
                ;;
            native)
                echo
                echo '[targets.game]'
                echo "exe = \"games/$name.sh\""
                echo 'default = true'
                ;;
        esac
    } >"$config/environments/$name.toml"

    if [[ $kind == wine && $prefix == 1 ]]; then
        local prefix_dir=$root/prefixes/$name
        mkdir -p "$prefix_dir/dosdevices" "$prefix_dir/drive_c/Games/$name"
        printf 'WINE REGISTRY Version 2\n;; All keys relative to \\\\Machine\n\n#arch=win64\n' \
            >"$prefix_dir/system.reg"
        printf 'WINE REGISTRY Version 2\n' >"$prefix_dir/user.reg"
        printf 'WINE REGISTRY Version 2\n' >"$prefix_dir/userdef.reg"
        ln -s ../drive_c "$prefix_dir/dosdevices/c:"
        : >"$prefix_dir/drive_c/Games/$name/game.exe"
    fi

    if [[ $kind == native ]]; then
        local script=$root/games/$name.sh
        if [[ $run == sleep ]]; then
            # Bounded, so a forgotten run ends by itself.
            printf '#!/bin/sh\nexec sleep 600\n' >"$script"
        else
            printf '#!/bin/sh\nexit 0\n' >"$script"
        fi
        chmod +x "$script"
    fi

    if [[ -n $art ]]; then
        mkdir -p "$root/art/$name"
        printf '%s' "${posters[art]}" | base64 -d >"$root/art/$name/grid.png"
        printf 'schema_version = 1\n\n[grid]\nfile = "grid.png"\nmanual = "poster.png"\n' \
            >"$root/art/$name/art.toml"
    fi

    if [[ -n $sessions || -n $imported ]]; then
        local log=$root/stats/$name.jsonl start seconds session
        : >"$log"
        if [[ -n $imported ]]; then
            if [[ $imported == *@* ]]; then
                printf '{"imported":{"seconds":%d,"last_played":"%s"}}\n' \
                    "${imported%@*}" "${imported#*@}" >>"$log"
            else
                printf '{"imported":{"seconds":%d}}\n' "$imported" >>"$log"
            fi
        fi
        for session in $sessions; do
            start=${session%@*}
            seconds=${session#*@}
            printf '{"target":"game","started_at":"%s","ended_at":"%s","seconds":%d,"end":"exited"}\n' \
                "$start" "$(iso $(($(date -u -d "$start" +%s) + seconds)))" "$seconds" >>"$log"
        done
    fi

    if ((favorite || hidden)) || [[ -n $added ]]; then
        library+=$'\n'"[environments.$name]"$'\n'
        if ((favorite)); then library+=$'favorite = true\n'; fi
        if ((hidden)); then library+=$'hidden = true\n'; fi
        if [[ -n $added ]]; then library+="added = $added"$'\n'; fi
    fi
}

# Steam games. The ids are far above any real app id, so the real Steam never
# has them: the cards show "not installed" and have no Native or Windows label.

# source: Steam; tags: none; favorite; not installed; has art; title set
add steam-orchard-keeper kind=steam app_id=3999990001 title="Orchard Keeper" favorite \
    added=2026-11-01T09:00:00Z art=0

# source: Steam; tags: one (game, so the button says Play); no title
add steam-lantern-run kind=steam app_id=3999990002 tags=game added=2026-10-20T18:30:00Z

# source: Steam; tags: several, mixed case; hidden; title differs from name
add steam-voidmarch kind=steam app_id=3999990003 tags="game Strategy co-op-campaign" \
    title="Voidmarch: Ascension" hidden added=2026-09-02T14:00:00Z

# source: Steam; tags: one, mixed case; favorite and hidden
add steam-tidal-signal kind=steam app_id=3999990004 tags=RPG favorite hidden \
    added=2025-11-11T08:20:00Z

# source: Steam; tags: one; no added date; no art
add steam-glass-orchard kind=steam app_id=3999990005 tags=indie

# GOG games. Source detection reads gog_id; in a portable folder the app shows
# no GOG folder state or update badge (see the header).

# source: GOG; tags: several; favorite; title differs from name (has an apostrophe); art; recent plays
add gog-iron-harbor gog_id=1207658001 tags="game rpg" title="Iron Harbor: Director's Cut" \
    favorite added=2026-08-14T12:00:00Z art=1 \
    sessions="2026-11-05T19:00:00Z@7200 2026-11-04T20:00:00Z@5400 2026-10-28T21:00:00Z@10800"

# source: GOG; tags: none; no title; short playtime a long time ago
add gog-paper-kingdom gog_id=1207658002 added=2025-12-24T10:15:00Z \
    sessions="2026-06-01T15:00:00Z@600"

# source: GOG; tags: one; hidden; played once
add gog-ember-signal gog_id=1207658003 tags=game hidden added=2026-03-03T17:45:00Z \
    sessions="2025-12-30T22:00:00Z@3600"

# source: GOG through Heroic (gog_id and heroic_id); tags: several; imported playtime of 150 h
add gog-citadel-via-heroic gog_id=1207658004 heroic_id=1207658004 tags="game imported-from-heroic" \
    added=2026-09-20T19:00:00Z imported=540000@2026-09-15T20:00:00Z

# source: GOG (store hint gog on a Heroic import, no gog_id); tags: one; imported playtime plus a session
add gog-hint-only heroic_id=1432567890 store=gog tags=game title="Hint Only" \
    added=2026-02-02T11:00:00Z imported=7200@2026-02-01T20:00:00Z \
    sessions="2026-02-10T20:00:00Z@1800"

# Epic games: a Heroic import whose store hint is egs.

# source: Epic; tags: several, mixed case; favorite; title differs from name; art
add epic-frozen-garden heroic_id=0a1b2c3d4e5f60718293a4b5c6d7e8f9 store=egs \
    tags="game Epic-Exclusive" title="Frozen Garden" favorite added=2026-05-05T10:00:00Z art=2 \
    sessions="2026-10-31T21:00:00Z@4500"

# source: Epic; tags: none; hidden
add epic-wild-voyage heroic_id=1a2b3c4d5e6f708192a3b4c5d6e7f809 store=egs hidden \
    added=2026-01-19T09:30:00Z

# source: Epic; tags: one; imported playtime plus two sessions
add epic-quiet-lagoon heroic_id=2b3c4d5e6f708192a3b4c5d6e7f80910 store=egs tags=game \
    added=2026-04-12T16:00:00Z imported=180000@2026-03-30T22:00:00Z \
    sessions="2026-10-02T20:00:00Z@3600 2026-10-03T20:00:00Z@2400"

# Amazon games: a Heroic import whose store hint is amazon.

# source: Amazon; tags: one; recent play; art
add amazon-velvet-engine heroic_id=amzn1.adg.product.c0ffee00-1111-2222-3333-444455556666 \
    store=amazon tags=game added=2026-07-07T13:00:00Z art=0 \
    sessions="2026-11-03T18:00:00Z@9000"

# source: Amazon; tags: none; not installed (prefix not built); no added date; no title
add amazon-lost-orchard heroic_id=amzn1.adg.product.c0ffee00-7777-8888-9999-aaaabbbbcccc \
    store=amazon noprefix

# Heroic imports with no store label.

# source: no store label (Heroic import with an unknown store hint); Windows; tags: one
add heroic-sideload heroic_id=sideload-0001 store=sideload tags=game added=2026-06-18T12:00:00Z

# source: no store label (Heroic import); Native; tags: none
add heroic-native-lantern kind=native heroic_id=sideload-0002 added=2026-06-19T12:00:00Z

# Native Linux games, each a stub script in games/ that exits at once.

# source: Native; tags: one; favorite; art; recent play
add native-rusty-circuit kind=native tags=game favorite added=2026-10-10T08:00:00Z art=3 \
    sessions="2026-11-05T08:30:00Z@2700 2026-11-02T08:30:00Z@1500"

# source: Native; tags: several, mixed case
add native-savage-tundra kind=native tags="linux-native Indie" added=2026-10-01T10:00:00Z \
    sessions="2026-09-12T19:00:00Z@5400"

# source: Native; tags: none; Unicode title (Cyrillic, CJK, accents); played long ago
add native-pale-archive kind=native title="Звёздный архив 影の塔 Café" \
    sessions="2025-02-14T12:00:00Z@900"

# source: Native; tags: none; favorite and hidden
add native-stellar-odyssey kind=native favorite hidden added=2026-08-30T20:00:00Z

# source: Native; tags: one; RUNNING GAME. Select it in the app and press Play
# (the stub sleeps for 10 minutes, then exits by itself; Stop ends it sooner).
# Starting it writes run state under ~/.local/state/wineo/.portable/ in the
# real HOME, nowhere else.
add long-running-test kind=native tags=game title="Running Test (sleeps 10 min)" run=sleep \
    added=2026-11-05T23:00:00Z

# Plain Wine games (Windows).

# source: Windows; tags: one; 400 h of play, last played in late October
add wine-marathon-player tags=game added=2025-06-10T12:00:00Z art=4 \
    sessions="$(daily 2026-09-20 40 36000)"

# source: Windows; tags: one; not installed (prefix not built)
add wine-broken-foundry tags=game added=2026-10-25T12:00:00Z noprefix

# source: Windows; tags: one that isn't game (work, so the button says Launch); title differs from name; never played; art
add wine-notepad-plus tags=work title="Notepad++" added=2026-10-26T07:00:00Z art=5

# source: Windows; tags: one; Unicode title (macron, CJK, tilde, accents); art
add wine-unicode-quest tags=game title="Ōkami no Tō 影の塔 Café Ñandú" added=2026-10-15T09:00:00Z art=1 \
    sessions="2026-10-29T20:00:00Z@3000"

# source: Windows; tags: none; long title (exactly 120 characters, the longest valid)
add wine-long-title added=2026-10-12T09:00:00Z \
    title="The Extraordinarily Long and Winding Chronicles of the Wandering Cartographer of the Forgotten Seven Seas Part Two Redux"

# source: Windows; tags: none; DELIBERATE validate warning title_invalid (135 characters), still loads
add wine-invalid-title added=2026-10-13T09:00:00Z \
    title="An Even Longer Title That Breaks the One Hundred and Twenty Character Limit So That Validate Warns While the Card Still Shows It Anyway"

# source: Windows; sorts first by name (aaa) but last by title (Zebra Safari)
add aaa-zebra-safari tags=game title="Zebra Safari" added=2026-09-09T09:00:00Z

# source: Windows; sorts last by name (zzz) but first by title (Aardvark Adventure)
add zzz-aardvark-adventure tags=game title="Aardvark Adventure" added=2026-09-10T09:00:00Z

# source: Windows; tags: none; lowercase title (case-insensitive title sort)
add wine-lowercase-title title="ancient lantern" added=2026-09-11T09:00:00Z

# source: Windows; tags: none; accented first letter in the title (sorts after Z by bytes, not by letter)
add wine-accent-title title="Éclair Tactics" added=2026-09-12T09:00:00Z

# source: Windows; tags: none; title starts with a digit
add wine-digit-title title="7 Days of Dust" added=2026-09-13T09:00:00Z

# source: Windows; tags: a very long tag (80 characters)
add wine-long-tag tags="game a-really-long-tag-name-that-will-wrap-or-be-cut-in-chips-too-and-the-filter-list" \
    added=2026-09-14T09:00:00Z

# source: Windows; tags: mixed case only (Game is not game, so the button says Launch)
add wine-mixed-case-tags tags="Game Co-Op RPG" added=2026-09-15T09:00:00Z

# source: Windows; tags: eight, more than a card can show
add wine-many-tags tags="game rpg co-op indie retro sim strategy action" added=2026-09-16T09:00:00Z

# source: Windows; tags: none; favorite and hidden
add wine-favorite-hidden favorite hidden added=2026-09-17T09:00:00Z

# source: Windows; tags: none; hidden only
add wine-hidden-only hidden added=2026-09-18T09:00:00Z

# source: Windows; tags: none; favorite only
add wine-favorite-only favorite added=2026-09-19T09:00:00Z

# source: Windows; tags: none; no added date; never played
add wine-never-played-no-added

# source: Windows; tags: none; no added date; played
add wine-played-no-added sessions="2026-08-08T20:00:00Z@7200"

# source: Windows; tags: none; the oldest added date (2024); never played
add wine-added-oldest added=2024-01-15T08:00:00Z

# source: Windows; tags: none; the newest added date; never played
add wine-added-newest added=2026-11-05T23:30:00Z

# source: Windows; tags: none; a 45 second session
add wine-short-play added=2026-10-30T09:00:00Z sessions="2026-11-01T10:00:00Z@45"

# source: Windows; tags: one; the newest last-played value (2026-11-06 01:00 UTC, so the local day depends on the time zone)
add wine-played-today tags=game added=2026-10-05T09:00:00Z sessions="2026-11-06T01:00:00Z@1800"

# source: Windows; tags: one; last played a year ago
add wine-played-year-ago tags=game added=2025-10-01T09:00:00Z sessions="2025-11-07T20:00:00Z@14400"

# source: Windows; tags: one; last played about a month ago, 12 sessions of 25 minutes
add wine-played-month-ago tags=game added=2026-08-01T09:00:00Z \
    sessions="$(daily 2026-09-25 12 1500)"

# source: Windows; tags: none; last played 10 days ago
add wine-played-ten-days-ago added=2026-07-01T09:00:00Z sessions="2026-10-27T20:00:00Z@4000"

# Summary: the table comes from the `# source:` comment above each add call.
summary=$(awk '
    /^# source:/ { note = substr($0, 11); next }
    /^add / {
        if (note == "") { print "missing # source: comment for " $2 > "/dev/stderr"; exit 1 }
        printf "%-28s %s\n", $2, note
        note = ""
    }
' "${BASH_SOURCE[0]}") || die "a game has no # source: comment"
shown=$(printf '%s\n' "$summary" | wc -l)
[[ $shown -eq $games ]] || die "bug: $games games but $shown summary rows"

{
    printf 'schema_version = 1\n\n[view]\n'
    printf 'sort = "name"\ndescending = false\nfavorites_first = false\n'
    printf 'zoom = 100\nposter_only = false\nrandom = "launchable"\n'
    printf '%s' "$library"
} >"$config/library.toml"

# Give the folder the mode a plain mkdir would, since mktemp made it 700.
chmod "$(printf '%03o' $((0777 & ~$(umask))))" "$root"

# One rename(2): it replaces an empty folder or a missing name, and refuses a
# non-empty folder, a symlink, or a file, whatever happened since the check.
if ! failure=$(mv -T -- "$root" "$dir" 2>&1); then
    die "can't move the library into $dir; it may have changed since the check or be a mount point, so nothing was written there ($failure)"
fi
moved=1
final=$(realpath -- "$dir")

echo "Wrote $games games to $final"
echo
echo "Name                         Covers"
echo "$summary"
echo
printf 'Open it with: env WINEO_PORTABLE_DIR=%q wineo-gui\n' "$final"
