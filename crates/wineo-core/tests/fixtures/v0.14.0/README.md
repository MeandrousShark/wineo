# v0.14.0 fixture set

Written by the real v0.14.0 release (`wineo` built from the v0.14.0 git tag) with
`scripts/generate-upgrade-fixtures.sh`, against fake wine, umu, and winetricks
programs. Do not edit by hand; rerun the script. Timestamps, the trash entry
id, and snapshot times differ from run to run.

## Layout

- `./` is a portable folder: `wineo --portable <copy of this directory>`.
- `xdg/` is a HOME for what a portable folder cannot hold: desktop entries are
  refused there, Steam games cannot be imported into one, and
  `external_runners` does nothing in one. Copy it to a temporary HOME and
  point `XDG_CONFIG_HOME` and `XDG_DATA_HOME` at `.config` and `.local/share`
  under it.
- Prefixes hold only `system.reg`, `dosdevices/c:`, `drive_c/` (with an empty
  `windows/win.ini`, since git keeps no empty directories), and empty fake
  program files, plus `winetricks.log` where winetricks ran. Prefix paths in
  configs are relative to the folder or start with `~`.
- The trash entry directory is named by time and a random suffix; glob
  `data/trash/*/`. Its manifest records a device and inode that never match a
  copy.

## Contents

- Global config: `config/config.toml`, keys: schema_version, stop_timeout, log_limit_mb, gamemode, wayland, mangohud, [defaults], arch, [env]
- Environments (portable): lazy (wine), plain (wine), templated (wine), tool (native)
- Templates (hand-written): full, mygame
- Art: art/plain/art.toml
- Snapshots: plain/first
- Playtime logs: lazy.jsonl, tool.jsonl
- Trash entries (names in the manifests): doomed (prefix left in place); doomed-prefix (prefix in the trash);
- XDG home environments (`xdg/.config/wineo/environments`): desktop (wine)
- Desktop entries under `xdg/.local/share/applications/`.

## Commands run, in order

`$ROOT` is the portable folder, `$HOME` the XDG home. Lines marked
`(not wineo)` are files made by the script, not by the release.

```
wineo --portable $ROOT set --global stop_timeout=20 log_limit_mb=25 gamemode=true defaults.arch=win32 env.DXVK_HUD=fps wayland=true mangohud=false
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
```

## Hand-written files

- `config/templates/*.toml`: no release has a command that writes a template,
  so they are written in the shape that release documents. The release
  itself lists them with `wineo templates`, and `mygame` builds environments.

## Left out

- External runners: arrive in v0.24.0.
- Hooks and Steam environments: arrive in v0.28.0.
- GOG settings and environments: arrive in v0.31.0.
- Global `[art] fetch`: in the file format, but `set --global` can't write it until v0.24.0.
- `heroic_id`: only a Heroic import writes it, and that needs a Heroic install.
- `dxvk` and `vkd3d` on an environment: setting either installs a build, and the release can only download one. The `full` template carries both keys instead.
- A prefix with real contents: prefixes hold only what the fake wine and umu wrote, plus empty fake program files.
