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
| Azahar (3DS) | `azahar` | `org.azahar_emu.Azahar` | `FILE` (last argument) | `-f` | no (unchecked) |

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

RPCS3 is single-instance: a second one exits with "Another instance of
RPCS3 is running". No other emulator above was seen handing off to a
running copy.

### What a game is on disk

- Dolphin: gcm, iso, tgc, ciso, gcz, wbfs, wia, rvz, wad, elf, dol.
- Cemu: wud, wux, wua, wuhb, or a title folder with `code/`, `content/`,
  and `meta/` (the `.rpx` is in `code/`).
- PCSX2: iso, bin, mdf, chd, cso, zso, gz.
- DuckStation: cue, chd, iso, pbp, ecm, mds, ccd, m3u; a `.bin` or `.img`
  beside a `.cue` isn't a game of its own.
- PPSSPP: iso, cso, chd, pbp, or a folder holding `EBOOT.PBP`.
- RPCS3: a disc folder with `PS3_GAME/USRDIR/EBOOT.BIN`, or an iso.
- melonDS: nds, srl, dsi, ids. mGBA: gba, gb, gbc, agb (list unchecked).
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

The info files are MIT (`libretro/libretro-core-info`).

## Flatpak

- `flatpak run ID ARGS...` replaces itself with the sandbox and stays until
  the app exits, so waiting on it and its process group works (read from
  the source, not run). The exported `exports/bin/ID` script is the same
  thing.
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
