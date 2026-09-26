# Native Linux games

A native environment launches a Linux binary or script directly, with no
prefix and no runner. It shows up in `list`, `status`, desktop entries, and
art next to Wine environments.

Only what is specific to Wine or Proton is refused for a native environment.
Everything else, like gamemode, `[env]`, targets, art, and desktop entries,
works the same for both kinds.

## Creating one

```
$ wineo new -n rimworld --native --exe ~/Games/RimWorld/start.sh -d
Created native environment rimworld with target rimworld
Added a desktop entry for rimworld in rimworld, with no icon
```

`--native` makes the environment native. `--exe` adds the program to launch
as the default target, named after the environment, and `-d`/`--desktop`
gives that target a desktop entry. The exe is checked before anything is
written, so a bad one creates nothing. `--art` works as usual, while
`--runner`, `--arch`, `--dir`, `--template`, and `--install-exe` are
refused.

Without `--exe`, the environment starts with no targets:

```
$ wineo new -n rimworld --native
Created native environment rimworld
Add a target with: wineo setup rimworld
```

Guided `wineo new` asks first, unless `--native` already answered it:

```
What kind of environment?
  1) Windows program (Wine or Proton)
  2) Native Linux program

Press Enter to pick 1, or pick a number. Ctrl+C to cancel.
Choice:
```

The native branch asks for the name, the executable, and art, then the
target's name and desktop entry. It skips location, runner, arch, and
installer. Esc at the Wine branch's template question goes back to this one.

A config written by hand works right away, since there's no prefix to build.
Start from [`native-environment.toml.example`](native-environment.toml.example)
(installed to `/usr/share/doc/wineo/native-environment.toml.example` by the
Arch package):

```toml
schema_version = 1
kind = "native"
gamemode = true

[targets.game]
exe = "~/Games/RimWorld/start.sh"
default = true
desktop = true
name = "RimWorld"
```

`kind = "native"` refuses the Wine-only keys `prefix`, `runner`, `arch`,
`gameid`, `store`, `dxvk`, and `vkd3d`, each with its own line:

```
native environment dusklight can't set runner
```

A config without `kind` is a Wine environment, so existing configs are
unchanged. The kind can't be changed later with `set`; make a new environment
instead.

## What `exe` can be

| `exe` | Meaning |
|---|---|
| `/usr/bin/dusklight`, `~/Games/RimWorld/start.sh` | That file. It must exist and be executable. |
| `dusklight` (no `/`) | Looked up on `PATH` at launch |
| `C:\...`, or anything ending in `.exe`, `.msi`, `.bat`, or `.lnk` | Refused as a Windows program |
| A relative path like `bin/game` | Refused, except in a [portable folder](portable.md), where it resolves against the folder |

The file runs directly, not through `sh`, so a script needs its shebang line
and execute bit:

```
~/Games/Demo/start.sh is not executable. Run chmod +x on it.
```

A bare name is looked up on the `PATH` the game gets: the global `[env]`, the
environment's `[env]`, then `-e`, falling back to wineo's own `PATH`. Values
in `[env]` aren't expanded, so write `PATH` entries as absolute paths.

`cwd` defaults to the exe's own directory. For a bare name found on `PATH`,
it defaults to your home directory instead.

The exe is checked when `new`, `set`, `setup`, or an import writes it, and
again at launch. Reading a config never checks it, so `list` and `status`
keep working while a game's drive is unmounted.

## Launching and stopping

`start`, `run`, `restart`, `stop`, and `logs` work as they do for Wine. The
command is the exe and its arguments, wrapped in `gamemoderun` when gamemode
is on. The global `[env]`, the environment's `[env]`, and `-e` apply, and no
`WINE*` variables are added. `WINEPREFIX` is yours to set in `[env]` or `-e`
here, since wineo doesn't manage it for native environments.

The game runs in its own process group, and the run lasts until the whole
group is gone. A launcher script that starts the game and exits, like an
AppImage wrapper or GOG's `start.sh`, is still tracked:

```
$ wineo status rimworld
rimworld: running since 2026-09-25 20:43:05 (2 min ago)
  target    game (~/Games/RimWorld/start.sh)
  kind      native
  pid       702760, exited 1 min ago with code 0
            other processes are keeping rimworld running
```

`stop` sends SIGTERM to the group, waits `stop_timeout`, then sends SIGKILL.
A run whose supervisor died is cleaned up by `stop` the same way.

A program that leaves its process group, with `setsid` or by asking an
already running launcher like Steam to start it, isn't tracked after it
leaves.

## Other commands

| Command | Native behavior |
|---|---|
| `set` | `gamemode`, `env.*`, and `targets.*` work. Wine-only keys and `kind` are refused. |
| `setup` | Skips the prefix and program scan. Adding a target goes straight to entering a path. |
| `desktop`, `art` | Work. The icon comes from art only, and an entry with no art uses the theme's generic icon. |
| `clone` | Copies the config and art under the new name, with desktop entries off |
| `portable copy` | Copies the config and art. Absolute exe paths stay host paths and are listed in the review. |
| `rm` | Works. `--delete-prefix` is refused. |
| `exec`, `winetricks`, `programs`, `snapshot` | Refused: `dusklight is a native environment, so it has no prefix` |
| `templates`, `import` of a prefix | Wine only |

Desktop entries get `Game;` as their category when gamemode is on, and
`Utility;Software;` otherwise.

`status --json` has `"kind": "native"`, with `runner` and `prefix` set to
`null`. See [JSON output](json-output.md).

## Heroic

Native games sideloaded into Heroic import as native environments. See
[Native Heroic games](downloads-and-imports.md#native-heroic-games).
