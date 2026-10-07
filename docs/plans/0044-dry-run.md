Status: draft

# 0044: `--dry-run` for import and non-guided `new`

## Goal

Let a script or a careful person see what `wineo new` and `wineo import`
would create, copy, adopt, or register, without touching disk, the network,
or Wine. A dry run resolves everything the real run would resolve, prints it,
and stops.

## Settled by the maintainer

From the backlog row in [features.md](../features.md):

- Cover `import` and non-guided `new`.
- Import already builds a review before writing, and a dry run can print that
  review and stop.

Queued first on 2026-10-06, ahead of the stats, hooks, and store plans.

## Existing foundation

- `wineo new -n NAME` calls `wineo_core::create::run`
  (`crates/wineo-core/src/create.rs`). It resolves the template, arch, and
  runner, checks that template components are installed
  (`install::check_available`), and works out the prefix directory. Then it
  takes the create lock, builds or adopts the prefix, writes the config, adds
  the `--exe` target, reconciles components, and writes desktop entries. The
  resolution half and the writing half share one function today, and the
  runner is resolved with `resolve_locked`, which takes a build lock.
- `new --native` goes straight to `Environment::create_native`, then
  `desktop::reconcile_native`.
- `import prefix SOURCE -n NAME --runner R` is non-guided. It already calls
  `import::inspect` and `print_review` before `import_reviewed` writes
  anything, so a dry run stops between the two.
- `import heroic --game ID -n NAME --runner R` builds a plan with
  `plan_game` and `check_builds`, shows `show_game_summary`, then applies it.
  Native Heroic games take `native::direct` instead.
- `import heroic --all` plans the whole library with `plan_library`, prints a
  table with `render_bulk_table`, and asks before importing. It writes before
  that confirmation in two places: `library.record_backfills` records
  `heroic_id` in existing configs, and `resolve_pending` and
  `download_latest_ge_proton` can download builds.
- The external runners offer from 0043 records its answer in global config
  the first time a game is added.

## Proposed design

### Commands

`--dry-run` is added to:

```text
wineo new -n NAME [--native] [other flags] --dry-run
wineo import prefix SOURCE -n NAME --runner R [other flags] --dry-run
wineo import heroic --game ID -n NAME --runner R [other flags] --dry-run
wineo import heroic --all [other flags] --dry-run
```

Native Heroic games skip `--runner`, as they do today. The flag has no short
form, since `-n` is already the name.

### What a dry run never does

- Write anything: configs, prefixes, desktop entries, icons, art, logs, lock
  files, run state, `heroic_id` backfills, or the external runners answer.
- Use the network: no build downloads, no SteamGridDB search, no umu runtime
  download.
- Start Wine, umu, an installer, or winetricks.
- Prompt. It runs the same with or without a terminal.

### What it checks

Everything the real run checks before its first write: the name is valid and
free, the template and runner resolve, the arch fits the runner, template
components are installed, the source is a prefix (import), the Heroic game
exists and is importable, and the `--exe` and `--install-exe` paths exist.

When a check fails, the dry run prints the same error the real run would and
exits 1. Things the real run would download or fetch are reported as "would"
lines instead of failing, except where the real run would fail too (a missing
template component in `new` fails both).

### Output

The plain review the real run would print, then one closing line. Examples:

```text
$ wineo new -n hades -t gaming --exe ~/Games/Hades/Hades.exe -d --dry-run
Would create environment hades
  prefix: ~/.local/share/wineo/prefixes/hades (new)
  runner: latest -> GE-Proton10-4 (Steam)
  template: gaming, win64
  components: dxvk 2.6, vkd3d-proton 2.14
  target: hades -> ~/Games/Hades/Hades.exe
  desktop entry: Hades
Dry run, nothing was created.
```

```text
$ wineo import prefix ~/old/hades -n hades --runner wine --dry-run
Import hades
  source: /home/me/old/hades
  copy to: /home/me/.local/share/wineo/prefixes/hades
  runner: wine -> wine-9.18
  layout: Wine, win64
The source game and launcher must be idle. External game files and saves stay where they are.
The source prefix stays in place. `rm --delete-prefix` deletes the imported copy.
Dry run, nothing was imported.
```

```text
$ wineo import heroic --all --dry-run
Checking 12 Heroic games...
<the existing bulk table>
Would record the Heroic id for 2 existing environments: hollow, celeste
Would download GE-Proton10-4 for 3 games.
Dry run, nothing was imported.
```

`import heroic --all --dry-run` doesn't need `--yes` or a terminal.

### Core shape

`create::run` splits into `create::plan`, which resolves and checks without
locks or writes and returns a `CreatePlan`, and `create::apply`, which takes
the plan, locks, re-checks what could have changed, and writes. `run` stays
as `plan` then `apply`, so the GUI and the CLI keep working unchanged. A dry
run and a real run then share every check by construction instead of by
copying.

Import already has this split (`inspect` then `execute_reviewed`), and so
does Heroic (`plan_game` and `plan_library`, then apply). Those only need
their write-before-confirm steps moved behind the dry-run check, or reported
instead when it's set.

## Open questions

1. Guided flows. Recommendation: `--dry-run` only works when the flags
   answer everything, and otherwise fails with the same "needs ..." message
   the no-terminal case prints. The guided flows already end at a review the
   person can back out of.
2. JSON. Recommendation: text only in this plan, and a backlog row for
   `--dry-run --json` if a script ever needs it. The review lines are already
   stable enough to grep.
3. `new --exe` with a missing program. The real run creates the environment
   and then exits 1 with "was not found". Recommendation: the dry run fails
   up front with that message and exits 1, since that's what the result
   would be.
4. Art. Recommendation: `--art --dry-run` prints "Would fetch art for NAME"
   without checking the key or searching, since both need the network or the
   keyring.
5. Wording. Recommendation: "Would create environment NAME" for `new`, the
   existing "Import NAME" review for import, and a closing "Dry run, nothing
   was created." or "Dry run, nothing was imported."

## Implementation sequence

1. Core: split `create::run` into `plan` and `apply`, with `plan` resolving
   the runner and component builds without locks, and `apply` re-checking
   the adopt decision and runner under its locks. Add a plan for native
   creation. No behavior change; existing tests stay green.
2. Core: let `plan_library` report backfills and pending builds without
   recording or downloading them, so the CLI can print them.
3. CLI: `--dry-run` for `new`, both Wine and native, with the "Would create"
   review.
4. CLI: `--dry-run` for `import prefix`, `import heroic --game`, native
   Heroic games, and `import heroic --all`.
5. Docs: `downloads-and-imports.md`, the `new` usage in `README.md`, the
   `features.md` row, and the acceptance checklist.

Steps 1 and 2 can run in parallel. Step 3 needs 1, and step 4 needs 2.

## Testing and acceptance

- An integration test helper snapshots the temporary XDG roots (paths,
  sizes, and mtimes) before and after each dry run and asserts they match.
  Every dry-run test uses it.
- The fake Wine and umu runners from `tests/common` fail the test if a dry
  run starts them.
- Each command gets a success test checking the review lines and exit 0, and
  failure tests for a taken name, an unknown runner, a missing source, and a
  missing `--exe`, checking the error matches the real run's.
- Heroic `--all` tests cover a library with backfills and pending builds, run
  without a terminal and without `--yes`.
- The acceptance checklist uses a `WINEO_PORTABLE_DIR` folder: dry-run each
  command, confirm the folder is unchanged, then run it for real and compare
  the result with the review.

## Scope limits

- No GUI changes. The GUI's Add and import flows already show a review.
- No dry run for other writing commands (`clone`, `set`, `rm`, `snapshot`,
  `setup`). They can get their own row if wanted.
- A dry run can't promise the real run will succeed: prefix creation,
  downloads, and installers can still fail, and the disk can change between
  the two runs.
