# Launch options

Four settings change how a target is launched: `gamemode`, `mangohud`,
`wayland`, and `gamescope`. They apply to game launches only (`start`,
`run`, `restart`, and desktop entries). `exec`, `winetricks`, `setup`, and
installers never get them, so winecfg doesn't open under gamescope. Pass
`-e PROTON_ENABLE_WAYLAND=1` to try a tool on Wayland.

## Where they go

`gamemode`, `mangohud`, and `wayland` are on or off, and can be set in the
global config, an environment, a target, or a template. The most specific
setting wins, and unset everywhere means off:

```toml
# ~/.config/wineo/config.toml
mangohud = true

# environments/palworld.toml
wayland = true

[targets.launcher]
exe = 'C:\Game\Launcher.exe'
mangohud = false
```

`gamescope` is a table on an environment, a target, or a template, not the
global config:

```toml
[gamescope]
width = 1920
height = 1080
filter = "fsr"
window = "fullscreen"
```

A target's `[targets.<name>.gamescope]` replaces the environment's whole
table, with no merging of fields, and `gamescope = false` on a target turns
it off for that target. `gamescope = true` is refused, since it says nothing
about how to run it; an empty `[gamescope]` table runs plain `gamescope --`.

From the command line:

```sh
wineo set palworld mangohud=true wayland=true
wineo set palworld gamescope.width=1920 gamescope.height=1080 gamescope.window=fullscreen
wineo set palworld targets.launcher.gamescope=false
wineo set palworld --unset gamescope
```

Unsetting a single field like `gamescope.width` keeps the table, so
gamescope stays on. `--unset gamescope` turns it off.

## Gamescope

Every field is optional:

| Field | Flag | Values |
|---|---|---|
| `width`, `height` | `-w`, `-h` | The resolution the game renders at |
| `output_width`, `output_height` | `-W`, `-H` | The window or screen size |
| `filter` | `-F` | `linear`, `nearest`, `fsr`, `nis`, `pixel`, `sgsr` |
| `scaler` | `-S` | `auto`, `integer`, `fit`, `fill`, `stretch` |
| `window` | `-f`, `-b` | `fullscreen` or `borderless` |
| `fps_limit` | `-r` | Frames per second |
| `fps_limit_unfocused` | `-o` | Frames per second while unfocused |
| `grab_cursor` | `--force-grab-cursor` | `true` or `false` |
| `args` | | Any other flags, passed last, like `["--hdr-enabled"]` |

Values are checked when the config is read, so a typo like
`filter = "bilinear"` fails before anything launches. Only current gamescope
flags are supported; `-F` and `-S` need gamescope 3.12 or newer.

Gamescope closes as soon as the program it started exits, and takes anything
that program handed off to with it. On Wine, wineo keeps gamescope's program
running until the prefix is idle, so a launcher that starts the game and
exits still works. Proton already waits for its prefix. A native launcher
script that starts the game in the background and exits doesn't work under
gamescope; use one that `exec`s the game, or point the target at the game
itself.

On Hyprland, gamescope 3.16's default Wayland backend can drop its window as
soon as a Wine or Proton program opens one. The game keeps running inside
gamescope with nothing on screen. The SDL backend doesn't have this problem:

```sh
wineo set palworld 'gamescope.args=["--backend", "sdl"]'
```

## MangoHud

With `mangohud` on, the launch runs under `mangohud`. Under gamescope,
MangoHud becomes gamescope's `--mangoapp` flag instead, which is what
gamescope recommends, so it needs the `mangoapp` program that ships with
MangoHud. Configure the overlay with `MANGOHUD_CONFIG` in `[env]`.

## Wayland

`wayland` runs the game on Wine's native Wayland driver instead of through
XWayland. It's a Wine-only key, so a native environment refuses it.

- On Proton, wineo sets `PROTON_ENABLE_WAYLAND=1`. The Proton build has to
  read that variable, which GE-Proton and Proton-CachyOS do.
- On Wine, wineo leaves `DISPLAY` out of the launch, so Wine falls back to
  its Wayland driver. The build has to ship `winewayland.so`.

A runner without support is an error at launch:

```
wayland is on for palworld, but proton-9.0 doesn't support it
```

Outside a Wayland session (no `WAYLAND_DISPLAY`), the game starts on X11
instead, with a note, so logging into an X11 session now and then doesn't
break every game that has the setting:

```
$ wineo start palworld
Not a Wayland session, starting palworld on X11.
Started palworld
```

Gamescope hides the session's Wayland display from the game and always
hands it an X display of its own. So with gamescope on too, wineo adds
`--expose-wayland` to gamescope's flags, and on Wine it also runs the game
through `env -u DISPLAY` inside gamescope:

```
gamescope -f --expose-wayland -- env -u DISPLAY wine game.exe
```

## The command line

Wrappers run in the same order Heroic uses:

```
gamescope [flags] -- mangohud --dlsym gamemoderun wine|umu-run|exe [args]
```

Each wrapper is left out when its setting is off, `mangohud --dlsym` is
dropped under gamescope in favor of `--mangoapp`, and Wayland under
gamescope adds what's described [above](#wayland). On Wine, gamescope's
program is `sh -c` running the rest and then `wineserver -w`, which keeps
gamescope open until the prefix is idle and passes the program's exit
status through. The binaries are found
on `PATH` when the launch starts. A missing one fails before anything runs:

```
gamescope is on for `palworld`, but it was not found or is not installed.
Install gamescope, or turn it off with `wineo set palworld gamescope=false`
```

## Status, validate, and desktop entries

`wineo status NAME` shows a line for each setting that's on:

```
  gamemode   on
  gamescope  on
  mangohud   on
  wayland    on
```

`status --json` has `gamescope`, `mangohud`, and `wayland` booleans next to
`gamemode`. [Validate](validate.md) reports a missing gamescope or MangoHud
and a runner without Wayland support. Desktop entries get the `Game;`
category when gamemode, gamescope, or MangoHud is on.

## Heroic import

Importing a game from Heroic carries these settings over; see
[Downloads and imports](downloads-and-imports.md#launch-options).
