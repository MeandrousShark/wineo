Status: draft

# 0028: `wineo validate`

## Goal

Find config mistakes and leftover state before a launch trips over them, and
offer to fix the ones that are safe to fix. Today most problems only show up
when something uses them: a typo in `environments/<name>.toml` is invisible to
`list` and `status`, a missing exe fails at `start`, a broken global config
silently falls back to defaults, and stray desktop entries are only listed by
`wineo desktop`. This is GUI entry criterion 2.

## Existing foundation

- Config parsing (`config.rs`): `check_schema` runs first, so a newer file is
  a `SchemaVersion` error rather than unknown keys. `parse_toml` turns serde
  failures, including `deny_unknown_fields`, into `ConfigSyntax`, and business
  rules give `ConfigInvalid`. Parsing stops at the first error in a file.
  Environments, the global config, templates, and `art.toml` all go through
  these helpers.
- `GlobalConfig::load_or_default` hides every global config error except
  `SchemaVersion`, so most commands never report a broken `config.toml`.
- `environment::names` and `Templates::list` list files without parsing
  them. `table::gather` (behind `list` and `status`) reads runtime state only,
  never the environment config.
- Runtime: `Runtime::status` marks a lost run, `check_resolved` finds an
  unresolved installer, and `stop.rs` cleans both up
  (`clean_up_lost_run`, `clean_up_installer`), one environment at a time.
  An interrupted restore leaves `paths.restore_marker(name)`.
- Every lock (`RunLock`, `Guard`, `ConfigGuard`, `InstallLock`) is an flock,
  so the kernel drops it when its holder dies. A stale lock can't exist, so
  validate has nothing to check there. The backlog row's "stale locks" item
  is dropped.
- Targets: `Launch::check_target` checks a Unix exe, a Unix cwd, and
  gamemode. `desktop::check` checks a `C:\` exe against the prefix without
  launching. `Launch::check_native_target` does the whole native resolution.
  `prefix::build_plan` (0023) says whether a missing prefix would be built or
  refused.
- Runners and components: `Runners::resolve` and `Builds::resolve` give
  `UnknownRunner`, `RunnerAlias`, `NoBuilds`, `UnknownBuild`, and so on.
  `components::offer_download_with` already offers a DXVK or vkd3d download
  for `NoBuilds` and `UnknownBuild`, but only in Heroic import.
- Desktop: `wineo desktop` flags entries whose environment is gone as stray
  and prints `Delete:` with their paths. `desktop::remove_file` and
  `remove_icons` can delete them. `reconcile` fixes one environment's entries.
- Art: `art::list` already returns `MissingFile`, `UnknownTarget`, and
  `Duplicates` warnings, shown only by `wineo art list`.
- `--json` conventions from `docs/json-output.md`: one document, snake_case,
  and the `{ ..., errors: [...] }` partial-failure shape with a non-zero exit.

## Proposed decisions for maintainer approval

### 1. What it checks, and how bad each problem is

Two levels. An **error** means something will fail: a command refuses the
config, or a launch can't work. A **warning** is leftover or odd state that
doesn't stop anything.

| Area | Check | Level | Fix offered |
|---|---|---|---|
| `config.toml` | Syntax, unknown keys, bad values, unsupported `schema_version` | error | no |
| Templates | Same parse checks | error | no |
| Templates | Runner or DXVK/vkd3d build that doesn't resolve | warning | no |
| Environment config | Same parse checks | error | no |
| Wine environment | Runner doesn't resolve | error | no, hint `wineo download` |
| Wine environment | DXVK or vkd3d build doesn't resolve | error | download it |
| Wine environment | Prefix missing but `start` would build it (0023) | warning | no |
| Wine environment | Prefix refused by the 0023 rules, or holds the other layout | error | no |
| Target | Exe missing (Unix path, or `C:\` path in an existing prefix) | error | no |
| Target | Unix working dir missing | error | no |
| Target | `gamemode` on but gamemode not installed | error | no |
| Native target | Exe missing, not executable, or a Windows path | error | no |
| Environment | Several targets and no default, so `start` needs `--target` | warning | no |
| Runtime | Lost run | warning | clean it up, like `wineo stop` |
| Runtime | Unresolved installer | warning | clean it up, like `wineo stop` |
| Runtime | Interrupted snapshot restore | error | no, hint `wineo snapshot restore` |
| Runtime | State dir for an environment that no longer exists | warning | remove it |
| Desktop | Entry for an environment that no longer exists | warning | remove it and its icons |
| Desktop | An environment's entries out of date with its config | warning | rewrite them, like `start` does |
| Desktop | Portable config with `desktop = true` hand-edited in | warning | no |
| Art | Recorded file missing, or a target that no longer exists | warning | drop the record |
| Art | Duplicate files for one kind | warning | no |
| Art | Art dir for an environment that no longer exists | warning | no |

When a config fails to parse, its other checks are skipped. When the prefix
is missing, `C:\` exe checks are skipped, since one error already covers it.
Nothing launches wine or runs a program, so validate stays fast.

Fixes never touch configs the user wrote, prefixes, or art files. An
unsupported version or a missing prefix is reported, never rewritten. That
follows the backlog row. The only config file a fix edits is `art.toml`, and
only to drop a record whose file or target is gone. Downloads are the one fix
that fetches anything, and they reuse the existing DXVK/vkd3d offer. Runner
downloads stay a hint, since those are big.

Open question: should unknown keys suggest the closest real key, like
`unknown field gamemod, did you mean gamemode?` Serde's message already
lists the expected fields, so the recommendation is to leave it as is.

### 2. How fixes are applied

The main choice. Here's the same broken setup under each option:

**A. Report, then one review for all fixes (recommended)**

Uses the 0022 review form. With no terminal, it only reports. `--fix` applies
every fix without asking, for scripts.

```
$ wineo validate
zomboid
  error    target zomboid: C:\Games\Zomboid\ProjectZomboid64.exe not found
  warning  art: icon.png is recorded but missing
oldgame
  warning  desktop entry wineo-oldgame-oldgame.desktop has no environment

1 error, 2 warnings.
  1) Drop zomboid's missing icon record
  2) Remove oldgame's desktop entry

Press Enter to fix them, or pick a number to skip one. Ctrl+C to cancel.
Choice:
Fixed 2 problems. 1 error left.
```

**B. Ask per fix**

```
$ wineo validate
zomboid
  error    target zomboid: C:\Games\Zomboid\ProjectZomboid64.exe not found
  warning  art: icon.png is recorded but missing
Drop zomboid's missing icon record? [Y/n]
oldgame
  warning  desktop entry wineo-oldgame-oldgame.desktop has no environment
Remove oldgame's desktop entry? [Y/n]

1 error left.
```

**C. Report only. `--fix` asks, `--fix --yes` doesn't**

```
$ wineo validate
zomboid
  error    target zomboid: C:\Games\Zomboid\ProjectZomboid64.exe not found
  warning  art: icon.png is recorded but missing (fixable)
oldgame
  warning  desktop entry wineo-oldgame-oldgame.desktop has no environment (fixable)

1 error, 2 warnings. Run wineo validate --fix to fix 2 of them.

$ wineo validate --fix
... the same review as A ...
```

A puts the whole picture on screen before asking anything, matches how
`setup` and imports already review, and makes one Enter the common path. B
interleaves questions with the report, so with many problems it gets long.
C is the safest, but it means running everything twice to fix anything.

### 3. What it prints when things are fine, and the exit code

Healthy environments aren't listed. A clean run prints one line:

```
$ wineo validate
No problems in 6 environments.
```

`-v` lists every environment and file checked, with `ok` on clean ones.

Exit 1 when errors are left after any fixes, and 0 otherwise, so warnings
alone don't fail a script. Ctrl+C at the review exits 130 without fixing
anything, like the other review menus.

### 4. Scope: all of it, or one environment

`wineo validate` checks the global config, every template, every environment,
and the stray items (desktop entries, state dirs, art dirs) with no
environment. `wineo validate <name>` checks only that environment: its
config, targets, runtime state, desktop entries, and art. It completes
environment names like the other commands.

Only the active root is checked: the standard locations, or the one folder
given by `--portable` or `WINEO_PORTABLE_DIR`. Desktop checks are skipped in
portable mode except the `desktop = true` warning, since portable
environments never get entries.

### 5. One error per broken file

Parsing stops at the first error, so a file with two typos shows one, and
the next one shows up after the first is fixed. Collecting every error would
mean a second parser beside serde's. Recommended: accept one per file.

### 6. `--json`

`wineo validate --json` prints the problems and never asks. Combined with
`--fix`, it applies the fixes first and reports what's left.

```json
{
  "problems": [
    {
      "environment": "zomboid",
      "area": "target",
      "level": "error",
      "code": "exe_missing",
      "message": "target zomboid: C:\\Games\\Zomboid\\ProjectZomboid64.exe not found",
      "fix": null
    },
    {
      "environment": "zomboid",
      "area": "art",
      "level": "warning",
      "code": "art_file_missing",
      "message": "art: icon.png is recorded but missing",
      "fix": "Drop zomboid's missing icon record"
    }
  ],
  "fixed": []
}
```

`environment` is `null` for the global config, templates, and stray items.
`code` is a fixed snake_case name per check, so a script or the GUI can match
on it without parsing messages.

### 7. Busy and running environments

Checks only read, so they run on a running environment too. A fix takes the
same lock its command would, so a fix for a busy environment (say `new` or a
restore in progress) fails with today's busy error. It's reported as not
fixed, and the rest go on.

## Implementation sequence

1. Core: a `validate` module with `Problem { environment, area, level, code,
   message, fix }`, a `Fix` enum, `check_all(paths)`,
   `check_environment(paths, name)`, and `apply(paths, &Fix)`. Move
   `clean_up_lost_run` from `stop.rs` into core so validate and `stop` share
   it. Add a way to tell whether an environment's desktop entries are out of
   date without writing them. That's a dry-run of `reconcile_with`.
2. CLI: `wineo validate [NAME]` with `--fix` and `--json`, the grouped report,
   the review from decision 2, and exit codes.
3. Docs: a `docs/validate.md` page with the check table, a `json-output.md`
   section, the README command list, and the `features.md` row marked done.
   Point `wineo desktop`'s stray `Delete:` note at `wineo validate`.

## Verification and acceptance

- Unit tests for each row of the check table, and for each fix, including a
  fix on a busy environment failing without stopping the others.
- CLI tests: a clean setup prints the one line and exits 0, errors exit 1,
  warnings alone exit 0, `--fix` applies without a terminal, and `--json`
  never asks. The config files a user wrote are byte-for-byte unchanged after
  every fix.
- Pty tests for the review: Enter fixes all, a number skips one, and Ctrl+C
  exits 130 with nothing changed.
- The maintainer breaks a few things by hand (a typo'd key, a deleted exe, a
  config removed without `wineo rm`, a deleted art file), runs
  `wineo validate`, fixes what it offers, and checks that `start` still works.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`.

## Scope limits

- No fixes that rewrite a user's config, beyond dropping dead `art.toml`
  records. No schema stamping of legacy files: a missing `schema_version`
  isn't a problem.
- No arch check of an existing prefix, and no check that a runner can
  actually start wine.
- No scan across several portable folders in one run.
- Runner downloads are a hint, not a fix.
