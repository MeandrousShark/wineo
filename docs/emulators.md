# Emulated games

wineo can import games that run in an emulator, from a folder of ROMs or a
single game's file. Each game becomes a [native environment](native.md)
whose target runs the emulator with the game, plus an
[`[emulation]` table](native.md#emulated-games) saying where it came from.
Launching, stopping, playtime, tags, art, hooks, and desktop entries work
as for any other game.

```sh
wineo import roms ~/Games/roms/gamecube --emulator dolphin --list
wineo import roms ~/Games/roms/gamecube --emulator dolphin --all --art
wineo import roms ~/Games/roms/snes --emulator retroarch --core snes9x --all
wineo import roms "~/Games/roms/gamecube/Metroid Prime (USA).rvz" --emulator dolphin
wineo import roms ~/Games/roms/wiiu --emulator cemu --exe ~/Apps/Cemu.AppImage --all
wineo import roms ~/Games/roms/switch --custom ~/Apps/eden.AppImage \
    --args "-f -g {rom}" --ext nsp,xci --system switch --all
wineo import roms --emulators
```

Bare `wineo import roms`, or the "Emulated games" choice in `wineo
import`, asks its questions in turn: the emulator, the folder or file, the
core for RetroArch, the system when the files don't tell, and which game.

## Emulators

| Id | Emulator | Systems | Program names | Flatpak |
|---|---|---|---|---|
| `dolphin` | Dolphin | `gamecube`, `wii` | `dolphin-emu` | `org.DolphinEmu.dolphin-emu` |
| `retroarch` | RetroArch | from its [core](native.md#retroarch) | `retroarch` | `org.libretro.RetroArch` |
| `cemu` | Cemu | `wiiu` | `cemu`, `Cemu` | `info.cemu.Cemu` |
| `pcsx2` | PCSX2 | `ps2` | `pcsx2-qt`, `pcsx2` | `net.pcsx2.PCSX2` |
| `duckstation` | DuckStation | `ps1` | `duckstation-qt`, `duckstation-nogui` | `org.duckstation.DuckStation` |
| `ppsspp` | PPSSPP | `psp` | `ppsspp`, `PPSSPPSDL`, `PPSSPPQt` | `org.ppsspp.PPSSPP` |
| `rpcs3` | RPCS3 | `ps3` | `rpcs3` | `net.rpcs3.RPCS3` |
| `melonds` | melonDS | `nds` | `melonDS`, `melonds` | `net.kuribo64.melonDS` |
| `mgba` | mGBA | `gb`, `gbc`, `gba` | `mgba-qt`, `mgba` | `io.mgba.mGBA` |
| `xemu` | xemu | `xbox` | `xemu` | `app.xemu.xemu` |
| `azahar` | Azahar | `3ds` | `azahar` | `org.azahar_emu.Azahar` |

wineo looks for each emulator on `PATH` (the global config's `env.PATH`
when it sets one), then as a Flatpak, system-wide first, then yours.
When both are installed, the one on `PATH` is used. `--exe PATH` runs a
built-in emulator from somewhere else, like an AppImage, or picks the
Flatpak by naming its export script.

`--emulators` lists what wineo found, and RetroArch's cores:

```
$ wineo import roms --emulators
ID         NAME       FOUND
dolphin    Dolphin    /usr/bin/dolphin-emu
retroarch  RetroArch  /usr/bin/retroarch

Cores for RetroArch (retroarch):
  CORE    NAME                            SYSTEM
  snes9x  Nintendo - SNES / SFC (Snes9x)  snes
```

When two cores share an id, like a distro snes9x and one from RetroArch's
online updater, a FOLDER column tells them apart.

## What's found

The files in the folder and its subfolders that match the emulator's file
types, plus its folder games: Cemu title folders (with `code`, `content`,
and `meta`, for a game or a demo), PS3 disc folders for RPCS3, and PSP
folders with an `EBOOT.PBP` for PPSSPP. A `.bin` or `.img` beside a `.cue`
isn't listed, and neither is a file an `.m3u` lists. A subfolder wineo
can't read is skipped with a warning.

A game's title is its file name without the extension and bracketed notes,
so `Metroid Prime (USA) (Rev 1) [!].rvz` becomes `Metroid Prime`. Its
environment name is the title as a slug; two games that would get the same
name get their system added, then a number.

A ROM that some environment already has as its `[emulation] rom` shows as
imported and isn't imported again, so running the import on the same
folder again adds only the new games.

```
$ wineo import roms ~/Games/roms/snes --emulator retroarch --list
TITLE           SYSTEM  FILE                            IMPORTED
Chrono Trigger  snes    Chrono Trigger (USA).sfc        chrono-trigger
Super Metroid   snes    Super Metroid (Japan, USA).sfc
```

## Choosing games

- `--list` lists the games without importing anything.
- `--game NAME` imports one game, by its title, its file name, or its path
  (relative to the folder, or full). When several games match, the error
  lists their paths so you can pass one.
- `--all` imports every game not imported yet, after a confirmation.
  `--yes` skips it.
- A single file, or a folder game, given as the path imports that game
  directly, with no confirmation. `--all` and a `--game` that names another
  game are refused.
- `--dry-run` shows the review and changes nothing.

Without `--game`, `--all`, or `--list`, a folder import asks which game in
a terminal, with an "All N games" choice. Without a terminal it stops and
says which flag to pass.

```
$ wineo import roms ~/Games/roms/gamecube --emulator dolphin --system gamecube --all --yes
Emulator: Dolphin (dolphin-emu)
NAME                  TITLE                 SYSTEM    FILE
metroid-prime         Metroid Prime         gamecube  Metroid Prime (USA).rvz
super-mario-sunshine  Super Mario Sunshine  gamecube  Super Mario Sunshine (USA).iso
Imported metroid-prime.
Imported super-mario-sunshine.
Imported 2 of 2 games.
```

Each game gets its system as a tag, like `gamecube`, when the system is
known. Desktop entries are off, as in the other batch imports; turn one
on afterwards with `wineo setup NAME`.
Emulators that take a fullscreen flag start in fullscreen; `--no-fullscreen`
leaves it out. `--art` fetches art from SteamGridDB, using the top match for
each title, when a key is set (see [Artwork](art.md)).

## Systems

Some file types don't say which system a game is for: Dolphin plays
`.iso`, `.rvz`, `.gcz`, `.ciso`, `.wia`, `.elf`, and `.dol` for both
GameCube and Wii. For those, `--system gamecube` or `--system wii` picks
one for the whole import; a terminal asks instead. Files that do tell, like
`.gcm` or `.wbfs`, keep their own system. `--system` must be one the
emulator plays.

## RetroArch

RetroArch needs a core. wineo offers the installed cores that play the
files in the folder; `--core` picks one by its id (`snes9x`), its name, its
file name, or the path to its `.so`. With one matching core, it's used.
With several, a terminal asks and otherwise the import stops and lists
them. See [RetroArch](native.md#retroarch) for where cores are found, how
games get their systems, and how wineo makes RetroArch quit when the game
closes.

## Custom emulators

Any other emulator works with `--custom`:

```sh
wineo import roms ~/Games/roms/switch --custom ~/Apps/eden.AppImage \
    --args "-f -g {rom}" --ext nsp,xci --system switch --all
```

- `--custom EXE` is the command, a path or a name on `PATH`.
- `--args` is required: one string, split like a shell would, with
  `{rom}` replaced by the game's path, as a whole argument or inside one.
  Quote a `#` that starts an argument, like `'#2'`.
- `--ext` is required: the file types to import, comma-separated.
- `--system` is optional and becomes the games' system and tag.

The games' `emulator` is `custom`, and their Source in the desktop app is
"Emulated". `--custom` can't be combined with `--emulator` or `--core`.

## By hand

An emulated game is a native environment, so it can also be written by
hand in `~/.config/wineo/environments/NAME.toml`:

```toml
schema_version = 1
kind = "native"
title = "Metroid Prime"
tags = ["gamecube"]

[emulation]
emulator = "dolphin"
system = "gamecube"
rom = "~/Games/roms/gamecube/Metroid Prime (USA).rvz"

[targets.metroid-prime]
exe = "dolphin-emu"
args = ["-b", "-e", "/home/me/Games/roms/gamecube/Metroid Prime (USA).rvz"]
default = true
```

The target is what runs; nothing rebuilds it from `[emulation]`. See
[Emulated games](native.md#emulated-games) for the table's keys and what
`validate` checks.

## Portable folders

Emulated games can't be imported into a [portable folder](portable.md),
since their targets hold full paths:
`Emulated games can't be imported into a portable folder.`

## JSON

`--list` and `--emulators` take `--json`. See
[JSON output](json-output.md#emulated-games).
