Status: draft

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

The GUI builds `pairs` and `unset` from its controls: a key set to Default
is unset, everything else is written with the value TOML expects.

### Where it lives

A Settings button in the detail panel opens the environment's page. A gear
in the top bar opens the global page. See decision 2.

### Environment page

Sections, top to bottom. Native environments hide what the native example
config refuses (prefix, runner, arch, gameid, store, dxvk, vkd3d, wayland,
winetricks).

- **Runner:** runner (installed runners plus aliases like
  `GE-Proton-Latest`), arch (`win64`, `win32`; fixed to win64 on Proton).
  Choosing a runner of the other kind is refused before Save, with the same
  "make a new environment" reason the CLI gives.
- **Launch:** gamemode, mangohud, wayland (decision 1 for how inheriting
  looks). Gamescope is a switch; turning it on shows width, height, output
  width and height, filter, scaler, window, FPS limits, grab cursor, and
  extra args. Empty fields stay unset.
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
- **Art:** what `new` and `import` fetch (all kinds, or the icon only).
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

### 1. Showing inherited launch options

gamemode, mangohud, and wayland have three states on an environment (on,
off, or unset, which follows the global config), and targets add a fourth
level.

**Recommended:** option A, a dropdown whose Default entry says what it
resolves to and where from.

```
option A: dropdown                         option B: switch plus inherit
Gamemode   [ Default (on, global) v ]      Gamemode   [x] same as global  (on)
MangoHud   [ On                   v ]      MangoHud   [ ] same as global  (==o)
Wayland    [ Off                  v ]      Wayland    [ ] same as global  (o==)
```

A is one control per row and explains itself. B keeps the switch look but
doubles the controls, and a greyed-out switch is easy to misread.

### 2. Where the page opens

**Recommended:** option A, a full page that replaces the library, with a
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
|   Gamemode  [ Default (on)     v ]     | |               |                |
| ...                                    | |               |                |
|----------------------------------------| |               |----------------|
| 2 changes            [Revert] [Save]   | |               | [Revert][Save] |
+----------------------------------------+ +---------------+----------------+
```

### 3. Save model

**Recommended:** option A, Save and Revert. A runner or DXVK change can
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
gamescope (off, same as the environment, or its own settings).

**Recommended:** Add target offers the installed programs from
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
     Gamemode   [ Same as environment v ]
     Gamescope  [ Same as environment v ]
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
  (Default unsets, empty gamescope fields stay unset, removed env rows
  unset), dirty tracking, revert, the leave prompt, and reload on a file
  change.
- A round trip test: load a commented config, change one control, save, and
  check that only that line changed.
