Status: done

# 0032: Declarative winetricks dependencies

## Goal

Let an environment list the winetricks verbs it needs and have wineo install
whichever are missing, instead of remembering to run `wineo winetricks` by
hand after every new prefix, clone to a fresh prefix, or rebuild. This is GUI
entry criterion 6. The GUI will show the list as an editable set of verbs, so
the config shape settles here.

```toml
winetricks = ["vcrun2022", "corefonts", "d3dcompiler_47"]
```

```sh
wineo set palworld 'winetricks=["vcrun2022", "corefonts"]'
```

## Existing foundation

- `wineo winetricks NAME [ARGS]...` (`commands/winetricks.rs`) already runs
  winetricks correctly for both runner kinds. On Proton it runs umu's own
  `winetricks` verb inside the container, so nothing extra is needed on
  `PATH`. On Wine it finds `winetricks` on `PATH` and passes `WINE`,
  `WINESERVER`, and a `WINEDLLOVERRIDES` with the menu builder off.
  `WINEPREFIX` is set last and can't be overridden.
- It holds `Operation::Winetricks` because several verbs wait for every wine
  process in the prefix to exit, refuses while the environment is running or
  its last run was lost, and reconciles DXVK/vkd3d and desktop entries
  first. Ctrl+C ends the session and waits for the prefix to go idle.
- DXVK and vkd3d are the model for "wanted versus applied":
  `install::reconcile` diffs the config against a per-prefix manifest
  (`.wineo-components.toml`) and only touches the prefix when something
  differs. `build::prepare_target` runs it before every `start`, `run`, and
  `restart`, and `winetricks` runs it too.
- winetricks keeps its own record. Every verb it installs, settings included,
  is appended to `$WINEPREFIX/winetricks.log`, one per line, in install
  order. `winetricks list-installed` just prints that file, and winetricks
  itself uses it to skip verbs that are already installed and to refuse
  conflicting ones (checked in winetricks 20260125). On Proton, `WINEPREFIX`
  is the `pfx/` directory, so the log lands there.
- No prefix on the maintainer's machine has a `winetricks.log` yet.
- `WINE_ONLY_KEYS` in `config.rs` makes native environments refuse Wine-only
  keys like `dxvk` and `wayland`. Plan 0026 already says winetricks
  dependencies will be Wine-only.
- `kind`, `mangohud`, `wayland`, and `[gamescope]` were added without a
  schema bump: an older wineo refuses a config using them with
  `unknown field` instead of misreading it. `winetricks` follows the same
  rule, so it needs no bump either. The `features.md` row calling this "the
  first real test of 0018" predates that and should be updated.

## Decisions (settled with the maintainer, 2026-09-27)

### 1. The installed record

**Recommended: read `winetricks.log`.** It's winetricks' own record, it's
what winetricks checks before skipping a verb, and it already includes verbs
installed by hand with `wineo winetricks`. It travels with the prefix through
clone, snapshots, portable copies, and trash. A verb is missing when no line
of the log equals it.

The alternative is a wineo manifest like `.wineo-components.toml`. It would
duplicate winetricks' log and go stale whenever someone ran winetricks by
hand.

### 2. When missing verbs get installed

**Recommended: A, at launch, like DXVK.**

A. At launch. `start`, `run`, and `restart` install missing verbs before the
   game starts, in the foreground, so `start` shows winetricks' progress
   before it detaches. `validate --fix` installs them ahead of time.
   (`wineo winetricks NAME` with no verbs already opens the winetricks GUI,
   so it can't double as "install what's missing.")

```
$ wineo start palworld
Installing vcrun2022, corefonts with winetricks
...winetricks output...
Started palworld
```

B. At `set`. `set` installs right away, and launches only warn about missing
   verbs. A long download at `set` time is surprising, and a clone or a new
   prefix would still need a launch-time path.

C. Only on request. Launches never install. `validate` reports
   `winetricks_missing` and `validate --fix` or `wineo winetricks NAME`
   installs. The simplest option, but it doesn't remove the step the feature
   is meant to remove.

With A, `validate` still reports `winetricks_missing`, and `--fix` installs.

### 3. When an install fails

**Recommended: the launch fails.** The game likely needs the verb, and
winetricks' output is already on screen. Running `start` again retries only
what's still missing, since winetricks logs each verb as it succeeds.

```
$ wineo start palworld
Installing vcrun2022, corefonts with winetricks
...
winetricks failed installing corefonts for palworld
```

The alternative is a warning and a launch anyway.

### 4. Removing a verb from the list

winetricks can't uninstall. **Recommended:** removing a verb only stops
wineo from installing it again. The docs say so, and `set --unset` or editing
the list never touches the prefix.

### 5. Where the key lives

**Recommended:** environments and templates only, Wine-only. Verbs are
prefix-wide, so a target-level list doesn't make sense. A global list ("every
prefix gets corefonts") is possible but can be added later without breaking
anything.

### 6. Which verbs are allowed

**Recommended:** any verb, checked only for shape at load (no spaces, no
leading `-`, so flags like `--force` can't sneak in). Unknown verbs fail when
winetricks runs, with winetricks' own error. Checking against
`winetricks list-all` would need winetricks installed just to read a config.

Settings verbs like `win10` or `renderer=vulkan` are logged too, so they work
as "apply once." If the setting is later changed by hand, wineo won't put it
back. The docs say so.

## Launching

- The check is a read of `winetricks.log`, so a launch with nothing missing
  costs one small file read and starts no process.
- Missing verbs install in list order, in one `winetricks -q` call, with the
  same environment, locks, running check, and Ctrl+C handling as
  `wineo winetricks` today. On Wine, a missing `winetricks` on `PATH` is an
  error before anything runs, like `gamescope_missing`.
- `restart` and `start` on an already-running environment skip the install,
  since winetricks needs the prefix idle; `status` and `validate` still
  report what's missing.
- `-e`/`--env` overrides from the launch apply to the winetricks call too,
  like they do for `wineo winetricks -e`.

## Reporting

- `status` gains a `winetricks` line listing the verbs, with missing ones
  marked. Its JSON gains `winetricks` (the list) and `winetricks_missing`.
- `validate` gains `winetricks_missing` (fixable) and `winetricks_unavailable`
  when a Wine environment lists verbs and `winetricks` isn't on `PATH`.

## Heroic import

Heroic has no declarative list; it runs winetricks on demand. **No mapping.**
An imported prefix keeps its `winetricks.log`, so verbs installed through
Heroic already count as installed if the maintainer adds them to the list.

## Implementation sequence

1. Core: the `winetricks` key on the environment and templates, shape
   checks, `WINE_ONLY_KEYS`, reading `winetricks.log`, and the missing-verb
   computation. Move the winetricks runner out of the CLI command into core
   so launch and the command share it.
2. CLI: install at launch through `build::prepare_target`,
   `set`/`--unset` for the array, `status` and its JSON,
   the validate codes and fix, and both example configs.
3. Docs: a winetricks section in `config-files.md` (or its own page),
   `validate.md`, `json-output.md`, `native.md`, README, and `features.md`.

## Verification and acceptance

- Unit tests for parsing, shape checks, native refusal, templates, and the
  missing-verb computation against fixture logs (empty, missing file,
  settings verbs, a verb that's a prefix of another like `vcrun2022` and
  `vcrun2022_arm`).
- Launch tests with the fake runners: nothing missing starts no winetricks,
  missing verbs run once in order with `-q`, a failure stops the launch, a
  running environment skips the install, and `-e` reaches winetricks.
- An ignored real-program test installing a small verb (like `corefonts` or
  a settings verb such as `win10`) into a temporary prefix, then launching
  again to confirm nothing reruns.
- The maintainer adds a real verb to one Wine and one Proton environment,
  launches, and launches again.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`.

## Scope limits

- No uninstalling. winetricks can't.
- No global or per-target lists yet.
- No verb catalog, search, or descriptions. The GUI can add a picker later
  from `winetricks list-all`.
- No Heroic mapping.
