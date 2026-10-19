# Emulated games

Add and launch games from emulators like Dolphin, RetroArch, and Cemu,
with playtime, art, and desktop entries. Backlog row in
[features.md](../features.md): UX, priority 3, depends on 0026. The row
keeps its priority and status; this page keeps the detail.

From the maintainer (2026-10-14), who may pull it earlier. A native
environment can already launch one by hand, since a target is `exe`,
`args`, and `cwd`: `dolphin-emu -b -e GAME`, `retroarch -L CORE ROM`,
`cemu -g GAME`. The feature adds the rest: a guided flow that finds
installed emulators (native or Flatpak) and fills in their arguments, a
RetroArch core picker, and importing a ROM folder per system through the
import grid. Emulators must exit when the game does (Dolphin's `-b`), or
playtime keeps counting.

Open questions: one environment per game or per emulator, Flatpak
emulators under `flatpak run`, and per-game emulator config (Dolphin `-u`,
RetroArch `--config`). Needs a plan.
