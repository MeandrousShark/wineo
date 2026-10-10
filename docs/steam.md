# Steam games

Games you own on Steam can join the rest of the library: tagged,
counted in `wineo stats`, started and stopped from the CLI or the desktop app,
with [hooks](hooks.md) around them. Steam keeps owning the game, its prefix,
and its process. Wineo reads Steam's files but never writes to them.

A Steam game is a proxied environment, a third kind next to Wine and
[native](native.md). Only what Steam allows applies to it: arguments, tags, a
title, hooks, and a desktop entry. Snapshots, winetricks, `exec`, clone, and
portable copies don't, since Steam owns the prefix.

## Importing

```
$ wineo import steam --list
APP ID   TITLE              SIZE     STATE           IMPORTED
1086940  Baldur's Gate 3    140 GiB  installed       bg3
2142790  Fields of Mistria  612 MiB  installing 42%
292030   The Witcher 3      -        not installed
```

`--list` shows each game's app id, title, size, state, and the environment
it's imported as. Installed games come first. Games you own but haven't
installed follow, by title, when wineo has a
[Steam Web API key](#owned-games); without one, only installed games are
listed, with `Add a Steam Web API key to see uninstalled games.` Steam's own
tools, like Proton builds, the Steam Linux Runtimes, and
Steamworks Common Redistributables, are left out. A game whose manifest
can't be read shows as `unreadable manifest` and doesn't hide the rest.

Import one game by app id:

```
$ wineo import steam --game 1086940 -n bg3
Ready to import bg3:
  game      Baldur's Gate 3 (1086940)
  size      140 GiB
  launcher  Steam
  tags      game
  desktop   off
  art       Steam cache
Imported bg3.
```

The name defaults to a slug of the title, like `baldurs-gate-3`. A name
another environment already has is an error. A game is already imported when
any environment has its app id, whatever it's called, so importing it again
does nothing:

```
$ wineo import steam --game 1086940
Already imported as bg3.
```

Import every installed game that isn't imported yet, or with `--owned`,
every game you own:

```sh
wineo import steam --all --dry-run
wineo import steam --all
wineo import steam --all --owned
```

`--all` reviews each game and asks once, or `--yes` skips the question,
which a script needs since there's no terminal to ask at. One game failing
doesn't stop the rest, and the summary counts what happened: `Imported 1 of 2
games.`, then `1 already imported: NAME` and `1 failed: NAME` when those
apply. Guided `wineo import` offers Steam as a source too.

Import writes the config, tags the game `game`, and copies Steam's cached
art: the poster as Grid, the hero image as Hero, and the logo as Logo. A
missing art file is skipped, and one that isn't a valid image is skipped
with a warning. Art never fails an import. `--art` fills in the rest from
SteamGridDB, looked up by the Steam app id rather than a title search.

Steam games can't be imported into a [portable folder](portable.md), since
the folder would depend on this machine's Steam.

## Owned games

Steam only lists the games you own through its Web API, which needs a key.
Get one at <https://steamcommunity.com/dev/apikey> (Steam may refuse one to
an account with no purchases or without Steam Guard), then save it:

```
$ wineo import steam --key
Steam Web API key:
Saved the Steam Web API key to the keyring
```

Wineo checks the key with Steam before saving it, and never prints it. It
goes in the keyring like the [SteamGridDB key](art.md#the-steamgriddb-key), or
in a private `steam.key` file under wineo's data folder when there's no
keyring. `WINEO_STEAM_KEY` overrides both. `--remove-key` removes it. A key
sees its own account's library even when the profile is private. Wineo
finds the account itself, from the most recently logged in Steam user.

The owned list is cached in `~/.cache/wineo/steam-owned-<steamid>.json` and
fetched again when it's over an hour old, or right away with `--refresh`.
Only import commands use it; the library, `list`, and the desktop app's
library never go online for it.

- If a fetch fails and there's a cached list, wineo uses it and warns:
  `Couldn't refresh owned games: REASON`.
- With no cached list, listings show installed games only, with `Couldn't
  get owned games: REASON`, and importing an uninstalled game fails with
  the same message.
- When several Steam accounts have logged in and none is marked most
  recent, owned games fail with `Steam doesn't say which account was used
  last.`

Importing an installed game never needs the key or the network. An
uninstalled game imports the same way, with its title from the owned list
and Steam's cached art when Steam has it. An app id that's neither installed
nor owned is refused: `App 12345 isn't installed or owned.` Steam's API
sometimes misses games, mostly free ones; install those in Steam first and
import them from the local scan.

## Installing and uninstalling

```
$ wineo install witcher3
Asked Steam to install witcher3.
$ wineo uninstall witcher3
Asked Steam to uninstall witcher3.
```

Both open Steam's own dialog (`steam://install/<app_id>` or
`steam://uninstall/<app_id>`), starting Steam if it isn't running, and
return. Steam's dialog picks the library and asks before uninstalling, so
wineo doesn't ask again. Uninstalling keeps wineo's config, and the game
goes back to `not installed`.

`-f` (`--follow`) stays and shows progress until Steam is done:

```
$ wineo install witcher3 -f
Waiting for Steam's install dialog...
Installing The Witcher 3... 42% (14.2 GiB of 33.8 GiB)
Installed.
```

- On a terminal the progress line updates in place. Piped, it prints a line
  each time the state or percent changes.
- Steam rewrites its progress only now and then, so the percent moves in
  jumps. A paused install looks the same on disk as one that hasn't started
  downloading, so it shows without a percent until Steam resumes.
- It waits for the dialog as long as it's open. Ctrl+C stops following,
  never the install, and exits 130.
- If the game disappears partway, it prints `Steam canceled the install.`
  and exits 1. An uninstall that ends with the game still installed prints
  `Steam canceled the uninstall.`

| Game is | `install` | `uninstall` |
|---|---|---|
| installed | `Already installed.` | Opens the dialog |
| starting or running, from wineo or Steam | `Already installed.` | Refused: `Stop bg3 before uninstalling it.` |
| install queued | Opens the dialog | Opens the dialog |
| installing | `Steam is already installing bg3.` (`-f` follows it) | Opens the dialog |
| uninstalling | Refused: `Steam is uninstalling bg3.` | Opens the dialog |
| not installed | Opens the dialog | `Not installed.` |
| unavailable or unknown | Refused with the reason | Refused with the reason |

Only Steam games can be installed or uninstalled.

## The config

```toml
# environments/bg3.toml
schema_version = 1
kind = "proxied"
launcher = "steam"
app_id = "1086940"
title = "Baldur's Gate 3"
tags = ["game"]
desktop = false

[hooks]
after = 'notify-send "Saves synced"'
```

A proxied config can hold `tags`, `title`, `[hooks]`, and `desktop`, and
nothing Wine or native. Setting anything else is refused:

```
$ wineo set bg3 runner=proton
error: bg3 is a Steam game, so it can't set runner.
```

`title` is written at import. While Steam has the game's manifest, its name
wins, and `title` is shown once Steam no longer knows the game. Install state,
size, and Proton build are read from Steam each time and never stored.

A config written by hand works too. Start from
[`proxied-environment.toml.example`](proxied-environment.toml.example)
(installed to `/usr/share/doc/wineo/proxied-environment.toml.example` by the
Arch package). See [Config files](config-files.md#proxied-environments) for
the schema.

## Which Steam

Wineo finds native Steam (`~/.local/share/Steam`, which `~/.steam/steam` and
`~/.steam/root` point at) and Flatpak Steam. It uses one of them for
everything: import, launch, art, playtime, and finding running games. With
both installed, native wins unless the global config says otherwise:

```sh
wineo set --global steam=flatpak
wineo set --global --unset steam
```

A game only the other install has counts as not installed, so the two are
never mixed for one game. Runner discovery is separate and still scans both;
see [Downloads and imports](downloads-and-imports.md#use-builds-from-steam-heroic-and-lutris).

## Launching

`wineo start bg3` asks Steam to launch the game with `steam -applaunch
1086940`, or through `flatpak run` for Flatpak Steam, and returns once Steam
accepts:

```
$ wineo start bg3
Asked Steam to start bg3.
```

The run is `starting` until the game's process appears, then `running`.
Steam may have to start itself or update the game first, so the wait is
generous: 60 seconds for the game to appear, and while Steam is updating it,
as long as the download or staging keeps making progress, giving up after
10 minutes without any. A game that never appears ends the run with `Steam
didn't start bg3.`

`wineo run bg3` stays in the foreground through all of it. It exits 0 when
the game exits on its own, 130 when it's stopped, and 1 when Steam never
starts it. The game's own exit code can't be collected, since Steam started
it, not wineo.

Only arguments reach the game, through `wineo start bg3 -- ARGS`.
Environment variables, gamemode, MangoHud, and gamescope only reach a Steam
game through its launch options in Steam, so `-e` is refused:

```
$ wineo start bg3 -e DXVK_HUD=fps
error: bg3 is a Steam game, so set environment variables in its Steam launch options.
```

The game has one implicit target, `steam`, so `--target steam` works and any
other target name is an error.

### Games started from Steam

A game started from the Steam client shows as running too:

```
$ wineo status bg3
bg3: running (started from Steam)
```

`start` on it says it's already running and exits 0, and `run` exits 1, as
for any running environment. No hooks run for it, since wineo didn't launch
it. If Steam starts the game while a `before` hook is running, say from a
click in Steam, wineo takes it over instead of asking for a second launch,
and `after` runs once it exits:

```
Started bg3 (Steam had already launched it).
```

## Stopping

`wineo stop bg3` and Stop in the desktop app end the game's process tree:
Steam's `reaper` for the game and everything under it. The Steam client
itself is never signaled. Each process is checked to still be the same one
before it's signaled, so a reused pid is never hit.

Stop sends SIGTERM, then SIGKILL to whatever is left after `stop_timeout`.
`--force` and Force stop in the app go straight to SIGKILL. Stop works the
same on a game started from Steam.

Stop during `starting` ends the wait right away and the run ends `stopped`.
Steam can't be asked to take back a launch it already queued, so a game that
appears afterward shows as `running (started from Steam)`.

When more than one process matches the game, wineo leaves them all alone and
says `More than one Steam process matches bg3.`

## Install states

Each Steam game is in one of these states, shown in `list`, `status`, and
the desktop app:

| State | Meaning | Start |
|---|---|---|
| installed | Its manifest says fully installed | Works |
| updating | Steam is updating an installed game | Works, and waits in `starting` |
| installing | Steam is installing it, with a percent once Steam knows the size | Refused: `Steam is still installing bg3.` |
| install queued | Install was pressed, and the download hasn't started | Refused: `Steam lists bg3 as not installed.` |
| uninstalling | Steam is removing it | Refused: `Steam is uninstalling bg3.` |
| not installed | Steam is readable and no library has it | Refused: `Steam lists bg3 as not installed.` |
| unavailable | A library Steam lists is missing, so it might be there | Refused: `Steam's library at /mnt/games isn't available.` |
| unknown | Steam isn't found, or one of its files can't be read | Refused with the reason |

```
$ wineo list
bg3       not installed
hades     updating
witcher3  installing 42%
```

`list` shows the state in place of `inactive` when the game isn't plainly
installed, and `status` adds the reason for unavailable and unknown. A
starting or running game shows as such. In the desktop app, a game that isn't
installed is grayed out, and unavailable or unknown shows a warning, since
those games may still be on disk.

## Playtime

Steam's own total is the source of truth. Wineo records no sessions for
Steam games, so play started from Steam counts too and nothing is counted
twice.

```
$ wineo stats bg3
bg3
  played      370 h on Steam, last Sep 4
```

The total comes from the most recently logged in Steam account. Steam
doesn't keep sessions, so there are no session, average, longest, or
per-week lines, and the overview shows `-` under sessions. When Steam's files
can't be read, playtime shows as unknown, not zero, and the game is left out
of totals. See [Playtime](stats.md#steam-playtime).

## Hooks

`[hooks]` works as for any environment; see [Hooks](hooks.md). A Steam
game's hooks also get `WINEO_LAUNCHER=steam` and `WINEO_APP_ID`, and
`WINEO_TARGET` is `steam`. `WINEO_PREFIX` is Steam's
`compatdata/<app_id>/pfx` when the game has one, so a save-sync hook can find
a Proton game's saves. `WINEO_EXIT_CODE` is always empty, and
`WINEO_SECONDS` covers only the time the game was running, not the wait in
`starting`.

## Other commands

| Command | Steam game |
|---|---|
| `start`, `run`, `restart`, `stop`, `status`, `list`, `stats` | Work |
| `set` | `tags`, `title`, `hooks`, and `desktop` only |
| `art` | Works, and `art fetch` looks the game up on SteamGridDB by app id unless `--game` or `--search` says otherwise |
| `desktop` | `desktop = true` adds `wineo-NAME-steam.desktop`, which runs `wineo start NAME` so hooks run |
| `rm` | Removes wineo's config and art. It never uninstalls the game |
| `validate` | Adds Steam checks: Steam not found, no active install, library unavailable, unreadable files. A game that isn't installed, or is installing, isn't a problem and isn't reported |
| `winetricks`, `exec`, `snapshot`, `clone`, `portable copy` | Refused: `Steam manages bg3's prefix.` |
| `setup` | Refused, since there are no targets to set up |

Nothing global reaches a Steam game: the global `[env]`, gamemode, MangoHud,
gamescope, Wayland, and template defaults are skipped. Only `stop_timeout`
applies, to Stop.

`status --json` and `list --json` have `"kind": "proxied"`, plus `launcher`,
`app_id`, `install`, `install_progress`, and `started_by`, which says whether a running game was
started by `wineo` or `steam`. See [JSON output](json-output.md).
