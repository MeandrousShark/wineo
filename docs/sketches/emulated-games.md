---
type: proposal
status: done
tags: [emulation, imports]
---
# Emulated games

Add and launch games from emulators like Dolphin, RetroArch, and Cemu,
with playtime, art, and desktop entries. Queued as plan 0068 in
[features.md](../features.md). This page keeps the research.

From the maintainer (2026-10-14), pulled ahead of store imports on
2026-11-15. A native environment can already launch one by hand, since a
target is `exe`, `args`, and `cwd`: `dolphin-emu -b -e GAME`,
`retroarch -L CORE ROM`, `cemu -g GAME`. The feature adds the rest: a
guided flow that finds installed emulators (native or Flatpak) and fills in
their arguments, a RetroArch core picker, and importing a ROM folder per
system through the import grid.

## What wineo has today

- Target `args` are a literal array. There is no placeholder like `{rom}`
  anywhere, so each game needs its own target or its own environment.
- A native run lasts until its whole process group is gone. So playtime
  stops when the emulator's process exits, not when the game does.
- The only `flatpak run` wineo builds is for Steam. Nothing looks for
  emulators.
- A library card is one environment, with its own art, playtime, tags, and
  favorite.

## Emulators on Linux

Checked on 2026-11-15 against each project's source or docs and Flathub's
app metadata. "Exits" means the process ends when the game is stopped with
that command line. Items marked (unchecked) weren't confirmed.

| Emulator | On PATH | Flathub id | Boot a game | Fullscreen | Exits |
|---|---|---|---|---|---|
| Dolphin | `dolphin-emu` | `org.DolphinEmu.dolphin-emu` | `-b -e FILE` | no flag; `-C Dolphin.Display.Fullscreen=True` (unchecked) | yes, with `-b` |
| RetroArch | `retroarch` | `org.libretro.RetroArch` | `-L CORE FILE` | `-f` | no, unless `quit_on_close_content` is set; quitting RetroArch exits |
| Cemu | `cemu`, `Cemu` | `info.cemu.Cemu` | `-g FILE` | `-f` | yes |
| PCSX2 | `pcsx2-qt`, `pcsx2` | `net.pcsx2.PCSX2` | `-batch -- FILE` | `-fullscreen` | yes, with `-batch` |
| DuckStation | `duckstation-qt`, `duckstation-nogui` | `org.duckstation.DuckStation` (end of life on Flathub) | `-batch -- FILE` | `-fullscreen` | yes, with `-batch` |
| PPSSPP | `ppsspp`, `PPSSPPSDL`, `PPSSPPQt` | `org.ppsspp.PPSSPP` | `--pause-menu-exit FILE` | `--fullscreen` | yes, with `--pause-menu-exit` |
| RPCS3 | `rpcs3` | `net.rpcs3.RPCS3` | `--no-gui PATH` | `--fullscreen` (needs `--no-gui`) | yes, with `--no-gui` |
| melonDS | `melonDS`, `melonds` | `net.kuribo64.melonDS` | `FILE` | `-f` | no; closing the window exits |
| mGBA | `mgba-qt`, `mgba` | `io.mgba.mGBA` | `FILE` | `-f` | no; closing the window exits |
| xemu | `xemu` | `app.xemu.xemu` | `-dvd_path FILE` | `-full-screen` | no; closing the window exits |
| Azahar (3DS) | `azahar` | `org.azahar_emu.Azahar` | `FILE` (last argument) | `-f` | no; stopping the game returns to its list |

Switch: upstream Ryujinx is discontinued. The Ryubing fork
(`io.github.ryubing.Ryujinx`) and Eden (`dev.eden_emu.eden`, `-f -g FILE`)
are on Flathub, but the situation is unstable, so no Switch emulator should
be built in by name.

Official AppImages exist for Cemu, PCSX2, DuckStation, PPSSPP, RPCS3,
melonDS (zipped), mGBA, xemu, and Azahar. None was found for Dolphin or
RetroArch.

Separate settings: Dolphin `-u DIR`; RetroArch `--config FILE` and
`--appendconfig FILE`; Cemu `-m MLC`; PCSX2 `-datapath DIR` and
`-gamecfg FILE`; PPSSPP `--config=FILE`; RPCS3 `--config PATH`; xemu
`-config_path FILE`. DuckStation, melonDS, and mGBA have no flag for it.

Ending options with `--` (checked 2026-11-17 in each parser): PCSX2 and
DuckStation take it and treat the rest as the path. Dolphin's optparse
takes it. PPSSPP treats it as an unknown option and shows an error. RPCS3
passes everything after it to the game. Azahar ignores it. xemu,
melonDS, mGBA, and Cemu are unchecked. wineo uses `--` only for PCSX2 and
DuckStation; every ROM path is absolute, so it never starts with `-`.

RPCS3 is single-instance: a second one exits with "Another instance of
RPCS3 is running". No other emulator above was seen handing off to a
running copy.

### What a game is on disk

- Dolphin: gcm, iso, tgc, ciso, gcz, wbfs, wia, rvz, wad, elf, dol.
- Cemu: wud, wux, wua, wuhb, or a title folder with `code/`, `content/`,
  and `meta/`. `-g` takes the title folder or the `.rpx` in its `code/`
  (`TitleInfo::DetectFormat`), and Cemu exits with a game it was started
  with.
- PCSX2: iso, bin, img, mdf, chd, cso, zso, gz (`VMManager::IsDiscFileName`).
  Not cue: its open dialog lists `.cue`, but nothing reads one
  (`CDVD/InputIsoFile.cpp`; issue #4880 is still open).
- DuckStation: cue, chd, iso, pbp, ecm, mds, ccd, m3u, bin, img
  (`System::IsDiscPath`); a `.bin` or `.img` that a `.cue` names isn't a
  game of its own.
- PPSSPP: iso, cso, chd, pbp, or a folder holding `EBOOT.PBP`, passed as
  the folder (`Identify_File` checks for `EBOOT.PBP` inside).
- RPCS3: a disc folder with `PS3_GAME/USRDIR/EBOOT.BIN`, or an iso, passed
  as the top folder (`GetElfPathFromDir` finds the `EBOOT.BIN`).
- melonDS: nds, srl, dsi, ids.
- mGBA: gb, gbc, sgb (Game Boy) and gba, agb, mb (GBA), from `romFilters()`
  in its Qt `utils.cpp`. It also opens `.bin` and `.rom`, and `.zip`,
  `.7z`, and `.elf` in some builds, which are too generic to scan for.
- xemu: xiso images, usually `.iso`.
- Azahar: cci, 3ds, cia, 3dsx, cxi, app, and the compressed zcci, zcia,
  z3dsx, zcxi.
- RetroArch: whatever the core's `.info` file lists.

### RetroArch cores

A core is `NAME_libretro.so`, described by `NAME_libretro.info`: lines of
`key = "value"`, with `supported_extensions = "smc|sfc|swc"`, `systemname`,
`systemid`, `display_name`, `corename`, and `categories = "Emulator"`.

- Arch: cores in `/usr/lib/libretro/`, info in `/usr/share/libretro/info/`.
- Debian and Ubuntu: `/usr/lib/x86_64-linux-gnu/libretro/` and
  `/usr/share/libretro/info/`.
- Fedora: cores in `/usr/lib64/libretro` (info path unchecked).
- Downloaded by the user: `~/.config/retroarch/cores`.
- Flatpak: cores in
  `~/.var/app/org.libretro.RetroArch/config/retroarch/cores/`; info ships
  inside the app, at
  `$(flatpak info --show-location org.libretro.RetroArch)/files/share/libretro/info`
  (derived, unchecked).

- Gentoo (`games-emulation/retroarch` 1.22.2, checked 2026-11-15): the
  package installs no cores and no `/usr/lib64/libretro` or
  `/usr/share/libretro`. Cores from RetroArch's own downloader land in
  `~/.config/retroarch/cores`, and so do the info files: all of them at
  once (360 on 2026-11-15), for cores that aren't installed too, beside a
  `core_info.cache`. So a core is installed when its `.so` is there, not
  when its `.info` is.

Info files seen on that install:

| Core file | `systemid` | `supported_extensions` |
|---|---|---|
| `snes9x_libretro.so` | `super_nes` | `smc\|sfc\|swc\|fig\|bs\|st` |
| `melondsds_libretro.so` | `nds` | `nds\|ids\|dsi` |
| `azahar_libretro.so` | `3ds` | `3ds\|3dsx\|app\|axf\|bcci\|bcxi\|cci\|cxi\|elf\|z3dsx\|zcci\|zcxi` |
| `mgba_libretro.so` | `game_boy_advance` | `gb\|gbc\|gba` |

A core has one `systemid` even when it plays several systems: mGBA's says
`game_boy_advance` for `gb` and `gbc` files too, and Genesis Plus GX's
says `mega_drive` for Master System and Game Gear files. The `database`
key lists each system by name
(`Nintendo - Game Boy|Nintendo - Game Boy Color|Nintendo - Game Boy Advance`).

`-L` takes a full path to a core, or just its file name
(`NAME_libretro.so`), per RetroArch's own help text.

The info files are MIT (`libretro/libretro-core-info`).

### Quitting with the game

Run on 2026-11-17 against RetroArch 1.22.2 (Gentoo) with the mGBA core, the
null video, input, and audio drivers, a throwaway HOME, its own
`--config`, and a blank GBA image (zeros, with `0xEA` at offset 3 and
`0x96` at `0xB2` so mGBA accepts it). Content was closed with
`CLOSE_CONTENT` over the network command interface:

- `quit_on_close_content` is `0` (RetroArch stays open), `1` (it quits,
  exit 0), or `2` (it quits when the content was started from the command
  line, as a launcher starts it; exit 0).
- A setting from `--appendconfig` is saved back into the main config: one
  run with `quit_on_close_content = "1"` appended left a full, existing
  main config at `"1"`, after closing the game or quitting.
- Appending `config_save_on_exit = "false"` as well keeps the main config
  byte-for-byte unchanged, and closing the game still quits.
- A missing appended file is logged (`Failed to append config`) and
  ignored.
- A `|` anywhere in the appended file's path splits it, and the append
  fails.
- A stub main config is rewritten in full on startup, so checks like these
  need a complete config seeded by an earlier run.

From RetroArch's source (master and v1.22.2): saved overrides compare
against the main config plus `--appendconfig`, so appended keys never land
in them. Every normal quit, including the first SIGTERM and closing the
window, honors `config_save_on_exit`. "Save Current Configuration" doesn't:
it writes all live settings, appended ones included, into the main config
(RetroArch's man page says so too). `quit_on_close_content` and its `2`
value came in 1.9.1 (2021, PR #11464).

Earlier notes, from the binary's strings and help text:

- The config key `quit_on_close_content` exists ("Quit on Close Content"
  in the menu). Its values weren't confirmed.
- `--appendconfig=FILE`: "Extra config files are loaded in, and take
  priority over config selected in -c (or default). Multiple configs are
  delimited by '|'." So a path with a `|` in it would be split.
- Whether an appended setting is saved back to the main config on exit
  (`config_save_on_exit`) is unchecked.

## Flatpak

- `flatpak run ID ARGS...` replaces itself with the sandbox and stays until
  the app exits, so waiting on it and its process group works (read from
  the source, not run). The exported `exports/bin/ID` script is the same
  thing.
- `--filesystem=PATH` reads PATH up to the first unescaped `:`, and the
  rest is the mode (`ro`, `rw`, `create`; anything else is ignored with a
  warning). A `:` in the path is written `\:` and a `\` as `\\`, since
  flatpak 0.11.4 (`flatpak-context.c`, `parse_filesystem_flags`). `..` and
  `/` itself are refused.
- What the sandbox can read differs: RetroArch and mGBA have the host
  (home, `/media`, `/run/media`, not `/mnt`); Dolphin, Cemu, PPSSPP, xemu,
  and Azahar have it read-only; melonDS only home; RPCS3 home read-only
  plus `/media` and `/run/media`; PCSX2 and DuckStation nothing. A ROM the
  sandbox can't read needs `--filesystem=DIR` on the `flatpak run` line.

## How others do it

- **ES-DE** (MIT): `es_systems.xml` maps a system to extensions and
  commands with placeholders (`%EMULATOR_DOLPHIN% -b -e %ROM%`), and
  `es_find_rules.xml` finds each emulator: names on PATH first, then fixed
  paths and AppImage globs, then the Flatpak `exports/bin` folders.
- **Lutris** (GPL-3.0): a runner per emulator in Python, per-game YAML, and
  a fallback to `flatpak run` when its own copy is missing. Its libretro
  runner reads the same `.info` files.
- **Playnite** (MIT, Windows): emulator profiles with arguments like
  `-g "{ImagePath}" -f`, platforms, and extensions; a scan folder paired
  with a profile; multi-disc games grouped into one entry.
- **Steam ROM Manager** (GPL-3.0): one JSON preset per system with a glob
  and arguments, writing non-Steam shortcuts.

## Left for later

Per-game emulator settings, achievements (see
[achievements](achievements.md)), watching a ROM folder for new games, and
user-defined emulator files.
