# Downloads and imports

## Download a runner or graphics component

```sh
wineo download
wineo download ge-proton --list
wineo download ge-proton
wineo download proton-cachyos
wineo download dxvk
wineo download vkd3d
```

The guided command offers providers and releases. A provider without a version
selects its latest supported stable upstream release. Use `--version <tag>` for
an exact release and `--list --page 2` for older releases.

Downloads support Linux x86_64. CachyOS selection uses its baseline SLR build.
wineo verifies published checksums before extraction and installs each build in
its own directory. Existing directories are never replaced. An identical
completed download is a no-op. Cancellation removes incomplete staging data.

Local runner and component listings remain offline. Launching never downloads a
runner automatically.

## Follow the newest installed runner

```sh
wineo set my-game runner=GE-Proton-Latest
wineo set my-game runner=Proton-CachyOS-Latest
wineo runners
```

These aliases select the newest compatible stable build of their family that
is already installed. Downloading a newer build affects the next operation.
An active game or installer keeps its original runner, including cleanup.
Status shows the concrete runner used for that operation.

Selecting a concrete runner name keeps it pinned. Aliases do not check for
updates or fall back to another runner family. The two alias names are reserved;
do not create directories or symlinks with those names in the runner root.

## Import an existing prefix

```sh
wineo import
wineo import prefix /path/to/prefix
wineo import prefix /path/to/prefix -n my-game --runner GE-Proton-Latest
wineo import prefix /path/to/prefix -n my-game --runner wine --adopt
```

Copying is the default. `--dir /destination` changes the copy destination.
`--adopt` registers the original folder in place and cannot be combined with
`--dir`. The selected runner must match the Wine or Proton layout. A Proton
import includes the complete compatdata directory, even when its `pfx` path was
supplied. Use `--arch win32` or `--arch win64` only when the registry has no
architecture marker.

Import preserves prefix files and registry settings without running Wine.
Symlinks are preserved, with internal absolute links rebased to the copy.
External game folders, linked saves, and other external data stay where they
are. Review lists those dependencies. Stop applications using the source before
importing; a copy is not a live snapshot.

Import checks that the source hasn't changed since review, before and after
the copy. The check compares file metadata (size, times, inode, links), not
contents, so it stays fast on large prefixes; touching a file between review
and import is enough to ask for a fresh review. A folder whose subfolders are
themselves prefixes, like Heroic's `Prefixes/default`, is refused: choose the
game's own prefix instead. Clone, snapshot, and portable copy refuse such a
folder too.

Fully specified imports register the prefix without prompting. Guided imports
offer target setup afterward, which can be skipped or revisited:

```sh
wineo setup my-game
```

If registration fails after copying, the completed copy is retained. The error
reports its location so it can be registered with `--adopt`.

## Import a manually added Heroic game

```sh
wineo import heroic --list
wineo import heroic
wineo import heroic --config-dir /path/to/heroic
wineo import heroic --game <id> -n my-game --runner <installed-runner> [--art]
```

wineo reads local Heroic settings once. It supports manually added Windows games
and [native Linux games](#native-heroic-games), with source selection when native and Flatpak libraries are both present.
Heroic files are never edited. Store libraries and credentials are not imported.

The guided flow can copy Heroic's runner, choose an installed runner, or download
a supported replacement. Runner installation finishes before prefix copying.
A successfully installed runner remains available if the import is canceled.

Review covers the executable, argument boundaries, working directory,
environment variables, gamemode, [launch options](#launch-options), and
supported compatibility toggles. Unsupported
wrappers, scripts, and runtime settings are omitted, and settings wineo manages
itself are dropped. The summary lists both as warnings, with `-v` naming every
omitted setting. Noninteractive imports refuse unresolved settings; add
`--interactive` to review them. Import does not launch the game automatically.

With a Wine runner, Heroic's DXVK and vkd3d-proton toggles become
`dxvk = "latest"` and `vkd3d = "latest"`. Proton bundles its own, so Proton
imports leave them out. When no local build exists, guided imports offer to
download the latest release. Noninteractive imports stop and name the
`wineo download` command to run.

With a SteamGridDB key, the guided review has an `art` row that fetches art
searching the Heroic title, and the fully specified flow fetches it with
`--art`. See [Fetching during new and
import](art.md#fetching-during-new-and-import).

### Launch options

Heroic's gamescope, MangoHud, and Wine Wayland settings become wineo's
[launch options](launch-options.md), following the same rules Heroic's own
launcher does:

| Heroic | wineo |
|---|---|
| Show MangoHud | `mangohud = true` |
| Gamescope, with upscaling or the FPS limiter on | a `[gamescope]` table |
| Enable Wine Wayland | `wayland = true`, when the chosen runner supports it |
| Enable HDR, with Wine Wayland imported | `PROTON_ENABLE_HDR = "1"` on Proton or `DXVK_HDR = "1"` on Wine, in `[env]` |

Heroic only runs gamescope when upscaling or the limiter is on, so its
gamescope settings are skipped otherwise. Game and upscale resolutions become
`width`, `height`, `output_width`, and `output_height`; FSR and NIS become
`filter`, integer and stretch become `scaler`; fullscreen and borderless
become `window`. The limiter's values become `fps_limit` and
`fps_limit_unfocused`, the cursor grab becomes `grab_cursor`, and additional
options are split into `args`. A value that isn't a number is skipped with a
warning.

MangoHud or Wine Wayland turned off in Heroic is written as `false`, so a
global setting in wineo doesn't turn it back on. Wine Wayland on a runner
without Wayland support is skipped with a warning naming the runner. HDR without Wine Wayland, and Heroic's WoW64 toggle, are
reported as not imported, since the runner's own build decides WoW64. An
`[env]` variable already set by the game's Heroic environment options wins
over the HDR one. The guided review and `-v` show each carried setting;
`--all`'s one line per game doesn't.

### Import every Heroic game

```sh
wineo import heroic --all [--runner <name>] [--adopt] [-d] [--art] [--yes]
```

Every Windows game uses `--runner`, or the newest installed Proton. A library
of only native games needs no runner. Games already in
wineo are skipped, matched by recorded Heroic id, adopted prefix, name, or
target executable. A table lists each game as copied, adopted, or skipped with
a reason, and one confirmation covers them all. Review items take the guided
defaults: unsupported settings are omitted, conflicting managed settings are
dropped, and settings needing attention are kept, each noted in the table.

`-d`/`--desktop` creates a desktop entry for every game without asking. `--yes`
skips the confirmation, works without a terminal, and creates no desktop
entries unless `--desktop` is also given. With a SteamGridDB key, `--all`
also asks `Fetch art for these games?`, which `--art` answers the same way;
each game takes the top search match for its Heroic title, and an art
failure never fails the game. Failed games don't stop the rest, and
the command exits nonzero if any failed. `-v` prints each game's full summary
and writes its import log.

### Native Heroic games

A native game added to Heroic (platform `linux`) imports as a [native
environment](native.md), with no runner, prefix, or copy question:

```
Import Dusklight as dusklight
  kind: native
  exe: /usr/bin/dusklight
  gamemode: off
```

It gets one default `game` target from Heroic's executable, plus its launch
arguments, environment variables, gamemode, MangoHud, gamescope, and
`nvidiaPrime`. Wine Wayland and HDR are ignored, as Heroic does for native
games. Variables that
wineo manages for Wine, like `WINEPREFIX`, are kept as written, since wineo
manages none of them for native. Heroic's Wine version, prefix, DXVK, and
other Wine settings are ignored, and wrappers are reported as not imported,
the same as for Windows games. `--runner`, `--adopt`, `--dir`, and `--arch`
are refused for a native game.

A native game whose executable is missing can't be imported, and `--all`
skips it:

```
skip: executable not found: /usr/bin/soh
```

Mac games are still refused, since wineo only runs Linux ones.

## Removal

`wineo rm my-game` trashes the config, art, and playtime log while leaving the
prefix and snapshots in place. `wineo rm my-game --delete-prefix` also trashes
the configured prefix and its snapshots. External game files and Heroic
metadata are not included. See [Trash and removal](trash.md) for restore,
prune, and recovery.
