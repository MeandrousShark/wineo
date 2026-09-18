Status: done

# 0002: Everyday management

## Goal

Round out day-to-day use: remove an environment, run winetricks in it, complete verbs and flags in the shell, and periodically clear oversized session logs.

## Decisions

Settled by the maintainer:

- The plan covers four priority 1 rows from `docs/features.md`: the `rm` verb, `wineo winetricks`, shell completion for verbs and flags, and a log size cap. Per-environment process tracking waits until real apps show whether 0001's program exit tracking is enough.
- `rm` accepts exactly one environment name. Bulk removal is intentionally unsupported, now and in future plans: no multiple names, wildcard selection, or `--all` option.
- `rm` keeps the prefix unless `--delete-prefix` is passed, and refuses while the environment is running or a conflicting operation is active.
- `winetricks` refuses to run while the environment is running, since several winetricks verbs wait for every wine process in the prefix to exit. It holds exclusive access for the operation so another wineo command can't start using or remove the prefix during installation.
- `wineo completions <shell>` prints the completion script, and the user saves it where their shell looks.
- After Ctrl+C interrupts winetricks, wineo waits for the prefix to go idle before releasing exclusion, using the existing graceful-stop timeout and forced wine cleanup. A repeated Ctrl+C forces cleanup.
- Log reset detection uses a per-run reset counter stored in its own file next to the log, not a field in the runtime state file.
- The log limit is global only and is a soft threshold, checked periodically. When a detached run's log passes it, wineo empties the file and notes that output was dropped. This discards all accumulated output, including the most recent lines before the reset; subsequent output starts a fresh log in the same file.

## Details

### Operation coordination

- Keep operation exclusion separate from the existing run lock. Holding an operation lock must not imply that an app is running or make `stop` signal a removal process.
- Operation locks use stable files outside the per-environment runtime directories that `rm` removes. Do not unlink a coordination lock that another command could still open or hold.
- `rm` and `winetricks` exclude conflicting operations. `start`, `run`, `exec`, creation/adoption, and changes to an environment's prefix participate in the same coordination. Existing supported use of `exec` alongside an app continues to work.
- Coordinate by environment name and resolved prefix identity where applicable, so different names pointing to the same prefix cannot bypass exclusion. Acquire multiple required locks in a consistent order.
- Validate config existence, prefix identity, and operation eligibility after acquiring the relevant locks. A start prepared before removal must revalidate before spawning; it cannot launch from a stale spec after the config is gone. Detached runs retain the necessary protection in the supervisor.
- Keep exclusion until the operation and its use of the prefix end. A one-time `is_running()` check is not sufficient.
- These locks coordinate wineo commands. Wine started outside wineo does not take them; users must close those programs before installing components or deleting a prefix. Broader process discovery remains deferred.

### `wineo rm <name> [--delete-prefix]`

- Removes the environment's config file and its runtime state dir (`state.toml`, the logs, `create.log`, the run lock). Stable operation lock files are retained.
- Requires one exact name. `wineo rm a b` is a usage error and removes nothing, including when a shell wildcard expands to multiple names.
- A running environment is refused with `<name> is running, stop it first`. Conflicting operations are refused with a message identifying what is busy. Removal holds operation exclusion through prefix, runtime state, and config cleanup.
- If a run is marked as lost, refuse removal and direct the user to `wineo stop <name>` first. A missing supervisor lock does not prove its Wine processes have stopped.
- Output when the config can be read: `Removed environment photoshop, its prefix is still at ~/Apps/Photoshop`, or with `--delete-prefix`, `Removed environment notepad and its prefix`.
- `--delete-prefix`:
  - Deletes the prefix dir without following symlinks, since a prefix links `z:` to `/` and its user folders to the home directory.
  - Refuses a dir that isn't recognized as a Wine prefix (no `system.reg`). This catches common wrong-path mistakes; it is not proof of exclusive ownership. A prefix dir that's already gone is noted and skipped.
  - Refuses deletion if another environment refers to the same resolved prefix, even if that environment is stopped. Compare canonical paths so path aliases are recognized. The reference check is coordinated with adoption and prefix config changes, so a new reference cannot appear during deletion.
  - Applies to adopted prefixes outside the prefix root too, since the flag is explicit.
  - The prefix is deleted first and the config last. A filesystem failure can leave partial results; retain the config and report the path and failed step. Do not promise rollback or unconditional retries.
  - If a partial deletion removed `system.reg`, a retry still refuses the unrecognized directory. Explain that the user must inspect and finish cleaning up the remaining prefix manually, then run `wineo rm <name>` to remove the registration. Do not add a force option that bypasses prefix validation.
- Broken configs can still be removed without `--delete-prefix`; parsing them is optional for the success message. Use `Removed environment photoshop; its prefix was left untouched` when the prefix path cannot be read. Invalid runtime state must not be silently treated as proof that a prefix is safe to delete.

### `wineo winetricks [-e KEY=VAL]... <name> [winetricks args...]`

- Runs `winetricks` from PATH attached, like `exec`, with:
  - `WINEPREFIX` set to the resolved prefix, and `WINE` and `WINESERVER` set to the runner's resolved absolute executable paths, so PATH changes cannot select a different runner.
  - The merged `[env]` tables and `-e` values, with the menu builder override from 0001 applied after merging.
  - Wineo owns `WINEPREFIX`, `WINE`, and `WINESERVER` for this command. Inherited values are replaced; explicit `[env]` or `-e` assignments to these keys are rejected with a clear error. Existing global rejection of `WINEPREFIX` remains unchanged.
- Wineo options such as `-e KEY=VAL` go before the name. Everything after the name goes to winetricks, including `-q`, `--force`, `-e`, and `-h`. No `--` is needed. `wineo winetricks --help` shows wrapper help; `wineo winetricks notepad -h` passes `-h` to winetricks. An optional `--` immediately after the name is consumed as the argument separator.
- No arguments opens the winetricks GUI.
- Refuses while running with `<name> is running, stop it before running winetricks`, and refuses a missing prefix, like `exec`. Lost runs must be cleaned up with `stop` first.
- `winetricks` missing from PATH is an error: `` `winetricks` was not found on PATH ``.
- Holds operation exclusion until winetricks exits and its prefix is idle, including after cancellation. Another winetricks invocation and conflicting starts, execs, removals, or prefix changes are refused while it is active.
- Does not overwrite the previous app run's state or restart options. `status` and `list` identify the environment as busy with winetricks. `stop` and `restart` report that winetricks must be finished or canceled in its terminal instead of treating it as an app run.
- Ctrl+C cancels the attached command. Exclusivity is held until the prefix is idle: wineo applies the existing graceful-stop timeout and then forced Wine cleanup, so exclusion is never released while its Wine children remain active. A repeated Ctrl+C forces cleanup immediately. The wrapper returns the cancellation exit code.
- The command's exit code passes through, including the same signal-to-exit-code mapping used by `exec`.

### `wineo completions <shell>`

- Prints the script for `bash`, `elvish`, `fish`, `powershell`, or `zsh` to stdout, generated by `clap_complete` from the same clap definitions. Saved scripts must be regenerated after upgrades to include new or changed commands.
- Covers verbs and flags. Environment and target names stay a separate, later feature.
- Hidden commands like `__supervise` are left out.
- New dependency in the CLI crate: `clap_complete`.

### Soft log size limit

Global config:

```toml
# ~/.config/wineo/config.toml
log_limit_mb = 50        # 0 turns the cap off
```

- Defaults to 50. `wineo set --global log_limit_mb=100` works like `stop_timeout`. The setting is global only; per-environment limits remain deferred.
- The limit is read when a run starts, so a change applies from the next start.
- The supervisor checks the log's size every second throughout the run, including while waiting for background Wine processes after the launched program exits. It checks once more before completing the run, so a short burst before exit is also handled.
- Once the file is over the threshold, the supervisor empties it and writes `wineo: the log passed 50 MB; all output before this reset was dropped`. Wine keeps writing straight to the same file in append mode. This clears the entire log; it does not retain the last 50 MB.
- This is a soft threshold, not a hard disk-usage bound. Bursts can exceed it between checks, and a supervisor crash can prevent a pending or final check.
- `output.log.1` still keeps the previous run's log. That file may have been written with a larger limit or with the limit disabled, so total log usage is not bounded by twice the current setting.
- `wineo logs -f` must detect a reset even if the file has already grown past its old read position. A size-shrink check alone is insufficient. Detection uses a per-run reset counter in its own file next to the log. The supervisor bumps it with short coordination around truncation so the counter change and the truncation are observed consistently. A follower that sees the counter change seeks to the beginning of the file; follower reads stay bounded so rapid regrowth cannot stall it.
- Following continues from the beginning of the current reset's output. Bytes discarded before the follower reads them cannot be recovered. `logs -n` must tolerate a concurrent reset instead of failing on a short read.
- Attached runs aren't logged, so the cap doesn't apply to them.

### Testing

- `rm`: CLI tests without wine for removing config and state, keeping the prefix, broken-config output, an already-missing prefix, and rejecting multiple names before any deletion. Test `--delete-prefix` on a fake prefix with a symlink out of it (whatever it points at must survive), refusal of a non-prefix dir, shared prefixes including path aliases, active operations, and lost runs. Simulate partial deletion with and without `system.reg` remaining and verify the config and recovery message.
- Coordination: controlled races between removal and a prepared start, winetricks and start/run/exec/removal, and prefix deletion and adoption/config changes. Verify stable lock identity survives runtime cleanup, aliases cannot bypass exclusion, and compatible app/exec use still works. Use explicit synchronization rather than timing-only sleeps.
- `winetricks`: a fake script on PATH prints its arguments and `WINEPREFIX`, `WINE`, `WINESERVER`, and `WINEDLLOVERRIDES`, then exits with a chosen code. Cover resolved runner paths, inherited and explicit managed-variable conflicts, configured/CLI DLL overrides, missing winetricks, missing prefix, nonzero and signal exits, busy reporting, cancellation, and child cleanup. Parser tests cover wrapper options before the name, forwarded flags and `-e` after it, `-h`, `-- -h`, and multiple arguments.
- `completions`: every supported shell produces a script containing visible commands and flags, including the completion command and its shell choices. Every shell excludes `__supervise` from the generated script.
- Log limit: a controlled fake writer crosses `log_limit_mb = 1` and verifies periodic clearing, the marker, subsequent output, and a final check for a burst shorter than the polling interval. Cover disabled limits, background-process output, and a previous log created with a larger or disabled limit. Do not assert a hard size bound during active writes.
- Log reading: force a truncate-and-regrow cycle past the follower's old offset before its next poll, repeated resets, a concurrent reset during `logs -n`, and a reset as the run ends. Verify the follower resumes at the current reset rather than silently skipping its initial output.
- Ignored real wine tests: `winetricks list-installed` on a fresh prefix, and a `WINEDEBUG=+all` notepad run with `log_limit_mb = 1`.

## Phases

1. Operation coordination, single-environment `rm`, `completions`, and `winetricks`.
2. Soft log size limit and reset-aware log reading.

Commit each logical change separately. Each phase ends with tests, fmt, and clippy passing, then a push.

## Done when

With wine and winetricks installed:

```
wineo completions fish > ~/.config/fish/completions/wineo.fish
wineo st<Tab>                              # offers start, status, stop
wineo new -n notepad
wineo set notepad targets.notepad.exe=notepad
wineo start notepad
wineo winetricks notepad list-installed    # refuses, notepad is running
wineo stop notepad
wineo winetricks notepad list-installed    # runs against the notepad prefix
wineo set --global log_limit_mb=1
wineo start notepad -e WINEDEBUG=+all       # oversized output.log is cleared periodically
wineo stop notepad
wineo rm notepad                           # prefix is kept
wineo new -n notepad --dir ~/.local/share/wineo/prefixes/notepad
wineo rm notepad --delete-prefix           # prefix is gone
```

`wineo rm a b` is always rejected before changing either environment. While a winetricks command is active, `status` reports it as busy and conflicting operations are refused.

