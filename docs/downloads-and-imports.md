# Downloads and imports

## Download a runner or graphics component

```sh
wineo runners download
wineo runners download ge-proton --list
wineo runners download ge-proton
wineo runners download proton-cachyos
wineo dxvk download
wineo vkd3d download
```

Runners download under `wineo runners`, and DXVK and vkd3d-proton under
their own commands. The guided `wineo runners download` offers providers and
releases. A download without a version selects the latest supported stable
upstream release. Use `--version <tag>` for an exact release and
`--list --page 2` for older releases.

`wineo download`, the old spelling, keeps working until 2.0 and warns with
the replacement for the provider you named.

Downloads support Linux x86_64. CachyOS selection uses its baseline SLR build.
wineo verifies published checksums before extraction and installs each build in
its own directory. Existing directories are never replaced. An identical
completed download is a no-op. Cancellation removes incomplete staging data.

Local runner and component listings remain offline. Launching never downloads a
runner automatically.

## Update what's installed

```sh
wineo runners update
wineo runners update ge-proton dxvk
wineo runners update --check
```

Gets the newest release of every family you already have a build of:
GE-Proton, Proton-CachyOS, DXVK, and vkd3d-proton. Name one or more
families to update only those; `wineo dxvk update` and `wineo vkd3d
update` do the same for one family, with `--check` and `--json` alike. It
doesn't ask first, and a run can download about 1 GiB.

```
$ wineo runners update
GE-Proton       GE-Proton10-20 -> GE-Proton10-27 (498 MiB)
Proton-CachyOS  up to date (proton-cachyos-10.0-20261101-slr-x86_64)
DXVK            dxvk-2.6 -> dxvk-2.7.1 (9 MiB)
Downloading GE-Proton10-27
...
Installed GE-Proton10-27 and dxvk-2.7.1.
Nothing uses GE-Proton10-20 or dxvk-2.6 now. Remove them with `wineo runners remove NAME` and `wineo dxvk remove NAME`.
```

`--check` prints the table and downloads nothing. It asks GitHub, so it
needs the network; `--check --json` is in [JSON output](json-output.md).

A family counts as installed when wineo's own root holds a build of it,
by the rule the `*-Latest` aliases use: a build you extracted there by
hand counts, a build from a prerelease doesn't, and builds from Steam,
Heroic, and Lutris never do. DXVK and vkd3d-proton count only under the
names downloads give them, `dxvk-2.6` and `vkd3d-proton-2.14`. A family
you name that isn't installed is an error, with the command that
downloads it.

Older builds stay. An environment set to a concrete build keeps it, and
one on `GE-Proton-Latest` or `dxvk = "latest"` moves to the new build the
next time it starts; a game that's running is untouched. The line after
the installs names the older builds nothing uses any more. It's left out
when a config couldn't be read, since then wineo can't tell what's in use.

One family failing doesn't stop the others: the rest still update, a
summary says which didn't, and the command exits 1. Once GitHub says its
hourly limit for requests without a login is used up, the families left
are reported with that error and not asked about. Ctrl+C exits 130 and
keeps any build that had finished. See [Exit codes](exit-codes.md).

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
updates or fall back to another runner family; `wineo runners update` gets
the newer build they then follow. The two alias names are reserved;
do not create directories or symlinks with those names in the runner root.

## Use builds from Steam, Heroic, and Lutris

wineo can use Proton and Wine builds that Steam, Heroic, and Lutris already
installed, read where they are, instead of downloading or copying them again.
It's off until you say yes. Guided `new` asks once when you pick a template
tagged `game`, and guided `wineo import heroic` asks before choosing a
runner:

```
Use Proton and Wine builds from Steam, Heroic, and Lutris too? [Y/n]
```

Either answer is saved as `external_runners` in the global config, and
`wineo set --global external_runners=true` changes it later. Runs that pass
flags answering part of the flow, `--all`, and runs without a terminal never
ask. See [Runners from other launchers](config-files.md#runners-from-other-launchers).

With it on, `wineo runners` lists each build's source:

```
wine                   wine            wine-11.19     /usr/bin/wine
GE-Proton-latest       proton  wineo   GE-Proton10-3
GE-Proton11-1          proton  Heroic  GE-Proton11-1
GE-Proton11-3          proton  Steam   GE-Proton11-3
GE-Proton11-7          proton  wineo   GE-Proton11-7
Proton-9.0             proton  Steam   Proton-9.0
GE-Proton-Latest       alias           -> GE-Proton11-7
```

wineo looks in:

- Steam: `compatibilitytools.d` under `~/.steam/root`, `~/.local/share/Steam`,
  and Flatpak Steam, plus Valve's Proton in each library's
  `steamapps/common`.
- Heroic: `tools/proton` and `tools/wine`, native and Flatpak.
- Lutris: `runners/wine`, native and Flatpak.

A folder name wineo can't use gets a derived one, so Valve's `Proton 9.0` is
`Proton-9.0`. When the same name is in several places, the first folder
that has it owns it, in the order wineo's runners root, Steam, Heroic,
Lutris, even if that copy is broken. If a hidden copy is a different build,
its row says so: `(Heroic has a different build)`. `wine` and the
`*-Latest` aliases keep their meaning; the aliases still follow only
builds wineo downloaded. `runner = "proton"`, which the gaming template uses,
picks the newest Proton from every source. It compares names, not release
dates, so between families the name decides: every `proton-cachyos` build
sorts above every `GE-Proton` build, and so does Steam's `Proton-9.0` when
other launchers' builds are on. Set a `*-Latest` alias or a concrete build
when you want one family.

wineo never changes or deletes another launcher's builds. `wineo runners
remove` refuses with `GE-Proton11-3 belongs to Steam.`, and so does a
runners root you pointed inside a launcher's folder. If Steam or Heroic
deletes a build an environment uses, launching reports an unknown runner, as
it would for a build deleted by hand. Builds from other launchers are never
used while a portable folder is active.

## Remove a runner or graphics component

```sh
wineo runners remove GE-Proton9-20
wineo dxvk remove dxvk-2.5.3
wineo vkd3d remove vkd3d-proton-2.14 --yes
```

Deletes an installed build from its root. `rm` works in place of `remove`. On a terminal it asks first with
the build's size, `Remove runner GE-Proton9-20 (1.2 GiB)? [y/N]:`, and prints
`Removed runner GE-Proton9-20 (1.2 GiB freed)`. `--yes` skips the question.
Without a terminal it needs `--yes`, and otherwise stops with `remove
requires a terminal or --yes; nothing was removed`.

A build that anything still uses is refused, naming the users:

```
GE-Proton9-20 is used by hades, celeste (GE-Proton-Latest), and template gaming (proton).
```

A use is an environment that names the build, one whose
`GE-Proton-Latest`, `Proton-CachyOS-Latest`, or DXVK or vkd3d `latest`
resolves to it today, a template that names it or whose `proton` picks it as
the newest Proton, the global default runner, a running game or installer,
or an installer whose leftovers haven't been cleaned up. Deleting the newest
build behind an alias is refused rather than quietly moving those
environments to an older one. A build that's being downloaded or is held by
a running program is refused too. System Wine and runners outside the
runner root can't be removed.

An environment or template config that can't be read might use the build,
so the removal is refused before the question and nothing is touched:

```
error: GE-Proton9-20 is kept, since a config can't be read
invalid config ~/.config/wineo/environments/hades.toml: missing field `prefix`
```

Each config's own error follows the first line. Fix or remove the file and
run the removal again. A build that's in use is reported as in use first.
The desktop app's Downloads page still lists the builds and what uses them,
with a warning naming each file it skipped.

The build is renamed out of the way before it's deleted, so an interrupted
removal never leaves a half build that lists as usable. The next removal in
that root cleans up what's left.

## Guided import

`wineo import` with nothing else asks what to import: an existing prefix, a
manually added Heroic game, a Steam game, a [GOG game](gog.md), an
[Epic game](epic.md), or emulated games. Choosing Epic logs you in, or lists
the Epic games you own once you are; install one with `wineo import epic
--game ID`.

## Import an existing prefix

Add `--dry-run` to any non-guided import to see what it would do without
changing anything. See [Dry runs](dry-run.md).

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
are. Review counts the links that point to them, mostly Wine's drive mappings
in `dosdevices`; add `-v` to list each one. Stop applications using the source
before importing; a copy is not a live snapshot.

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

## Import Steam games

```sh
wineo import steam --list
wineo import steam --game 1086940
wineo import steam --all
wineo import steam --all --owned
```

Steam games import as proxied environments that Steam keeps launching and
owning. Games you own but haven't installed import too, with a Steam Web API
key, and `wineo install` installs them through Steam. See
[Steam games](steam.md) and its [owned games](steam.md#owned-games) section.

## Import emulated games

```sh
wineo import roms ~/Games/roms/gamecube --emulator dolphin --list
wineo import roms ~/Games/roms/snes --emulator retroarch --core snes9x --all
wineo import roms --emulators
```

Games from a folder of ROMs, or one game's file, import as native
environments that run the emulator. See [Emulated games](emulators.md).

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

With [builds from other launchers](#use-builds-from-steam-heroic-and-lutris)
on, the game uses Heroic's runner where it is, with no copy, and the review
shows it as `GE-Proton-latest (Heroic)`. Otherwise the guided flow can copy
Heroic's runner, choose an installed runner, or download a supported
replacement. Runner installation finishes before prefix copying.
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
download command to run.

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
| Scripts to run before and after launch | `hooks.before` and `hooks.after`; see [Hooks](hooks.md) |

Heroic only runs gamescope when upscaling or the limiter is on, so its
gamescope settings are skipped otherwise. Game and upscale resolutions become
`width`, `height`, `output_width`, and `output_height`; FSR and NIS become
`filter`, integer and stretch become `scaler`; fullscreen and borderless
become `window`. The limiter's values become `fps_limit` and
`fps_limit_unfocused`, the cursor grab becomes `grab_cursor`, and additional
options are split into `args`. A value that isn't a number is skipped with a
warning.

Each script path is quoted so `sh` runs exactly that file, whatever
characters its name has. The review lists the scripts as Heroic stored
them, and the bulk table marks a game that `runs launch scripts`.

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
prefix and snapshots in place. `wineo rm my-game --include-prefix` also trashes
the configured prefix and its snapshots. External game files and Heroic
metadata are not included. See [Trash and removal](trash.md) for restore,
prune, and recovery.
