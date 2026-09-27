Status: draft

# 0031: Wine Wayland, gamescope, and MangoHud

## Goal

Make Wine's native Wayland driver, gamescope, and MangoHud launch settings,
instead of hand-written `[env]` entries or wrapper scripts, and carry them
over from Heroic imports. This is GUI entry criterion 5. The GUI will show
these as toggles and a gamescope form, so the config shape settles here.

## Existing foundation

- `gamemode` is the model: an `Option<bool>` on the global config, the
  environment, each target, and templates. `launch::gamemode` picks the most
  specific setting, and `Launch` stores the resolved `gamemoderun` path so a
  detached run can't find a different one on a different PATH. A missing
  binary is `Error::GamemodeMissing` at launch and `gamemode_missing` in
  validate.
- `runner::wrapped` puts one wrapper in front of wine, `umu-run`, or a native
  exe. `exec`, `winetricks`, and installers never get gamemode or `GAMEID`.
- Heroic import lists `gamescope`, `showMangohud`, and `enableWineWayland` as
  unsupported today, and `enableHDR` as runner-specific.
- Heroic's `launcher.ts` (read 2026-09-27):
  - Wrapper order is `gamescope [opts] --`, then custom wrappers, then
    `mangohud --dlsym`, then `gamemoderun`, then `umu-run`. With gamescope
    on, MangoHud becomes gamescope's `--mangoapp` flag instead of a wrapper.
  - Gamescope only runs when `enableUpscaling` or `enableLimiter` is on. Its
    settings are `gameWidth`/`gameHeight` (`-w`/`-h`), `upscaleWidth`/
    `upscaleHeight` (`-W`/`-H`), `upscaleMethod` (`fsr`, `nis`, `integer`,
    `stretch`), `windowType` (`fullscreen` `-f`, `borderless` `-b`),
    `fpsLimiter`/`fpsLimiterNoFocus` (`-r`/`-o`), `enableForceGrabCursor`,
    and `additionalOptions` (shell-split).
  - `enableWineWayland` sets `PROTON_ENABLE_WAYLAND=1` on Proton and
    `DISPLAY=` on Wine. `enableHDR` adds `PROTON_ENABLE_HDR=1` or
    `DXVK_HDR=1`, and only with Wayland on. Native games drop all of these
    except gamescope and MangoHud.
- Support can be read off the runner: a Wine build with Wayland ships
  `lib/wine/x86_64-unix/winewayland.so`, and a Proton build that honors the
  variable has `PROTON_ENABLE_WAYLAND` in its `proton` script. GE-Proton11-7,
  proton-cachyos 11.0, and system Wine 11.18 all pass both.
- Current gamescope (3.16 here) takes `-F` for the filter (`linear`,
  `nearest`, `fsr`, `nis`, `pixel`, `sgsr`) and `-S` for the scaler (`auto`,
  `integer`, `fit`, `fill`, `stretch`). Heroic still has a branch for the
  pre-3.12 flags.
- A new key needs no `schema_version` bump, for the same reason `kind`
  didn't: an older wineo refuses the file with `unknown field` instead of
  misreading it.

## Decisions (open)

### 1. Gamescope config shape

**A. Named fields plus `args` (recommended)**

```toml
[gamescope]
width = 1920           # -w, what the game renders at
height = 1080          # -h
output_width = 2560    # -W
output_height = 1440   # -H
filter = "fsr"         # -F
scaler = "fit"         # -S
window = "fullscreen"  # -f, or "borderless" for -b
fps_limit = 60         # -r
fps_limit_unfocused = 10  # -o
grab_cursor = true     # --force-grab-cursor
args = ["--hdr-enabled"]
```

```sh
wineo set palworld gamescope.width=1920 gamescope.window=fullscreen
```

Every field is optional; an empty `[gamescope]` runs plain `gamescope --`.
`filter`, `scaler`, and `window` are checked at load, so a typo fails like
any other bad value. `args` covers every flag without a field.

**B. Just the arguments**

```toml
gamescope = ["-w", "1920", "-h", "1080", "-W", "2560", "-H", "1440", "-F", "fsr", "-f"]
```

```sh
wineo set palworld gamescope='["-w", "1920", "-h", "1080", "-f"]'
```

Less to build and never behind gamescope, but nothing is checked until
gamescope runs, and the GUI either parses flags back out or shows a raw
text box.

A gives the GUI a form and catches typos, and `args` keeps B's escape hatch.

### 2. Where each setting lives

**Recommended:**

- `wayland` and `mangohud` are booleans on the global config, the
  environment, targets, and templates, and the most specific one wins,
  exactly like `gamemode`.
- `gamescope` lives on the environment, targets, and templates, not the
  global config. A target's table replaces the environment's whole, with no
  field-by-field merging, and `gamescope = false` on a target turns it off
  (say, for a launcher target). What's in the file is what runs.
- `wayland` is a Wine-only key. A native environment refuses it at either
  level, like `dxvk`. Gamescope and MangoHud work for native games.

The alternative is merging gamescope fields across levels, so a global
`output_width` could hold your monitor size. With `window = "fullscreen"`
gamescope fills the monitor anyway, so that buys little for a harder rule.

### 3. When Wayland can't be used

**Recommended:**

- A runner without Wayland support is an error at launch, like missing
  gamemode, and a `wayland_unsupported` error in validate:
  `wayland is on for palworld, but proton-9.0 doesn't support it`.
- Not being in a Wayland session (no `WAYLAND_DISPLAY`) launches on X11
  with one warning: `Not a Wayland session, starting palworld on X11.`
  Logging into an X11 session now and then shouldn't break every game that
  has the setting.

The alternative is refusing in both cases, which is stricter but makes the
setting depend on how you logged in.

### 4. Tools

**Recommended:** none of the three apply to `exec`, `winetricks`, setup, or
installers, same as gamemode. Running winecfg in a gamescope window isn't
useful, and `-e PROTON_ENABLE_WAYLAND=1` still works for testing a tool.

### 5. Heroic HDR

**Recommended:** with `enableWineWayland` and `enableHDR` both on, import
HDR as the same `[env]` entry Heroic sets (`PROTON_ENABLE_HDR=1` on Proton,
`DXVK_HDR=1` on Wine). It's only an environment variable, so a key of its
own can wait until someone asks. HDR without Wayland stays unsupported.

## Launching

- Order matches Heroic: `gamescope [flags] --`, then `mangohud`, then
  `gamemoderun`, then wine, `umu-run`, or the native exe. With gamescope on,
  MangoHud becomes `--mangoapp` instead of a wrapper. Whether `mangohud`
  needs Heroic's `--dlsym` is checked in step 1.
- `Launch` keeps resolved paths for all three binaries, as it does for
  `gamemoderun` today. Missing ones are `gamescope_missing` and
  `mangohud_missing`, at launch and in validate.
- Wayland on Proton sets `PROTON_ENABLE_WAYLAND=1`. On Wine it removes
  `DISPLAY` from the launch, which makes Wine fall back to its Wayland
  driver.
- `status` gains `gamescope`, `mangohud`, and `wayland` lines when on, and
  its JSON gains the same three booleans. Desktop entries get the `Game;`
  category when any of gamemode, gamescope, or MangoHud is on.

## Heroic import

- `enableWineWayland` becomes `wayland = true`, or a warning when the chosen
  runner doesn't support it. Skipped for native games, like Heroic does.
- `showMangohud` becomes `mangohud = true`.
- `gamescope` becomes a `[gamescope]` table only when `enableUpscaling` or
  `enableLimiter` is on, Heroic's own rule. Fields map as listed above,
  `integer` and `stretch` become `scaler`, `fsr` and `nis` become `filter`,
  and `additionalOptions` is shell-split into `args`.
- `enableHDR` per decision 5.
- The review shows the new rows, and Heroic's global defaults feed these
  keys the same way they feed gamemode today. `wrapperOptions` stays
  unsupported.

## Implementation sequence

1. Core: the keys on the global config, environment, targets, and templates;
   gamescope parsing and checks; `wayland` as Wine-only; runner support
   detection; `Launch` resolving and ordering the wrappers; the Wayland
   environment; the new errors.
2. CLI and the rest of core: `set` and `unset` (including
   `gamescope.<field>` and `targets.<name>.gamescope.<field>`), `status` and
   its JSON, desktop categories, the validate codes, and both example
   configs.
3. Heroic import mapping and review rows.
4. Docs: a `docs/launch-options.md` page, `config-files.md`, `native.md`,
   `validate.md`, `json-output.md`, `downloads-and-imports.md`, README, and
   `features.md`.

## Verification and acceptance

- Unit tests for precedence at each level, the target table replacing the
  environment's, `gamescope = false`, bad `filter`/`scaler`/`window` values,
  `wayland` refused on native, and runner support detection against fake
  runner trees.
- Launch tests for the exact command line in every combination, including
  `--mangoapp`, and the Wayland variables on fake Wine and Proton runners.
- Heroic tests against fixture game configs for every mapped field, the
  upscaling/limiter rule, and HDR.
- Ignored real-program tests: a Wine and a Proton target under gamescope,
  ended by `wineo stop`, to confirm gamescope stays in the run's process
  group and doesn't outlive it. Also a launcher that exits while its game
  keeps running, to see whether gamescope closes early on plain Wine (umu
  waits for the prefix, so Proton shouldn't). If it does, wrap the runner
  command so gamescope's child lasts until the prefix is idle.
- The maintainer plays one Proton game with each setting and re-imports a
  Heroic game that has them.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`.

## Scope limits

- No gamescope version detection. Only the current `-F`/`-S` flags.
- No generic wrapper setting. Heroic's `wrapperOptions` stays unsupported;
  pre-launch hooks are their own backlog item.
- No MangoHud config management. `MANGOHUD_CONFIG` goes in `[env]`.
- No guided `new` prompts for these. Templates and `set` cover them.
