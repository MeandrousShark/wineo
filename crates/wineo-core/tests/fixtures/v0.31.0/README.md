# v0.31.0 fixture set

Written by the real v0.31.0 release (`wineo` built from the v0.31.0 git tag) with
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

## Contents

- Global config: `portable/config/config.toml`, keys: schema_version, stop_timeout, log_limit_mb, gamemode, wayland, mangohud, external_runners, steam, [defaults], arch, games_root, [env], [art], fetch, [gog], language
- Environments in `portable/config/environments`: lazy (wine), plain (wine), templated (wine), tool (native)
- Templates in `portable/config/templates` (hand-written): full, mygame
- Art, under `portable/`: art/plain/art.toml
- Snapshots, under `portable/prefixes/.wineo-snapshots`: plain/first
- Playtime logs, under `portable/stats`: lazy.jsonl, tool.jsonl
- Trash entries under `portable/data/trash` (names in the manifests): doomed (prefix left in place); doomed-prefix (prefix in the trash);
- XDG home environments (`xdg/.config/wineo/environments`): desktop (wine), external (wine), steam-game (proxied)
- XDG home global config: `xdg/.config/wineo/config.toml`, only so external runners count.
- Desktop entries under `xdg/.local/share/applications/`.

## Commands run, in order

`$ROOT` is the portable folder, `$HOME` the XDG home. Lines marked
`(not wineo)` are files made by the script, not by the release.

```
wineo --portable $ROOT set --global stop_timeout=20 log_limit_mb=25 gamemode=true defaults.arch=win32 env.DXVK_HUD=fps wayland=true mangohud=false external_runners=true art.fetch=icon,grid steam=native gog.language=de-DE defaults.games_root=~/Spiele
(not wineo) wrote $ROOT/config/templates/mygame.toml
(not wineo) wrote $ROOT/config/templates/full.toml
wineo --portable $ROOT templates
wineo --portable $ROOT new -n plain
wineo --portable $ROOT new -n templated -t mygame --arch win64
wineo --portable $ROOT new -n lazy --arch win64
(not wineo) touched $ROOT/prefixes/plain/drive_c/Games/Plain/Plain.exe
wineo --portable $ROOT set plain 'targets.main.exe=C:\Games\Plain\Plain.exe' targets.main.default=true 'targets.main.args=["-windowed"]' targets.main.gamemode=false 'targets.main.cwd=C:\Games\Plain' env.PLAIN=1
(not wineo) touched $ROOT/prefixes/templated/drive_c/Games/Templated/Templated.exe
(not wineo) touched $ROOT/prefixes/templated/drive_c/Games/Templated/Editor.exe
wineo --portable $ROOT set templated 'targets.game.exe=C:\Games\Templated\Templated.exe' targets.game.default=true 'targets.editor.exe=C:\Games\Templated\Editor.exe'
(not wineo) touched $ROOT/prefixes/lazy/drive_c/Games/Lazy/Lazy.exe
wineo --portable $ROOT set lazy 'targets.main.exe=C:\Games\Lazy\Lazy.exe' targets.main.default=true 'tags=["game"]' 'winetricks=["vcrun2022", "corefonts"]' wayland=true mangohud=true gamescope.width=1280 gamescope.height=720 gamescope.fps_limit=60 targets.main.mangohud=false
wineo --portable $ROOT set lazy hooks.before=true hooks.after=true targets.main.hooks=false
(not wineo) wrote $ROOT/games/tool.sh
wineo --portable $ROOT new -n tool --native --exe $ROOT/games/tool.sh
wineo --portable $ROOT set tool 'targets.tool.args=["--fast"]'
(not wineo) wrote a 1x1 PNG outside the root to use as art
wineo --portable $ROOT art set plain grid $SCRATCH/grid.png
wineo --portable $ROOT art set plain.main icon $SCRATCH/grid.png
wineo --portable $ROOT snapshot create plain first
wineo --portable $ROOT start lazy
wineo --portable $ROOT stats
wineo --portable $ROOT start tool
wineo --portable $ROOT new -n doomed --arch win64
wineo --portable $ROOT rm doomed
wineo --portable $ROOT new -n doomed-prefix --arch win64
wineo --portable $ROOT snapshot create doomed-prefix kept
wineo --portable $ROOT rm doomed-prefix --delete-prefix
wineo new -n desktop --dir '~/prefixes/desktop' --arch win64   (XDG, HOME=$HOME)
(not wineo) touched $HOME/prefixes/desktop/drive_c/Games/Desktop/Desktop.exe
wineo set desktop 'targets.main.exe=C:\Games\Desktop\Desktop.exe' targets.main.default=true targets.main.desktop=true 'targets.main.name=Desktop Game'   (XDG, HOME=$HOME)
(not wineo) wrote a fake Proton build under .local/share/Steam/compatibilitytools.d/GE-Proton-fixture
wineo set --global external_runners=true   (XDG, HOME=$HOME)
wineo new -n external --runner GE-Proton-fixture --dir '~/prefixes/external'   (XDG, HOME=$HOME)
wineo set external gameid=umu-12345 store=gog   (XDG, HOME=$HOME)
(not wineo) wrote a fake Steam library with Hades II (1086940) and its cached poster
wineo import steam --game 1086940 -n steam-game   (XDG, HOME=$HOME)
```

## Hand-written files

- `config/templates/*.toml`: no release has a command that writes a template,
  so they are written in the shape that release documents. The release
  itself lists them with `wineo templates`, and `mygame` builds environments.

## Left out

- GOG-tracked environment (`gog_id`): every way to make one needs a GOG login and the network.
- `heroic_id`: only a Heroic import writes it, and that needs a Heroic install.
- `dxvk` and `vkd3d` on an environment: setting either installs a build, and the release can only download one. The `full` template carries both keys instead.
- A prefix with real contents: prefixes hold only what the fake wine and umu wrote, plus empty fake program files.
