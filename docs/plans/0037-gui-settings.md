Status: approved

# 0037: GUI settings

## Goal

Edit an environment's settings and targets, and the global config, from
`wineo-gui`. This covers what `wineo set` and the target half of
`wineo setup` do today, with real controls for every key instead of
`KEY=VALUE` text.

It's the first of the GUI parity plans:

- 0037: settings (this plan)
- 0038: new, import (Heroic too), clone
- 0039: rm, trash, restore, prune, validate
- 0040: snapshots, portable copy, art, download, exec, winetricks

`completions` and `run` don't apply to a GUI. `list`, `status`, `stats`,
and `logs` are already the library and panel. `runners`, `dxvk`, `vkd3d`,
and `templates` become choices in the settings page and the 0038 create
flow, not screens of their own.

## Settled by the maintainer

- Parity means everything the CLI can do, not a screen per command.
- Settings use real controls: switches and choices for the keys that have
  them, KEY and VALUE rows for `env`.
- Settings come before create and import. 0038 takes those.
- Inherited on/off settings are switches that show the value in effect,
  with a reset button once overridden. No yes/no/default dropdowns
  (decision 1).

## Existing foundation

- `settings::check(paths, global, name, pairs, unset)` holds the rules
  `wineo set` enforces and applies DXVK and vkd3d changes. `settings::update`
  writes `KEY=VALUE` pairs through `toml_edit`, keeping comments and
  formatting, and refuses anything that fails validation.
- `settings::changes_the_launch` and `settings::changed_runner` say when a
  change needs the environment lock and when to warn about an unmigrated
  prefix.
- The sequence around them lives in the CLI (`commands/set.rs`): the kind
  refusal, the `Configure` lock over the old and new prefix,
  `Runtime::check_resolved`, `desktop::apply` afterwards, and the "changes
  apply the next time it starts" note.
- `Runners::new(paths, global)` lists runners and resolves aliases. The
  installed DXVK and vkd3d-proton builds come from the same code as
  `wineo dxvk` and `wineo vkd3d`.
- `programs::scan(prefix)` lists what's installed in a prefix, which
  `setup` picks targets from.
- The GUI's worker runs blocking core calls off the UI thread, and the
  library refreshes every 2 seconds.

## Design

### Core: one call for a settings change

Move `commands/set.rs`'s sequence into core as
`settings::apply(paths, file, pairs, unset) -> Applied`. `Applied` holds the
component changes, the unset keys that weren't set, whether the runner
changed, whether it's running, and any desktop entry failure. The CLI's
`set` keeps its argument parsing and its wording, and prints from
`Applied`. The GUI calls the same function, so both frontends lock, check,
write, and reconcile desktop entries the same way.

The GUI builds `pairs` and `unset` from its controls: a reset button or an
emptied field unsets the key, everything else is written with the value
TOML expects.

### Where it lives

A Settings button in the detail panel opens the environment's page. A gear
in the top bar opens the global page. See decision 2.

### Environment page

Sections, top to bottom. Native environments hide what the native example
config refuses (prefix, runner, arch, gameid, store, dxvk, vkd3d, wayland,
winetricks).

- **Runner:** runner (installed runners plus aliases like
  `GE-Proton-Latest`). Arch is shown read-only, since core refuses changing
  it once the prefix exists. Choosing a runner of the other kind is refused
  when picked, with "Can't switch between Wine and Proton runners. Make a
  new environment instead." (maintainer's pick, no CLI command in the GUI).
- **Launch:** gamemode, mangohud, and wayland as switches with a reset
  button once overridden (decision 1). Gamescope has no global setting, so
  here it's a plain switch with no reset (maintainer's pick): on writes the
  table (an empty one runs plain `gamescope --`), off removes it. The page
  remembers the fields until it's left, so flipping back on restores them.
  Gamescope on shows width, height, output width and height, filter,
  scaler, window, FPS limits, grab cursor, and extra args as chips. Empty
  fields stay unset.
- **Components (Wine only):** DXVK and vkd3d-proton, each None, Latest, or
  an installed build. vkd3d is disabled until DXVK is set. Both are
  disabled on Proton with the reason.
- **Winetricks (Wine only)** and **Tags:** chips with an add field, checked
  for shape as you type (no spaces or leading `-` for verbs, name
  characters for tags, no duplicates).
- **Environment variables:** KEY and VALUE rows, a + row, and an x per row.
  `WINEPREFIX` is refused on the row.
- **Targets:** one collapsible card per target (decision 4).
- **Advanced:** prefix path, gameid and store (Proton only), and heroic_id
  shown read-only. `kind` is shown, never editable.

### Global page

- **Launch defaults:** gamemode, mangohud, wayland. Unset globally means
  off, so these are plain switches.
- **New environments:** default runner and arch, and the prefix, runner,
  DXVK, and vkd3d roots as path fields.
- **Runs:** stop timeout (seconds) and log limit (MB) as number fields.
- **Art:** independent Icon, Grid, Hero, and Logo switches for what `new`
  and `import` fetch. All off disables automatic fetching.
- **Environment variables:** same rows as the environment page.

### Saving

Edits collect in the page, and a bar at the bottom shows how many changes
there are with Save and Revert (decision 3). Leaving with unsaved changes
asks to Discard or Keep editing.

- Save runs `settings::apply` on the worker. A lock or check failure shows
  in the bar and keeps the edits, so nothing's lost.
- A change that needs the lock (prefix, runner, DXVK, vkd3d) on a running
  environment is refused the same way `set` refuses it. Anything else saves,
  and the bar says it applies the next time it starts.
- A runner change shows the CLI's unmigrated-prefix warning after saving.
- DXVK and vkd3d installs can take a moment, so Save shows a spinner and
  the bar reports the component changes when done.
- The page reloads from disk after saving, so it always shows what was
  written. If the file changes on disk while the page is open with no edits,
  it reloads quietly. With edits, the bar says the file changed and Save
  becomes Reload.

### Configs with errors

An environment whose config can't be read has no controls to show. Its
page shows the error and an Open config file button (`xdg-open` on the
TOML), and reloads when the file changes.

### Keyboard

Tab and Shift+Tab move between fields, Ctrl+S saves, and Esc leaves the page
(asking first when there are unsaved changes).

## Decisions for the maintainer

The maintainer picked switches with reset for decision 1 and accepted the
other recommendations (2026-09-28).

### 1. Showing inherited launch options

gamemode, mangohud, and wayland have three states on an environment (on,
off, or unset, which follows the global config), and targets add a fourth
level.

**Settled:** option A. Each is a switch showing the value in effect, with a
label saying where it comes from. Flipping it overrides, and a reset button
shows up only on overridden rows, which unsets the key. An environment that
hasn't overridden a setting keeps following the global config, so changing
the global switch changes every such environment's next launch. The
maintainer turned down dropdowns for yes, no, and default: a switch is one
click and reads at a glance.

```
option A: switch plus reset (settled)      option B: flipping back = default
Gamemode   (==o)  global                   Gamemode   (==o)  global
MangoHud   (o==)  set here  [reset]        MangoHud   (o==)  set here
Wayland    (==o)  set here  [reset]        (flipping MangoHud back on drops
                                            the override)
```

B couldn't pin a value that matches global, and would have to guess at Save
whether a hand-written `gamemode = true` equal to global means unset.

- On a target, the label says `environment` or `global`, whichever the
  value falls back to, and reset unsets the target's key.
- On a target, gamescope follows the same rule (the environment's plain
  switch is the one exception, see Launch above). A target's table replaces the
  environment's whole table, so editing a field on a target that inherits
  copies the environment's fields into the target first. Switching it off on
  a target writes `gamescope = false`.
- An existing key counts as overridden even when it matches what it would
  inherit, so a hand-written value keeps its reset button and nothing is
  unset behind the maintainer's back.

### 2. Where the page opens

**Settled:** option A, a full page that replaces the library, with a
Back button. The 420px panel is too narrow for targets and gamescope, and a
second window is one more thing to manage.

```
option A: full page                        option B: wider panel
+----------------------------------------+ +---------------+----------------+
| < Back   hades settings                | | [grid]  [grid]| < hades        |
|----------------------------------------| | [grid]  [grid]| Runner  [...v] |
| Runner                                 | |               | Gamemode [...v]|
|   Runner    [ GE-Proton-Latest v ]     | |               | ...            |
|   Arch      [ win64            v ]     | |               | Targets        |
| Launch                                 | |               |  > game        |
|   Gamemode  (==o)  global              | |               |                |
| ...                                    | |               |                |
|----------------------------------------| |               |----------------|
| 2 changes            [Revert] [Save]   | |               | [Revert][Save] |
+----------------------------------------+ +---------------+----------------+
```

### 3. Save model

**Settled:** option A, Save and Revert. A runner or DXVK change can
install components or be refused, and saving each toggle as it's flipped
would make those failures show up mid-edit, one key at a time.

```
option A: save bar                         option B: live
| 2 changes            [Revert] [Save] |   (each change is written as soon
                                           as a control changes)
```

### 4. Adding and editing targets

A target card has exe, args (one per row), working dir, name, the default
marker, desktop entry, and its own gamemode, mangohud, wayland, and
gamescope switches (decision 1).

**Settled:** Add target offers the installed programs from
`programs::scan`, like `setup`, plus a typed path for anything the scan
misses. No file chooser dialog in this plan, since it needs a portal
dependency; add it with 0038, where picking an installer needs one anyway.
Targets can't be renamed here, since art under `art/<env>/<target>/` and
desktop entries follow the name. Remove shows which art and desktop entry
go with it, like `setup` does.

```
Targets                                         [+ Add target]
 v game (default)
     Exe        C:\Program Files\Hades\Hades.exe
     Args       [-nosplash] [+]
     Working dir  (the exe's own)
     Desktop entry  [x]   Name  Hades
     Gamemode   (==o)  environment
     Gamescope  (o==)  set here  [reset]
                                               [Remove target]
 > server
```

## Implementation sequence

1. **Core `settings::apply`.** Move the sequence out of `commands/set.rs`,
   keep the CLI's output byte for byte, and cover it with core tests. The
   existing `set` integration tests must pass unchanged.
2. **Page shell.** The page, Back, the dirty tracking, Save and Revert, the
   leave prompt, file change reloads, and the error page with Open config
   file. Runner and Launch sections only.
3. **Remaining environment sections.** Components, winetricks, tags, env
   rows, Advanced, and native hiding.
4. **Targets.** Cards, add from programs or a path, remove, default, and
   per-target overrides.
5. **Global page.**
6. **Docs.** `docs/gui.md`, features.md, and `docs/core-api.md` for
   `settings::apply`.

## Testing

- Core tests for `settings::apply`: locks against a running environment,
  kind refusal, runner kind mismatch, desktop reconcile, and the unset
  report.
- GUI `update` tests without a window: controls to `pairs` and `unset`
  (reset unsets, empty gamescope fields stay unset, removed env rows
  unset), dirty tracking, revert, the leave prompt, and reload on a file
  change.
- A round trip test: load a commented config, change one control, save, and
  check that only that line changed.

## Verification progress (2026-09-29)

All six implementation steps are present. Maintainer acceptance testing is
pending, so the plan remains approved.

- Formatting, Clippy with warnings denied, and the CLI and GUI release builds pass.
- The frozen workspace test gate passes: 1,482 passed, 16 ignored.
- Isolated X11 GUI checks cover global settings, saving with comments preserved,
  reverting, and the unsaved-changes prompt.
- Settings shortcuts now bypass the absent library search widget. Regression
  tests cover both settings pages; GUI checks confirm Ctrl+S with and without
  a focused field, Tab navigation, and Escape to blur then return to the library.

## Maintainer feedback (2026-09-29)

Approved during acceptance testing: use the same middle scroll speed on library
and settings pages, show resolved default root paths when unset, and use four
independent art-fetch switches with all off allowed. Existing all/icon configs
remain compatible; custom selections are stored as an array of art kinds.
Arrow-key trouble was caused by stuck Right Shift in Moonlight.

Feedback verification: formatting, Clippy, and both release builds pass.
The frozen workspace gate passes with 1,486 tests passed and 16 ignored.
GUI testing confirms default roots are visible without creating overrides,
custom art combinations save with comments preserved, and all-off saves as [].
All 16 art selections are covered by a settings-to-config round-trip test.

Runner-label follow-up: both settings pages now shorten long names by measured
text width, preserving the beginning and suffix, and show full names on hover.
The saved runner identity is unchanged. All 135 GUI tests, formatting, Clippy,
and the release build pass. Isolated GUI checks cover both pages, the open menu,
and the tooltip.
