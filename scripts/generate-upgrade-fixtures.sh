#!/bin/bash
# Regenerates crates/wineo-core/tests/fixtures/<tag>/ with the files a real old
# wineo release writes. For each tag it builds that tag's CLI from a git
# worktree, runs the release's own commands against fake runners in a fresh
# root, and copies the root's files into the fixtures directory.
#
# Usage: scripts/generate-upgrade-fixtures.sh [--keep-roots] [TAG...]
#   TAG            one of v0.2.0 v0.7.0 v0.14.0 v0.24.0 v0.28.0 v0.31.0 v0.34.0;
#                  none means all of them
#   --keep-roots   leave the scratch roots under $WORK/roots for inspection
#
# Environment:
#   WINEO_FIXTURE_WORK  where tag worktrees, the shared target directory, and
#                       scratch roots live. Defaults to fixture-gen next to the
#                       repo's other worktrees. It must be on disk (snapshots
#                       need reflink, and /tmp is RAM), and it is kept between
#                       runs so builds are reused. Delete it when done.
#
# Needs network only to fetch crates the first time a tag builds. Nothing here
# touches the real HOME, XDG directories, keyring, or Steam/Heroic/GOG data:
# each run gets its own HOME and XDG_* and a PATH holding only fakes.

# shellcheck disable=SC2016,SC2088
# Single quotes hold literal $ and ~ on purpose: wineo must see a literal ~ in
# a path, and fake scripts are written out unexpanded.

set -euo pipefail

TAGS_ALL=(v0.2.0 v0.7.0 v0.14.0 v0.24.0 v0.28.0 v0.31.0 v0.34.0)

repo=$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)
common=$(git -C "$repo" rev-parse --path-format=absolute --git-common-dir)
WORK=${WINEO_FIXTURE_WORK:-$(dirname "$common")/../.worktrees/wineo/fixture-gen}
mkdir -p "$WORK"
WORK=$(cd "$WORK" && pwd -P)
fixtures=$repo/crates/wineo-core/tests/fixtures
target=$WORK/target
keep_roots=0

# ge TAG MIN: true when TAG is MIN or newer.
ge() {
    [[ $(printf '%s\n%s\n' "${1#v}" "${2#v}" | sort -V | head -n1) == "${2#v}" ]]
}

# ---------------------------------------------------------------------------
# Fakes. The PATH wineo sees holds only these, so no real wine, umu,
# winetricks, gamescope, mangohud, or gamemode can ever be reached.
# ---------------------------------------------------------------------------

make_stub() {
    local stub=$WORK/stub tool path
    rm -rf "$stub"
    mkdir -p "$stub/bin" "$stub/lib/wine/x86_64-unix"
    # Makes wayland = true acceptable: wineo looks for the driver two
    # directories up from the wine binary.
    : >"$stub/lib/wine/x86_64-unix/winewayland.so"
    for tool in sh sleep rm kill printf seq touch head tr mkdir ln cat sed env \
        dirname basename cp mv uname id date wc sort grep chmod readlink \
        realpath ls; do
        path=$(type -P "$tool") || continue
        ln -s "$(readlink -f "$path")" "$stub/bin/$tool"
    done
    cat >"$stub/bin/wine" <<'EOF'
#!/bin/sh
case "$1" in
--version) echo wine-10.0; exit 0 ;;
wineboot)
    if [ "$2" = --init ]; then
        # Git keeps no empty directories, so drive_c gets one small file.
        mkdir -p "$WINEPREFIX/drive_c/windows" "$WINEPREFIX/dosdevices"
        : >"$WINEPREFIX/drive_c/windows/win.ini"
        ln -sfn ../drive_c "$WINEPREFIX/dosdevices/c:"
        printf 'WINE REGISTRY Version 2\n\n#arch=%s\n' "${WINEARCH:-win64}" >"$WINEPREFIX/system.reg"
    fi
    exit 0 ;;
reg | winemenubuilder.exe) exit 0 ;;
esac
sleep 2
EOF
    printf '#!/bin/sh\nexit 0\n' >"$stub/bin/wineserver"
    # Like the real umu, it makes pfx/ for an empty program and then fails.
    cat >"$stub/bin/umu-run" <<'EOF'
#!/bin/sh
if [ -z "$1" ]; then
    mkdir -p "$WINEPREFIX/pfx/drive_c/windows" "$WINEPREFIX/pfx/dosdevices"
    : >"$WINEPREFIX/pfx/drive_c/windows/win.ini"
    ln -sfn ../drive_c "$WINEPREFIX/pfx/dosdevices/c:"
    printf 'WINE REGISTRY Version 2\n\n#arch=win64\n' >"$WINEPREFIX/pfx/system.reg"
    exit 1
fi
case "$1" in
winetricks)
    shift
    [ "$1" = -q ] && shift
    for verb in "$@"; do printf '%s\n' "$verb" >>"$WINEPREFIX/pfx/winetricks.log"; done
    exit 0 ;;
reg | winecfg) exit 0 ;;
esac
sleep 2
EOF
    cat >"$stub/bin/winetricks" <<'EOF'
#!/bin/sh
[ "$1" = -q ] && shift
for verb in "$@"; do printf '%s\n' "$verb" >>"$WINEPREFIX/winetricks.log"; done
exit 0
EOF
    # Wrappers that run what they wrap, skipping their own options.
    printf '#!/bin/sh\nwhile [ "${1#-}" != "$1" ]; do shift; done\nexec "$@"\n' >"$stub/bin/mangohud"
    cp "$stub/bin/mangohud" "$stub/bin/gamemoderun"
    printf '#!/bin/sh\nwhile [ $# -gt 0 ] && [ "$1" != -- ]; do shift; done\n[ "$1" = -- ] && shift\nexec "$@"\n' >"$stub/bin/gamescope"
    chmod +x "$stub/bin/wine" "$stub/bin/wineserver" "$stub/bin/umu-run" \
        "$stub/bin/winetricks" "$stub/bin/mangohud" "$stub/bin/gamemoderun" \
        "$stub/bin/gamescope"
}

# ---------------------------------------------------------------------------
# Building
# ---------------------------------------------------------------------------

build_tag() {
    local tag=$1 tree=$WORK/tags/$1
    if [[ ! -d $tree ]]; then
        mkdir -p "$WORK/tags"
        git -C "$repo" worktree add --detach "$tree" "$tag" >/dev/null
    fi
    (
        cd "$tree"
        cargo fetch --locked >/dev/null 2>&1 || true
        CARGO_TARGET_DIR=$target cargo build --locked --release -p wineo-cli >&2
    )
    cp "$target/release/wineo" "$WORK/wineo-$tag"
}

# ---------------------------------------------------------------------------
# Running a release's commands
# ---------------------------------------------------------------------------

# Set by begin_set.
tag="" bin="" set_dir="" root="" home="" state="" cmds=""

begin_set() {
    tag=$1
    bin=$WORK/wineo-$tag
    set_dir=$WORK/roots/$tag
    root=$set_dir/root
    home=$set_dir/home
    state=$set_dir/state
    rm -rf "$set_dir"
    mkdir -p "$root" "$home" "$state" "$set_dir/cache"
    : >"$set_dir/no-bus"
    cmds=$set_dir/commands.txt
    : >"$cmds"
}

# Runs a command with an environment built from nothing: the release gets only
# the isolated HOME and XDG directories, the fake PATH, and a session bus that
# doesn't exist, so it can't reach the real keyring.
isolated_here() {
    cd "$set_dir" && exec env -i \
        HOME="$home" \
        XDG_CONFIG_HOME="$home/.config" \
        XDG_DATA_HOME="$home/.local/share" \
        XDG_STATE_HOME="$state" \
        XDG_CACHE_HOME="$set_dir/cache" \
        PATH="$WORK/stub/bin" \
        DBUS_SESSION_BUS_ADDRESS="unix:path=$set_dir/no-bus" \
        LANG=C.UTF-8 TERM=dumb \
        "$@"
}
# A subshell, so the cd and exec stay out of the script itself.
isolated() { (isolated_here "$@"); }

# note LINE: records a line in the set's README command list.
note() {
    printf '%s\n' "$1" >>"$cmds"
}

# Replaces the machine's paths in a logged command with placeholders.
redact() {
    sed -e "s|$root|\$ROOT|g" -e "s|$home|\$HOME|g" -e "s|$set_dir|\$SCRATCH|g"
}

# logged ARGS...: the arguments as a shell would need them, quoted only where
# a character calls for it.
logged() {
    local arg out=""
    for arg in "$@"; do
        if [[ $arg =~ ^[A-Za-z0-9_./=:@%+,-][A-Za-z0-9_./=:@%+,~-]*$ ]]; then
            out+=" $arg"
        else
            out+=" '${arg//\'/\'\\\'\'}'"
        fi
    done
    printf '%s' "$out" | redact
}

# w ARGS...: the release's wineo against the portable root.
w() {
    local line
    line=$(logged "$@")
    note "wineo --portable \$ROOT$line"
    isolated "$bin" --portable "$root" "$@" >"$set_dir/last.log" 2>&1 || {
        cat "$set_dir/last.log" >&2
        echo "failed ($tag): wineo $*" >&2
        return 1
    }
}

# x ARGS...: the release's wineo against the XDG home, for what a portable
# folder can't hold.
x() {
    local line
    line=$(logged "$@")
    note "wineo$line   (XDG, HOME=\$HOME)"
    isolated "$bin" "$@" >"$set_dir/last.log" 2>&1 || {
        cat "$set_dir/last.log" >&2
        echo "failed ($tag): wineo $*" >&2
        return 1
    }
}

# Notes a file or directory made outside wineo, such as a fake game exe.
made() {
    local text=$1
    text=${text//root\//\$ROOT/}
    text=${text//home\//\$HOME/}
    note "(not wineo) $text"
}

# Writes a file the way a user or an installer would, and says so.
hand_file() {
    local path=$1 text=$2
    mkdir -p "$(dirname "$path")"
    printf '%s' "$text" >"$path"
    made "wrote ${path#"$set_dir"/}"
}

# fake_exe PREFIX_DIR WINDOWS_DIR NAME: an empty program inside a prefix.
fake_exe() {
    mkdir -p "$1/drive_c/$2"
    : >"$1/drive_c/$2/$3"
    made "touched ${1#"$set_dir"/}/drive_c/$2/$3"
}

# Waits until nothing is running in the portable root.
wait_idle() {
    for _ in $(seq 40); do
        if isolated "$bin" --portable "$root" status 2>&1 | grep -q 'Nothing is running'; then
            return 0
        fi
        sleep 0.5
    done
    echo "$tag: a run never finished" >&2
    return 1
}

PNG='\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89\0\0\0\rIDATx\x9cc\x10\x14\xd7\xfa\x0f\0\x01\xe1\x01Re\x03\x38\x0e\0\0\0\0IEND\xaeB`\x82'

# ---------------------------------------------------------------------------
# One set
# ---------------------------------------------------------------------------

generate() {
    begin_set "$1"
    local left_out=()

    # ----- global config
    local global=(stop_timeout=20 log_limit_mb=25 gamemode=true
        defaults.arch=win32 env.DXVK_HUD=fps)
    if ge "$tag" v0.14.0; then
        global+=(wayland=true mangohud=false)
    fi
    if ge "$tag" v0.24.0; then
        # art.fetch is in the file format from v0.7.0, but only these releases
        # can write it with `set`.
        global+=(external_runners=true 'art.fetch=icon,grid')
    fi
    if ge "$tag" v0.28.0; then
        global+=(steam=native)
    fi
    if ge "$tag" v0.31.0; then
        global+=(gog.language=de-DE 'defaults.games_root=~/Spiele')
    fi
    w set --global "${global[@]}"

    # ----- a user template. Releases have no command that writes one, so this
    # is the one hand-written file; the release's own `templates` and `new -t`
    # prove it reads it.
    local tpl='schema_version = 1
runner = "wine"
arch = "win64"
gamemode = true
'
    if ge "$tag" v0.14.0; then
        tpl+='tags = ["game", "co-op"]
winetricks = ["corefonts"]
mangohud = true
'
    fi
    if ge "$tag" v0.28.0; then
        tpl+='
[hooks]
before = "true"
after = "true"
'
    fi
    tpl+='
[env]
WINEDEBUG = "-all"
'
    hand_file "$root/config/templates/mygame.toml" "$tpl"
    # A second template with the keys an environment can't take offline.
    hand_file "$root/config/templates/full.toml" 'schema_version = 1
gameid = "umu-12345"
store = "steam"
dxvk = "latest"
vkd3d = "latest"
'
    w templates

    # ----- environments
    w new -n plain
    w new -n templated -t mygame --arch win64
    if ge "$tag" v0.14.0; then
        w new -n lazy --arch win64
    fi

    fake_exe "$root/prefixes/plain" 'Games/Plain' Plain.exe
    w set plain 'targets.main.exe=C:\Games\Plain\Plain.exe' targets.main.default=true \
        'targets.main.args=["-windowed"]' targets.main.gamemode=false \
        'targets.main.cwd=C:\Games\Plain' 'env.PLAIN=1'
    fake_exe "$root/prefixes/templated" 'Games/Templated' Templated.exe
    fake_exe "$root/prefixes/templated" 'Games/Templated' Editor.exe
    w set templated 'targets.game.exe=C:\Games\Templated\Templated.exe' targets.game.default=true \
        'targets.editor.exe=C:\Games\Templated\Editor.exe'

    if ge "$tag" v0.14.0; then
        fake_exe "$root/prefixes/lazy" 'Games/Lazy' Lazy.exe
        w set lazy 'targets.main.exe=C:\Games\Lazy\Lazy.exe' targets.main.default=true \
            'tags=["game"]' 'winetricks=["vcrun2022", "corefonts"]' \
            wayland=true mangohud=true gamescope.width=1280 gamescope.height=720 \
            gamescope.fps_limit=60 targets.main.mangohud=false
    fi
    if ge "$tag" v0.28.0; then
        w set lazy 'hooks.before=true' 'hooks.after=true' \
            targets.main.hooks=false
    fi

    if ge "$tag" v0.7.0; then
        mkdir -p "$root/games"
        hand_file "$root/games/tool.sh" '#!/bin/sh
sleep 1
'
        chmod +x "$root/games/tool.sh"
        # v0.7.0's --native takes the program itself; later ones take --exe.
        if ge "$tag" v0.14.0; then
            w new -n tool --native --exe "$root/games/tool.sh"
        else
            w new -n tool --native "$root/games/tool.sh"
        fi
        w set tool 'targets.tool.args=["--fast"]'
    fi

    # ----- art, from a local image
    printf '%b' "$PNG" >"$set_dir/grid.png"
    made "wrote a 1x1 PNG outside the root to use as art"
    w art set plain grid "$set_dir/grid.png"
    w art set plain.main icon "$set_dir/grid.png"

    # ----- snapshot
    w snapshot create plain first

    # ----- a short run, for playtime (needs a release with stats)
    if ge "$tag" v0.14.0; then
        w start lazy
        wait_idle
        w stats
        grep -q '"end":"exited"' "$root/stats/lazy.jsonl" || {
            echo "$tag: the playtime log has no clean exit" >&2
            return 1
        }
        if ge "$tag" v0.7.0; then
            w start tool
            wait_idle
        fi
    else
        left_out+=("Playtime log: this release has no \`stats\`.")
    fi

    # ----- trash
    if ge "$tag" v0.14.0; then
        w new -n doomed --arch win64
        w rm doomed
        # The second one takes its prefix and snapshots to the trash too.
        w new -n doomed-prefix --arch win64
        w snapshot create doomed-prefix kept
        w rm doomed-prefix --delete-prefix
    else
        left_out+=("Trash entry: this release's \`rm\` deletes outright, there is no trash.")
    fi

    # ----- what a portable folder can't hold goes in a plain XDG home
    xdg_part

    # ----- left out
    if ! ge "$tag" v0.7.0; then
        left_out+=("Native environment: arrives in v0.7.0.")
    fi
    if ! ge "$tag" v0.14.0; then
        left_out+=("Launch options, winetricks, and tags: arrive in v0.14.0.")
    fi
    if ! ge "$tag" v0.24.0; then
        left_out+=("External runners: arrive in v0.24.0.")
    fi
    if ! ge "$tag" v0.28.0; then
        left_out+=("Hooks and Steam environments: arrive in v0.28.0.")
    fi
    if ge "$tag" v0.31.0; then
        left_out+=("GOG-tracked environment (\`gog_id\`): every way to make one needs a GOG login and the network.")
    else
        left_out+=("GOG settings and environments: arrive in v0.31.0.")
    fi
    if ge "$tag" v0.34.0; then
        left_out+=("GOG folder states and Galaxy records: the plan has tests write these with this release's writers.")
    fi
    if ge "$tag" v0.7.0 && ! ge "$tag" v0.24.0; then
        left_out+=("Global \`[art] fetch\`: in the file format, but \`set --global\` can't write it until v0.24.0.")
    fi
    left_out+=("\`heroic_id\`: only a Heroic import writes it, and that needs a Heroic install.")
    left_out+=("\`dxvk\` and \`vkd3d\` on an environment: setting either installs a build, and the release can only download one. The \`full\` template carries both keys instead.")
    left_out+=("A prefix with real contents: prefixes hold only what the fake wine and umu wrote, plus empty fake program files.")
    publish "${left_out[@]}"
}

# The environments that can't live in a portable folder: desktop entries, the
# Steam game, and an environment on another launcher's runner.
xdg_part() {
    ge "$tag" v0.2.0 || return 0
    x new -n desktop --dir '~/prefixes/desktop' --arch win64
    fake_exe "$home/prefixes/desktop" 'Games/Desktop' Desktop.exe
    x set desktop 'targets.main.exe=C:\Games\Desktop\Desktop.exe' targets.main.default=true \
        targets.main.desktop=true 'targets.main.name=Desktop Game'
    if ge "$tag" v0.24.0; then
        local ge_dir=$home/.local/share/Steam/compatibilitytools.d/GE-Proton-fixture f
        mkdir -p "$ge_dir/files/bin"
        for f in proton files/bin/wine files/bin/wineserver; do
            printf '#!/bin/sh\nexit 0\n' >"$ge_dir/$f"
            chmod +x "$ge_dir/$f"
        done
        printf '1717451631 GE-Proton-fixture\n' >"$ge_dir/version"
        made "wrote a fake Proton build under .local/share/Steam/compatibilitytools.d/GE-Proton-fixture"
        x set --global external_runners=true
        x new -n external --runner GE-Proton-fixture --dir '~/prefixes/external'
        x set external gameid=umu-12345 store=gog
    fi
    if ge "$tag" v0.28.0; then
        local steam=$home/.local/share/Steam
        mkdir -p "$steam/steamapps" "$steam/appcache/librarycache/1086940"
        printf '"AppState"\n{\n\t"appid"\t\t"1086940"\n\t"name"\t\t"Hades II"\n\t"StateFlags"\t\t"4"\n\t"installdir"\t\t"Hades II"\n\t"SizeOnDisk"\t\t"1024"\n}\n' \
            >"$steam/steamapps/appmanifest_1086940.acf"
        printf '%b' "$PNG" >"$steam/appcache/librarycache/1086940/library_600x900.jpg"
        made "wrote a fake Steam library with Hades II (1086940) and its cached poster"
        x import steam --game 1086940 -n steam-game
    fi
}

# environments_in DIR: "name (kind)" for each environment config in DIR.
environments_in() {
    local file kind
    for file in "$1"/*.toml; do
        [[ -e $file ]] || continue
        kind=$(sed -n 's/^kind = "\(.*\)"$/\1/p' "$file")
        printf '%s (%s)\n' "$(basename "$file" .toml)" "${kind:-wine}"
    done | paste -sd, - | sed 's/,/, /g'
}

# contents SET: what the set holds, read from the copied files.
contents() {
    local out=$1/portable xdg=$1/xdg entry
    printf '## Contents\n\n'
    printf -- '- Global config: `portable/config/config.toml`, keys: %s\n' \
        "$(sed -n 's/^\([a-z_]*\) *=.*/\1/p; s/^\[\(.*\)\]$/[\1]/p' "$out/config/config.toml" | paste -sd, - | sed 's/,/, /g')"
    printf -- '- Environments in `portable/config/environments`: %s\n' "$(environments_in "$out/config/environments")"
    printf -- '- Templates in `portable/config/templates` (hand-written): %s\n' \
        "$(find "$out/config/templates" -name '*.toml' -printf '%f\n' | sed 's/\.toml$//' | sort | paste -sd, - | sed 's/,/, /g')"
    if [[ -d $out/art ]]; then
        printf -- '- Art, under `portable/`: %s\n' "$(find "$out/art" -name art.toml | sed "s|$out/||" | sort | paste -sd, - | sed 's/,/, /g')"
    fi
    if [[ -d $out/prefixes/.wineo-snapshots ]]; then
        printf -- '- Snapshots, under `portable/prefixes/.wineo-snapshots`: %s\n' "$(find "$out/prefixes/.wineo-snapshots" -mindepth 2 -maxdepth 2 -type d | sed "s|$out/prefixes/.wineo-snapshots/||" | sort | paste -sd, - | sed 's/,/, /g')"
    fi
    if [[ -d $out/stats ]]; then
        printf -- '- Playtime logs, under `portable/stats`: %s\n' "$(find "$out/stats" -name '*.jsonl' -printf '%f\n' | sort | paste -sd, - | sed 's/,/, /g')"
    fi
    if [[ -d $out/data/trash ]]; then
        printf -- '- Trash entries under `portable/data/trash` (names in the manifests):'
        for entry in "$out"/data/trash/*/manifest.toml; do
            printf ' %s (prefix %s);' "$(sed -n 's/^name = "\(.*\)"$/\1/p' "$entry")" \
                "$(sed -n 's/^include_prefix = \(.*\)$/\1/p' "$entry" | sed 's/true/in the trash/; s/false/left in place/')"
        done
        printf '\n'
    fi
    printf -- '- XDG home environments (`xdg/.config/wineo/environments`): %s\n' \
        "$(environments_in "$xdg/.config/wineo/environments")"
    if [[ -f $xdg/.config/wineo/config.toml ]]; then
        printf -- '- XDG home global config: `xdg/.config/wineo/config.toml`, only so external runners count.\n'
    fi
    if [[ -d $xdg/.local/share/applications ]]; then
        printf -- '- Desktop entries under `xdg/.local/share/applications/`.\n'
    fi
    printf '\n'
}

# Copies the roots into the fixtures directory and writes the set's README.
publish() {
    local out=$fixtures/$tag item
    rm -rf "$out"
    mkdir -p "$out/portable" "$out/xdg"
    cp -a "$root/." "$out/portable/"
    # The XDG home: configs, data, and fake launcher folders. Run state, locks,
    # and caches live outside it.
    cp -a "$home/." "$out/xdg/"
    # Git would drop an empty directory, so the set would differ once checked out.
    if [[ -n $(find "$out" -type d -empty) ]]; then
        echo "$tag: the set has empty directories:" >&2
        find "$out" -type d -empty >&2
        return 1
    fi
    {
        printf '# %s fixture set\n\n' "$tag"
        printf 'Written by the real %s release (`wineo` built from the %s git tag) with\n' "$tag" "$tag"
        cat <<'EOT'
`scripts/generate-upgrade-fixtures.sh`, against fake wine, umu, and winetricks
programs. Do not edit by hand; rerun the script. Timestamps, the trash entry
id, and snapshot times differ from run to run.

## Layout

- `portable/` is a portable folder: copy it and run `wineo --portable <copy>`.
- `xdg/` is a HOME, a sibling of `portable/`, for what a portable folder
  cannot hold: desktop entries are refused there, Steam games cannot be
  imported into one, and `external_runners` does nothing in one. Copy it to a
  temporary HOME and point `XDG_CONFIG_HOME` and `XDG_DATA_HOME` at `.config`
  and `.local/share` under it.
- Prefixes hold only `system.reg`, `dosdevices/c:`, `drive_c/` (with an empty
  `windows/win.ini`, since git keeps no empty directories), and empty fake
  program files, plus `winetricks.log` where winetricks ran. Prefix paths in
  configs are relative to the portable folder or start with `~`.
- The trash entry directory is named by time and a random suffix; glob
  `portable/data/trash/*/`. Its manifest records a device and inode that never
  match a copy. A trashed prefix lives under `portable/prefixes/.wineo-trash/`.

EOT
        contents "$out"
        cat <<'EOT'
## Commands run, in order

`$ROOT` is the portable folder, `$HOME` the XDG home. Lines marked
`(not wineo)` are files made by the script, not by the release.

```
EOT
        cat "$cmds"
        cat <<'EOT'
```

## Hand-written files

- `config/templates/*.toml`: no release has a command that writes a template,
  so they are written in the shape that release documents. The release
  itself lists them with `wineo templates`, and `mygame` builds environments.

## Left out

EOT
        for item in "$@"; do
            printf -- '- %s\n' "$item"
        done
    } >"$out/README.md"
}

# ---------------------------------------------------------------------------

main() {
    local tags=() arg
    for arg in "$@"; do
        case $arg in
        --keep-roots) keep_roots=1 ;;
        -h | --help)
            sed -n '2,19p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        v*) tags+=("$arg") ;;
        *)
            echo "unknown argument: $arg" >&2
            exit 2
            ;;
        esac
    done
    [[ ${#tags[@]} -gt 0 ]] || tags=("${TAGS_ALL[@]}")
    make_stub
    for tag in "${tags[@]}"; do
        build_tag "$tag"
        generate "$tag"
        echo "wrote $fixtures/$tag" >&2
    done
    if [[ $keep_roots -eq 0 ]]; then
        rm -rf "$WORK/roots"
    fi
}

main "$@"
