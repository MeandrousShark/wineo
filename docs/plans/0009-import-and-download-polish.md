Status: approved

# 0009: Import and download polish

## Goal

Fix what the maintainer hit walking through Heroic import and runner
downloads: output that is too verbose to review, a missing desktop entry
step, download lists that are hard to pick from, and no way to import every
Heroic game at once.

## Decisions

Settled by the maintainer:

- Bulk Heroic import is built in, not a script.
- Imported environments record their Heroic game id. Only imports record it.
- Output uses color.
- Truncated output (collapsed link lists and similar) expands with
  `--verbose` / `-v`.
- Heroic import offers a desktop entry, like guided `new`.
- Download lists show a few releases with the newest marked, and a way to
  see the rest. Sizes are human readable.
- `--json` moves to 0010.
- Keep numbers for list items and letters for fixed actions.
- `--all` asks once whether to create desktop entries, and a yes creates one
  for every imported game.
- `-v` also writes the full detail to an import log.
- `--all` applies the guided review defaults without prompting: omit
  unsupported settings, drop conflicting managed settings, keep attention
  items. Each shows as a `warn:` note in the table.
- `--all` prints one line per game while importing, then a summary. `-v`
  adds each game's full summary and writes its import log.
- `--all` also counts an environment as already imported when one of its
  targets runs the Heroic game's exe, and records the id on it.
- `--all` prints `Checking N Heroic games...` before inspecting prefixes.
- A warning shared by every game in the `--all` table prints once under it
  instead of on each row.
- Guided Heroic import no longer asks about settings needing attention,
  unsupported settings, or conflicting managed settings. It applies the same
  defaults as `--all`, and the summary shows them as warnings: attention
  items by message, dropped conflicts as `X is dropped, wineo manages it`,
  and unsupported settings as a count that `-v` expands.

The maintainer approved this plan.

## Existing foundation

- `wineo import heroic` has a guided flow and a flag-driven flow
  (`--game`, `-n`, `--runner`). Both print `show_settings` and
  `show_prefix_review` from `crates/wineo-cli/src/commands/import_heroic.rs`.
- `SettingsReview` sorts Heroic settings into carried, needs attention, and
  unsupported. `HeroicIssue` repeats several of the unsupported ones.
- Environment configs have no record of where an environment came from.
- Downloads already filter to Linux x86_64 archives. The long list is every
  upstream release, not other architectures.
- `clap` already pulls in `anstyle` and `anstream`, so color adds no new
  dependency tree.
- Prompts use numbers for list items and `b` / `q` for back and cancel, with
  occasional letters for fixed actions like `p`. That was the rule picked
  during 0007.

## Heroic import output

### Game summary

Show only settings that are set, in aligned `label  value` rows:

```text
Palworld (Heroic kZXLzzKAFWBP4dpRatpPHu)
  runner       Proton-CachyOS-latest (Heroic's copy)
  exe          ~/Games/Palworld/Palworld.exe
  working dir  ~/Games/Palworld
  gamemode     on
  esync/fsync  on
  env          DXVK_NVAPI_ALLOW_OTHER_DRIVERS=1
               PROTON_ENABLE_NVAPI=1
               PROTON_ENABLE_WAYLAND=1
  warning      EAC and BattlEye runtimes aren't imported
```

Rules:

- Booleans print as `on` / `off`, never `Some(true)`.
- Paths under `$HOME` print with `~`.
- Empty arguments, a default working directory, and unset options are left
  out.
- Each Heroic setting appears once. The carried / review / unsupported /
  issue lists merge into the rows above plus `warning` rows.
- Settings wineo already covers print nothing: `autoInstallDxvk`,
  `autoInstallVkd3d`, and `autoInstallDxvkNvapi` for Proton runners, and a
  Proton prefix with both direct and nested registry markers.
- Blocking problems (missing prefix, runner kind mismatch) print as `error`
  rows in red.
- `-v` adds the Heroic field names and the unsupported settings that are
  normally hidden.

### Final review

Facts first, editable settings last, and the editable settings are the
menu:

```text
Palworld -> palworld
  prefix    ~/Games/Heroic/Prefixes/default/Palworld (left in place)
  runner    proton-cachyos-11.0-20260703-slr-x86_64
  game dir  ~/Games/Palworld (stays in place)
  links     24 prefix files point into Heroic's GE-Proton11-1

  1) exe          ~/Games/Palworld/Palworld.exe
  2) working dir  ~/Games/Palworld
  3) arguments    none
  4) env          3 variables
  5) gamemode     on
  6) name         palworld
  7) copy to      ~/.local/share/wineo/prefixes/palworld
  8) desktop      yes
Esc back, Ctrl+C cancel
Close Heroic and the game first.
Choice, or Enter to import:
```

- `runner a -> b` only shows the arrow when the names differ.
- The standard `dosdevices` drive links are not listed.
- Other external links are grouped by the runner or directory they point
  into, with a count. `-v` lists each one.
- The adopt note becomes one line under `prefix`, for example
  `(adopted, launches modify it)`.
- Global settings that apply show as a single `global` row, expanded by
  `-v`.

### Color

- Section titles bold, warnings yellow, errors red, paths and secondary
  details dim.
- Through `anstream`, so color is off when output is not a terminal, with
  `NO_COLOR`, and on with `CLICOLOR_FORCE`.
- Applies to import, download, and the prompt helpers. Other commands keep
  their output for now and can adopt it later.

## Desktop entries

Heroic import asks whether to create a desktop entry, defaulting to yes,
with the display name defaulting to the Heroic title. It reuses 0007's
desktop entry step. The flag-driven flow gets `--desktop`.

`--all` asks once, after the table and before importing. A yes creates an
entry for every imported game. `--desktop` answers it ahead of time, and
`--yes` without `--desktop` means no entries.

## Bulk import

```text
wineo import heroic --all [--runner <name>] [--adopt] [--desktop] [--yes]
```

1. Read every Heroic sideload candidate.
2. Skip games already imported, matched by recorded Heroic id. For imports
   made before this plan, also match an adopted prefix path, or an
   environment whose name equals the default name for that game. Matched
   environments get the id recorded.
3. Use `--runner` for every game, defaulting to the newest installed Proton,
   the same rule as the gaming template. If there is none, offer to download
   the latest GE-Proton.
4. Print one table and confirm once:

```text
Import 3 Heroic games with GE-Proton11-1:
  palworld        Palworld         copy
  hades           Hades            copy
  celeste         Celeste          skip: Wine runner, needs --runner
Already imported: zomboid
Import 2 games? [Y/n]
```

   A game with review items gets a note instead of a skip, for example
   `copy   warn: EAC/BattlEye not imported`.

5. Import each game, continuing past failures, and end with a summary:

```text
Importing palworld... done
Importing hades... failed: copy: disk full
Imported 1 of 2 games. 1 failed: hades
```

Games are skipped with a one-line reason when their name is taken, their
runner kind does not match `--runner`, or they have a blocking error.
`--yes` skips the confirmation and allows running without a terminal.

### Recorded id

Environment config gains a top-level field, written by Heroic import:

```toml
heroic_id = "kZXLzzKAFWBP4dpRatpPHu"
```

Older configs without it keep loading unchanged.

## Downloads

- Print the release details once. The second print at install start goes.
- Sizes print human readable, for example `320 MiB`.
- List rows show the tag and size. The installed name only shows with `-v`.
- The guided list shows the 3 newest releases, the first marked
  `[latest]`, plus `m) more` for the full paged list:

```text
Proton-CachyOS releases:
  1) cachyos-11.0-20260703-slr  320 MiB  [latest]
  2) cachyos-11.0-20260612-slr  318 MiB
  3) cachyos-10.0-20260520-slr  301 MiB
  m) more
Esc back, Ctrl+C cancel
Choice [1]:
```

- `download <provider> --list` stays complete.

## Missing DXVK and vkd3d-proton

Proton runners bundle their own, so this only concerns Wine environments.
When a guided flow (`new`, `import`) selects a DXVK or vkd3d-proton
setting with no matching local build, offer to download the latest release
instead of failing later.

- `set` stays noninteractive. Its missing-build error adds a hint, for
  example `Get one with: wineo download dxvk`.
- Heroic imports with a Wine runner carry `autoInstallDxvk` and
  `autoInstallVkd3d` over as `dxvk = "latest"` and `vkd3d = "latest"`,
  shown as `dxvk` and `vkd3d` summary rows. Proton imports still drop them.
- Flag-driven imports and `--all --yes` don't download. They fail with the
  same hint as `set`. Interactive `--all` offers each missing download once,
  before the table.

## Import log

With `-v`, the full review (every Heroic field, link, and global setting)
also goes to `$XDG_STATE_HOME/wineo/imports/<name>.log`, defaulting to
`~/.local/state/wineo/imports/`. Each import appends a timestamped section.
The path is printed at the end. `rm` leaves the log alone.

## Acceptance

- Walk the Palworld import again. The summary and final review fit on one
  screen, and every menu setting is visible directly above the prompt.
- `import heroic --all` imports the remaining sideloaded games and skips
  Project Zomboid.
- Download of a Proton-CachyOS release prints its details once and lists 3
  releases before `more`.
- `-v` prints the expanded review and writes the import log.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test`,
  and `cargo test -- --ignored`.

## Acceptance progress

On 2026-09-23, the maintainer confirmed that the Palworld import walkthrough
worked and that downloading Proton-CachyOS worked. The maintainer did not check
the real-library `import heroic --all` case or the manual `-v` review and import
log case during that walkthrough.

On a later real-library `import heroic --all` run, Wineo recognized Zomboid as
already imported and imported five of seven selected games. Cyberpunk 2077 and
Dispatch failed after copying, when staged validation could not inspect
`pfx/system.reg`. Both source prefixes have `pfx` as an absolute symlink to
their own root. Plan 0013 fixed staged validation for this layout, and the
updated binary is installed at `~/.local/bin/wineo`. The maintainer has not
retried the two real imports. The manual `-v` review and import log case also
has not been checked.

Plan 0012's disposable Heroic fixture exercised verbose bulk import, skipping
an existing environment, expanded review evidence, and import logs. Its
automated checks also covered download output and selection. This plan remains
approved until the two real imports are retried and the manual `-v` review and
import log case is checked.
