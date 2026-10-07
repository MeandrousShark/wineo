Status: draft

# 0046: Pre-launch and post-exit hooks

## Goal

Run a command of the user's before a target starts and after its run ends,
like mounting a drive, syncing saves, or turning notifications off while
playing. Third in the queue in [features.md](../features.md), so it lands
while the supervisor is the only launch model and before store games add a
second one.

## Settled by the maintainer

From the backlog row:

- Hooks run before a target starts and after it exits.
- Post-exit belongs in the supervisor, which already owns the run's lifetime.
- The plan has to decide failure handling, the environment variables passed
  in, and whether hooks are per environment or per target.

On 2026-10-06 the maintainer answered the open questions with each
recommendation:

1. A hook is a shell command, run with `sh -c`.
2. No global hooks. Templates carry hooks, so the `gaming` template or a
   custom one can give new games the same hooks. Global hooks are a backlog
   row.
3. No timeout on a `before` hook. Stop, Ctrl+C, and the GUI's Stop cancel it.
4. `wineo stop` during `after` waits for it, and `--force` kills it.
5. Heroic's `beforeLaunchScriptPath` and `afterLaunchScriptPath` are carried
   over as hooks on import.
6. The GUI runs hooks and shows a failed `before` hook as a launch error.
   Editing hooks in GUI Settings is a backlog row.

## Existing foundation

- Two launch paths end in the same `Supervisor::wait`
  (`crates/wineo-core/src/supervisor.rs`):
  - Detached (`start`, `restart`, desktop entries, the GUI's Play):
    `launch::start` sends a `Spec` to a re-entered `wineo` process, whose
    `serve_run` takes the launch and run locks, spawns the program, reports
    `Started` or `Failed` back over stdout, then waits. Its output goes to
    the run log.
  - Attached (`wineo run`, `crates/wineo-cli/src/commands/run.rs`): the CLI
    takes the run lock, spawns the program on the terminal, and calls
    `Supervisor::with_token(...).wait` in-process.
- A run ends when nothing is left in the prefix (Wine) or the process group
  (native), not when the launched exe exits. `wait` records the playtime
  session and the run's end (`exited`, `failed`, `stopped`, `lost`).
- Launch options (`gamemode`, `mangohud`, `wayland`, `gamescope`) set the
  pattern for per-environment and per-target settings: the most specific
  wins, and a target's `[targets.<name>.gamescope]` table replaces the
  environment's whole table. They apply to game launches only, never to
  `exec`, `winetricks`, `setup`, or installers. Templates carry them too.
- New optional keys like these need no schema bump (see
  [config-files.md](../config-files.md)).
- Heroic import lists `beforeLaunchScriptPath` and `afterLaunchScriptPath`
  as unsupported today (`crates/wineo-core/src/heroic.rs`), since wineo had
  nothing to run them with.

## Proposed design

### Config

A `[hooks]` table with `before` and `after`, on an environment, a target, or
a template:

```toml
# environments/hades.toml
[hooks]
before = 'mount /mnt/games'
after = 'rclone sync ~/Saves/Hades remote:saves/hades'

[targets.editor.hooks]
after = 'notify-send "Editor closed"'
```

- A target's `hooks` table replaces the environment's whole table, like
  `gamescope`, so the `editor` target above runs no `before` hook.
  `hooks = false` on a target turns hooks off for it.
- From the command line: `wineo set hades 'hooks.before=mount /mnt/games'`
  and `wineo set hades --unset hooks.after`.
- Hooks run for game launches only, the same set as launch options: `start`,
  `run`, `restart`, desktop entries, and the GUI. Never for `exec`,
  `winetricks`, `setup`, or installers.
- Wine and native environments alike.

### When they run

```text
wineo start hades
  take the launch and run locks
  before hook        (failure cancels the launch)
  start the program
  ... the run ...
  run ends           (playtime session recorded here)
  after hook         (failure is a warning)
  release the run lock
```

- Both hooks run inside the supervisor in a detached run, and inside `wineo
  run` in an attached one, after the run lock is taken. Two launches can't
  run the same `before` hook at once, and a new launch waits for the last
  `after` hook to finish, so a save sync is never cut short by the next
  launch.
- `restart` runs the old run's `after` hook, then the new run's `before`
  hook.
- Playtime stops at the end of the run, so hook time never counts as played.
- Output goes where the program's output goes: the run log for a detached
  run, the terminal for `wineo run`.
- A hook runs in the target's working directory, like the program.

### Failure and stopping

- `before` exits non-zero: the launch is canceled, nothing starts, and
  `start` prints `Launch canceled: the before hook exited with code 1.`
  (with the run log's path in a detached run). The GUI shows the same error.
- `after` exits non-zero: a warning in the run log, or on the terminal for
  `wineo run`. The run's own end and exit code don't change.
- `wineo stop` (or Ctrl+C in `wineo run`) during `before` stops the hook and
  cancels the launch.
- `wineo stop` during `after` waits for it, since killing a sync halfway is
  worse than waiting. `wineo stop --force` kills it.
- A run that's lost (wineo itself died) never gets its `after` hook, since
  nothing is left to run it. `validate` cleaning up a lost run doesn't run
  it either.

### Environment variables

Hooks get the user's own environment plus:

| Variable | Value |
|---|---|
| `WINEO_ENVIRONMENT` | The environment name |
| `WINEO_TARGET` | The target name |
| `WINEO_EXE` | The target's exe as configured |
| `WINEO_PREFIX` | The prefix path (Wine only) |
| `WINEO_HOOK` | `before` or `after` |
| `WINEO_END` | `after` only: `exited`, `failed`, or `stopped` |
| `WINEO_EXIT_CODE` | `after` only: the program's exit code, empty if unknown |
| `WINEO_SECONDS` | `after` only: how long the run lasted |

They don't get the environment's `env` table or Wine variables like
`WINEPREFIX`, since those are for the game. A hook that needs one can read
`WINEO_PREFIX`.

### Heroic import

`beforeLaunchScriptPath` and `afterLaunchScriptPath` become `hooks.before`
and `hooks.after`, quoted for the shell, instead of review warnings. The
review lists them so the person sees what will run.

### Elsewhere

- `status NAME` shows a `hooks` line when either is set.
- `validate` doesn't check hooks, since a shell command can't be checked
  without running it.
- 0044's `--dry-run` doesn't run hooks (it launches nothing anyway).

## Implementation sequence

1. Core config: `[hooks]` on environments, targets, and templates, with the
   target-replaces-environment rule, `hooks = false`, `wineo set` support,
   and `status` output. Config and template tests.
2. Core run: a `hooks` module that runs one hook with the variables above,
   cancelable through the run's token, output to a given writer. Wire it
   into `serve_run` and `Supervisor::wait` for detached runs, and into
   `run.rs` for attached ones, so both follow the order above. Report a
   failed `before` hook through `Report::Failed`. `restart` gets both.
3. CLI and GUI messages: the canceled-launch line, the warning for a failed
   `after`, and the GUI's launch error.
4. Heroic import: map the two script paths, list them in the review, and
   drop the two "not executed" warnings.
5. Docs: a "Hooks" section in `launch-options.md`, `config-files.md`,
   `downloads-and-imports.md` for Heroic, the `features.md` row, and the
   acceptance checklist.

Step 1 goes first. Steps 2 and 4 can run in parallel after it. Step 3 needs
step 2.

## Testing and acceptance

- Unit tests for the table rules: target replaces environment, `false`,
  templates, and `set`/`--unset`.
- Integration tests with the fake runners and hooks that write marker files:
  order (before, program, after), the variables, `before` failing cancels
  with nothing started and no session recorded, `after` failing leaves the
  run's end alone, `stop` during each hook, `stop --force` during `after`, a
  second launch waiting for `after`, `restart`, `run` and `start`, and no
  hooks for `exec`, `winetricks`, or installers.
- Heroic import tests for both script paths, including a path with spaces.
- Acceptance: a `before` hook that creates a file and an `after` hook that
  removes it, for a Wine game and a native one, through the CLI and the GUI,
  plus a failing `before` hook in each.

## Scope limits

- No global hooks.
- No hooks for `exec`, `winetricks`, `setup`, or installers.
- No GUI editing of hooks.
- No hook while the program runs (like "on focus"), and no per-hook timeout.
- Store games from the later proxied kind aren't covered. Their plan decides
  whether hooks apply when Steam starts the game.
