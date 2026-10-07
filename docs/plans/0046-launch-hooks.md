Status: approved

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

On 2026-10-06, after the revision below, the maintainer took each
recommendation on the new choices: a launch during `after` fails with the
busy message while `restart` waits; the GUI shows `before`'s output only
when it fails; and the GUI's Stop asks before killing an `after` hook. The
plan was approved the same day.

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

Found by the maintainer's review of the first draft (2026-10-06), each
checked against the code:

- `launch::prepare` checks the target's exe and working directory
  (`Launch::check_target`, `CwdNotFound`) before taking any lock or reaching
  the supervisor. A game on a drive the `before` hook would mount fails
  there first.
- The `Launch` operation lock is shared (`Operation::is_shared`). Only the
  run lock (`Runtime::try_lock`) keeps a second launch out, and it doesn't
  wait: detached `start` gets `AlreadyRunning` and `wineo run` bails with
  "already running". `status` reads "running" from that same lock.
- `supervisor::start` blocks on the supervisor's report line and doesn't
  take a cancel token, so Ctrl+C or the GUI's Cancel can't reach anything
  that runs inside the supervisor before it reports.
- `supervisor::stop` gives up after `stop_timeout` with `StillRunning`, and
  `restart` relies on that.
- For Proton, the configured prefix is the compatdata folder and
  `Runner::wine_prefix` adds `pfx`.

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

### Phases

A launch with hooks goes through three phases. Each one has its own lock
and its own status:

```text
wineo start hades
  1. before   in the launching process (CLI or GUI), before target checks
              exclusive Hook lock: status "busy: before hook"
  2. run      the supervisor, as today: run lock, status "running"
              ... the run ends, playtime session recorded ...
  3. after    the supervisor (or `wineo run`), right after the run ends
              exclusive Hook lock: status "busy: after hook"
```

- **before** runs as the first thing a launch does: after loading the
  config, before `prepare` checks the exe and working directory, and before
  winetricks or component installs. That makes "mount the drive the game
  lives on" work. It runs in the launching process, so Ctrl+C and the GUI's
  Cancel reach it through the token they already pass to `prepare`, and
  nothing new has to cross the supervisor handshake.
- A new `Operation::Hook` lock is exclusive, so a second launch, `exec`, or
  anything else that takes an operation lock fails with the existing busy
  message, like `hades is busy running its before hook`, instead of running
  a second `before` hook. A launch first checks that the environment isn't
  already running, so starting a running game never runs `before`.
- **run** is unchanged.
- **after** runs in whatever owned the run: the supervisor for a detached
  run, `wineo run` for an attached one. It takes the Hook lock before it
  lets go of the run lock, so nothing can start in between. `status` then
  shows the run as ended (with its real end and exit code) and the
  environment as busy, not as running.
- A launch during `after` fails with the busy message rather than waiting.
  `restart` is the exception: it waits for `after` to finish, printing
  `Waiting for the after hook.`, then starts again.
- Playtime stops at the end of the run, so hook time never counts.
- Both hooks run in `$HOME`, since the target's working directory may be on
  the drive `before` is about to mount. A hook that needs another folder can
  `cd` there.
- Each hook runs in its own process group. Canceling or killing a hook
  signals the whole group, so a hook that started a background process
  doesn't leave it behind.

### Output

- `before`: the terminal for the CLI. In the GUI it's captured and shown
  with the error when the hook fails, and dropped when it succeeds.
- `after`: the run log for a detached run, the terminal for `wineo run`.

### Outcomes

`after` runs exactly when the launch got past `before`, like a `finally`
block: if `before` turned notifications off, `after` gets the chance to turn
them back on, however the run ended.

| What happens | `after` runs | `WINEO_END` | `WINEO_EXIT_CODE` |
|---|---|---|---|
| `before` exits non-zero | no | | |
| `before` canceled (Ctrl+C, GUI Cancel) | no | | |
| The program fails to start after `before` succeeded | yes | `failed` | empty |
| The run ends normally | yes | `exited` or `failed` | the program's code |
| `wineo stop` or the GUI's Stop | yes | `stopped` | the program's code, or empty |
| `wineo stop --force` on the program | yes | `stopped` | empty |
| wineo itself dies (a lost run) | no | | |

While a hook runs:

- `before` exits non-zero: nothing starts, and the launch fails with
  `Launch canceled: the before hook exited with code 1.`, in the GUI too.
- `after` exits non-zero: a warning in its output. The run's end and exit
  code don't change.
- `wineo stop` during `before`: the hook's group gets SIGTERM, then SIGKILL
  after `stop_timeout`, and the launch is canceled.
- `wineo stop` during `after`: prints `Waiting for the after hook.` and
  waits with no deadline, since killing a sync halfway is worse than
  waiting. Ctrl+C stops waiting without touching the hook. `wineo stop
  --force` kills the hook's group right away. The run itself has already
  ended by then, so `stop`'s usual deadline only ever applies to the
  program.
- Ctrl+C in `wineo run` during `after`: the first one prints `Waiting for
  the after hook, press Ctrl+C again to kill it.`, and the second kills the
  hook's group.
- The GUI shows the environment as busy during either hook. Its Stop button
  during `after` asks `Kill the after hook?` before killing it.

### Environment variables

Hooks get the user's environment, minus `WINEPREFIX` and any inherited
variable from this table, plus:

| Variable | Value |
|---|---|
| `WINEO_ENVIRONMENT` | The environment name |
| `WINEO_TARGET` | The target name |
| `WINEO_EXE` | The target's exe as configured |
| `WINEO_PREFIX` | Wine only: the Wine prefix the game runs in, so `pfx` inside the compatdata folder for Proton, the same value the game gets as `WINEPREFIX` |
| `WINEO_HOOK` | `before` or `after` |
| `WINEO_END` | `after` only: `exited`, `failed`, or `stopped` |
| `WINEO_EXIT_CODE` | `after` only: the program's exit code, empty if unknown |
| `WINEO_SECONDS` | `after` only: how long the run lasted |

- Variables that don't apply (`WINEO_PREFIX` for a native game, the
  `after`-only ones in `before`) are removed, not left empty, so an inherited
  value can't leak in.
- An inherited `WINEPREFIX` is removed so a hook that runs `wine` doesn't
  quietly use some other prefix. A hook that wants the game's prefix sets
  `WINEPREFIX="$WINEO_PREFIX"`.
- `WINEO_PORTABLE_DIR` is set for a portable environment, so a hook that
  runs `wineo` sees the same folder.
- The environment's `env` table isn't passed, since it's for the game.

### Heroic import

`beforeLaunchScriptPath` and `afterLaunchScriptPath` become `hooks.before`
and `hooks.after`, instead of review warnings. Each path is quoted for `sh`
as one word, with single quotes and `'\''` for any single quote inside, so
spaces, apostrophes, `$`, and `;` in a path can't change what runs. The
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
2. Core hook runner: a `hooks` module that runs one hook with `sh -c` in
   `$HOME`, in its own process group, with the variables above. It takes a
   cancel token and kills the group on cancel (SIGTERM, then SIGKILL after
   `stop_timeout`), and writes output to a given writer. Plus
   `Operation::Hook` and its busy message.
3. Core launch wiring: `before` at the start of `launch::prepare` under the
   Hook lock, with the already-running check first. `after` in
   `Supervisor::wait`'s callers (`serve_run` and `run.rs`), taking the Hook
   lock before the run lock goes, with the outcome table's rules for a
   failed spawn. `stop` and `restart` behavior during `after`, and `stop
   --force` killing the hook's group.
4. CLI and GUI: the canceled-launch line, the waiting messages, Ctrl+C
   twice in `wineo run`, the GUI's busy state, captured `before` output on
   failure, and the GUI's Stop confirmation during `after`.
5. Heroic import: map the two script paths with shell quoting, list them in
   the review, and drop the two "not executed" warnings.
6. Docs: a "Hooks" section in `launch-options.md`, `config-files.md`,
   `downloads-and-imports.md` for Heroic, the `features.md` row, and the
   acceptance checklist.

Step 1 goes first. Steps 2 and 5 can run in parallel after it. Step 3 needs
step 2, and step 4 needs step 3.

## Decisions during implementation

- Step 2 (2026-10-06): the hook's process group, boot ID, and leader start
  time live in the `Hook` operation lock file, so `stop --force` in another
  process can find and verify it with the same checks the supervisor uses.
  A stale file is ignored, since the lock must still be held.
- `busy` stays a string in JSON: `hook-before` and `hook-after`, matching
  the other operation values.
- `status --json` shows hooks as presence only, never the commands, like
  saved launch arguments: `"hooks": {"before": true, "after": false}`, or
  `null` when neither is set. Settled by the maintainer on 2026-10-06; it
  lands with step 4.
- Picking a template that carries hooks shows a `hooks: before, after`
  line in `wineo new`'s summary and the GUI's Add flow, so commands that run
  on every launch are never a surprise. Settled by the maintainer on
  2026-10-06; it lands with step 4.
- A launch that fails after `before` succeeded but before the program
  starts (a missing exe, a failed winetricks or component install) still
  runs `after`, in the launching process since no supervisor exists yet,
  with `WINEO_END=failed` and an empty `WINEO_EXIT_CODE`. Its output goes
  where `before`'s did. Settled by the maintainer on 2026-10-06 during
  step 3.
- Hook locks cover only the environment's name, not its prefix. Hooks
  never touch the prefix, so two environments sharing one never block each
  other's hooks. Settled by the maintainer on 2026-10-06 during step 3.
- From step 3's review (2026-10-06), following the rules already settled:
  an environment without hooks behaves exactly as before, with the same
  signals, messages, exit codes, recorded run state, and lock files, so the
  new launch reservation and admission locks are only taken when a hook is
  set. And `after` belongs to whichever run actually happens: if `before`
  ran but the launch then finds the environment already running (only
  possible with an older wineo holding the run), the live run owns cleanup
  and this launch skips `after`. A lost run is refused before `before`.
- Heroic's script paths are shown as stored, with control characters
  escaped so a path can't fake a review line, and bulk reviews mark a game
  that `runs launch scripts`.
- A hook that exits normally leaves any background process it started
  alone. Only cancel and `--force` kill the hook's group.
- Notes for step 3 from review: `Guard::relabel` can't turn the shared
  `Launch` guard into the exclusive `Hook` lock, so step 3 has to close the
  gap between them without letting another launch in. The GUI's busy text
  (`panel.rs`, `library.rs`) and `launch/stop.rs` print `Operation::as_str()`
  and need proper wording for hooks.

## Acceptance fixes

- Short playtime (2026-10-07). Step 4's 30-second run showed as `0 m` in
  `wineo stats`. The maintainer chose `< 1 m` for any time under a minute,
  in the shared `duration_hm` formatter, so the CLI and GUI both show it.
  A true zero is still `0 m`.

## Testing and acceptance

- Unit tests for the table rules: target replaces environment, `false`,
  templates, and `set`/`--unset`.
- Hook runner tests: the variables for Wine, Proton (`pfx`), and native,
  removal of inherited `WINEPREFIX` and stale `WINEO_*` values,
  `WINEO_PORTABLE_DIR`, the working directory, and a canceled hook whose
  background child is killed with it (no survivor in its group).
- Integration tests with the fake runners and hooks that write marker files:
  - Order: before, program, after, for `start` and `run`.
  - `before` runs before target checks: a hook that creates the exe makes a
    launch of a missing exe succeed.
  - Every row of the outcome table, including a program that fails to spawn
    after `before` succeeded.
  - Locks and status: a second launch during `before` and during `after`
    gets the busy message and runs no hook; `status` shows busy, not
    running, during `after`; starting a running game runs no `before`.
  - Stopping: `stop` during `before` cancels it; `stop` during an `after`
    hook that outlasts `stop_timeout` waits for it; `stop --force` kills it
    and its child; `restart` waits for `after`, then starts.
  - Ctrl+C in `wineo run` during `before`, and twice during `after`.
  - No hooks for `exec`, `winetricks`, or installers.
- GUI: a test for the busy state during a hook, and for Cancel during
  `before` killing the hook.
- Heroic import tests for script paths with spaces, an apostrophe, `$`, and
  `;`, checking the stored hook runs exactly that path.
- Acceptance: a `before` hook that creates a file and an `after` hook that
  removes it, for a Wine game and a native one, through the CLI and the GUI,
  plus a failing `before` hook, a slow `after` hook with `stop` and `stop
  --force`, and a Proton game's `WINEO_PREFIX`.

## Acceptance checklist

Everything but step 11 runs in a throwaway portable folder. In fish:
`set -x WINEO_PORTABLE_DIR /tmp/acc-0046`, and `set -e WINEO_PORTABLE_DIR`
at the end. The test game is a shell script that `before` creates, which
also shows `before` running ahead of wineo's exe check.

1. `printf '#!/bin/sh\nsleep 30\n' > /tmp/acc-0046-game.sh`,
   `chmod +x /tmp/acc-0046-game.sh`, then
   `wineo new -n acc --native --exe /tmp/acc-0046-game.sh`.
2. `wineo set acc 'hooks.before=printf "#!/bin/sh\nsleep 30\n" > /tmp/acc-0046-game.sh && chmod +x /tmp/acc-0046-game.sh'`
   and `wineo set acc 'hooks.after=env | grep WINEO_ > /tmp/acc-0046-env'`.
   `wineo status acc` shows `hooks  before, after`, and `wineo status acc
   --json` has `"hooks":{"before":true,"after":true}`.
3. `rm /tmp/acc-0046-game.sh`, then `wineo start acc` starts it anyway,
   since `before` recreates the script ahead of wineo's exe check.
   `wineo stop acc`, then `/tmp/acc-0046-env` has `WINEO_END=stopped`,
   `WINEO_HOOK=after`, and no `WINEO_PREFIX`.
4. `wineo run acc` and let it finish (30 seconds): the env file now says
   `WINEO_END=exited` and `WINEO_EXIT_CODE=0`. `wineo stats acc --json | jq
   .seconds` counts the 30 seconds plus step 3's run, not the hook time (the
   text view rounds down to whole minutes).
5. `wineo set acc 'hooks.before=echo nope; exit 1'`. `wineo start acc`
   prints `nope`, then `Launch canceled: the before hook exited with code
   1.`, exits 1, and nothing runs.
6. Put the step 2 `before` back, and `wineo set acc 'hooks.after=sleep 60'`.
   Start and stop it: `stop` prints
   `Waiting for the after hook.` In another terminal, `wineo status acc` says
   `running its after hook`, and `wineo start acc` is refused as busy. Ctrl+C
   the waiting `stop`, then `wineo stop --force acc` kills the hook right away.
7. Start it again, then `wineo restart acc`: it waits for the 60-second
   `after`, then starts again. Stop it with `--force`.
8. `wineo run acc`, Ctrl+C once: it asks the game to close and says it's
   waiting for the after hook. Ctrl+C again kills the hook.
9. Set step 5's failing `before` again, then in the app (`wineo-gui`, from
   the same shell) Play `acc`. The error shows `nope` under it. Put the step 2
   `before` back, Play, then Stop: while the 60-second `after` runs, the
   panel says `Running its after hook`, and Stop asks `Kill the after
   hook?`.
10. Add a template with hooks:
    `mkdir -p /tmp/acc-0046/config/templates` and
    `printf '[hooks]\nafter = "true"\n' > /tmp/acc-0046/config/templates/acc.toml`.
    Pick `acc` in `wineo new` and in the app's Add flow: the `new` summary
    lists `after hook` and the app shows a Hooks line. Back out without
    creating anything.
11. On one of your real Proton games:
    `wineo set GAME 'hooks.after=echo $WINEO_PREFIX > /tmp/acc-0046-prefix'`,
    play briefly, and check the path ends in `/pfx`. Then
    `wineo set GAME --unset hooks`.
12. If a Heroic game has before or after launch scripts set, `wineo import
    heroic --dry-run --game ID -n NAME --runner RUNNER` lists them as
    `before hook` and `after hook`.
13. Clean up: `set -e WINEO_PORTABLE_DIR`, then
    `rm -r /tmp/acc-0046 /tmp/acc-0046-*`.

## Scope limits

- No global hooks.
- No hooks for `exec`, `winetricks`, `setup`, or installers.
- No GUI editing of hooks.
- No hook while the program runs (like "on focus"), and no per-hook timeout.
- Store games from the later proxied kind aren't covered. Their plan decides
  whether hooks apply when Steam starts the game.
