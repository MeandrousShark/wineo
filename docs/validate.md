# Validate

```sh
wineo validate
wineo validate zomboid
wineo validate --fix
wineo validate --json
```

`wineo validate` looks for config mistakes and leftover state before a launch
trips over them. It checks the global config, every template, every
environment, and anything left behind by an environment that no longer exists.
Give a name to check only that environment: its config, targets, runtime
state, desktop entries, and art.

The desktop app runs the same checks from an environment's
[Check for problems](gui.md#checking-for-problems) and its
[Checkup page](gui.md#checkup).

Nothing is launched. Validate never runs wine or a program, so it's fast and
safe on a running environment.

## The report

Healthy environments aren't listed. A clean run prints one line:

```
$ wineo validate
No problems in 6 environments.
```

Otherwise problems are grouped by environment, or by file for the global
config and templates, with a level on each line:

```
$ wineo validate
zomboid
  error    target zomboid: C:\Games\Zomboid\ProjectZomboid64.exe not found
  warning  art: icon.png is recorded but missing
oldgame
  warning  desktop entry wineo-oldgame-oldgame.desktop has no environment

1 error, 2 warnings.
```

An **error** means something will fail: a command refuses the config, or a
launch can't work. A **warning** is leftover or odd state that doesn't stop
anything. `-v` also lists everything that was checked, with `ok` on the clean
ones.

A config that fails to parse shows its first error only. Fix it and run
validate again to see the next one. Its other checks are skipped until it
parses.

## Fixes

Some problems have a safe fix. At a terminal, validate lists them after the
report and asks once:

```
  1) Drop zomboid's missing icon record
  2) Remove oldgame's desktop entry
Numbers skip those fixes, like 1 3 4.

Press Enter to fix them, or pick numbers. Ctrl+C to cancel.
Choice:
Fixed 2 problems. 1 error left.
```

Enter applies every fix listed. Type one or more numbers, like `1 3 4`,
`1,3,4`, or a range like `2-4`, to take those off the list. It's shown again
with what's left.
Ctrl+C cancels without fixing anything.

With no terminal, validate only reports. `--fix` applies every fix without
asking, for scripts.

Fixes never touch a config you wrote or an art file, and the only fix that
touches a prefix installs missing [winetricks verbs](config-files.md#winetricks-dependencies),
the same way `start` would. The only config a fix edits is `art.toml`, to drop a record whose file or target is
gone, and its comments are kept. A missing prefix or an unsupported
`schema_version` is reported, never rewritten.

A fix takes the same lock as the command it stands in for. If an environment
is busy, say with a snapshot restore, its fix fails with the usual busy error
and the others still run.

Interrupted trash removal, restoration, or pruning is reported as an error.
Recovery is explicit: review with `wineo validate`, then use
`wineo validate --fix` to recover the reported operation. Do not use
conflicting commands until recovery is complete. Invalid or unavailable trash
storage is reported for attention rather than recovered by ordinary commands.

## What it checks

| Area | Check | Level | Fix |
|---|---|---|---|
| `config.toml` | Syntax, unknown keys, bad values, unsupported `schema_version` | error | |
| Templates | Same parse checks | error | |
| Templates | Runner or DXVK/vkd3d build that doesn't resolve | warning | |
| Environment config | Same parse checks | error | |
| Wine environment | Runner doesn't resolve | error | hint: `wineo download` |
| Wine environment | DXVK or vkd3d build doesn't resolve | error | download it |
| Wine environment | `winetricks` verbs missing from the prefix's `winetricks.log` | warning | install them, like `start` does |
| Wine environment | `winetricks` verbs listed, but a Wine runner can't find `winetricks` on `PATH` | error | |
| Wine environment | Prefix missing, but `start` would build it | warning | |
| Wine environment | Prefix `start` would refuse, or one with the other layout | error | |
| Target | Exe missing (a Unix path, or a `C:\` path in an existing prefix) | error | |
| Target | Unix working dir missing | error | |
| Target | `gamemode` on, but gamemode isn't installed | error | |
| Target | gamescope on, but gamescope isn't installed | error | |
| Target | `mangohud` on, but `mangohud` (or `mangoapp` under gamescope) isn't installed | error | |
| Target | `wayland` on, but the runner has no Wayland support | error | |
| Native target | Exe missing, not executable, or a Windows path | error | |
| Environment | Several targets and no default, so `start` needs `--target` | warning | |
| Runtime | Lost run | warning | clean it up, like `wineo stop` |
| Runtime | Unfinished installer | warning | clean it up, like `wineo stop` |
| Runtime | Interrupted snapshot restore | error | hint: `wineo snapshot recover` |
| Trash | Interrupted removal, restoration, or prune | error | recover with `wineo validate --fix` |
| Trash | Trash storage cannot be inspected | error | |
| Runtime | State dir for an environment that no longer exists | warning | remove it |
| Runtime | [Playtime log](stats.md) for an environment that no longer exists | warning | remove it |
| Desktop | Entry for an environment that no longer exists | warning | remove it and its icons |
| Desktop | An environment's entries out of date with its config | warning | rewrite them, like `start` does |
| Desktop | Portable config with `desktop = true` | warning | |
| Art | `art.toml` fails to parse | error | |
| Art | Recorded file missing, or a target that no longer exists | warning | drop the record |
| Art | Duplicate files for one kind | warning | |
| Art | Art dir for an environment that no longer exists | warning | |

Some things are left out on purpose:

- The templates wineo ships aren't checked.
- DXVK and vkd3d builds aren't checked for Proton runners, which bring their
  own, or when the prefix already holds the pinned builds.
- When a prefix is missing, its `C:\` exe checks are skipped, since the prefix
  error covers them.
- When the runner doesn't resolve, a missing prefix isn't reported, since
  whether `start` would build it depends on the runner.
- An interrupted snapshot restore skips the rest of that environment's
  checks. Recover it first.
- A leftover state dir that still records a lost run or an unfinished
  installer has no fix, because `wineo stop` can't reach it once the
  environment is gone. Remove it by hand once nothing from it is running.
- Runner downloads are a hint, not a fix, since runners are big.

Locks aren't checked. Every wineo lock is released by the kernel when its
holder exits, so a stale one can't exist.

## Scope

Only the active root is checked: the standard locations, or the folder given
by `--portable` or `WINEO_PORTABLE_DIR`. Portable environments never get
desktop entries, so desktop checks are skipped there, apart from the warning
about a hand-edited `desktop = true`.

Shared-prefix discovery is also limited to registrations in the selected root;
idle registrations in other roots may not be detected.

## Exit status

Validate exits 1 when errors are left after any fixes, or when a fix failed,
and 0 otherwise, so warnings alone don't fail a script. Ctrl+C at the review
exits 130.

`--json` output is described in [JSON output](json-output.md#validate).
