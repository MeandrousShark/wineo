# 0001: Foundation and first run

Status: approved

## Goal

Set up the Cargo workspace and build enough of wineo-core and wineo-cli to create an environment with system wine and run notepad in it, attached and detached, with status, logs, stop, and restart working.

## Decisions

Settled by the maintainer:

- Rust with clap, serde, and toml. Crates: wineo-core (library), wineo-cli (the `wineo` binary), wineo-gui (later).
- Verbs: `new`, `start`, `set`, `stop`, `restart`, `status`, `list`, `run`, `exec`, `logs`, `config`, plus `rm` in a later plan.
- Process model: per-app supervisor. `start` spawns a small detached wineo process that owns the run. No daemon, no systemd dependency.
- The environment is the unit that `start`, `stop`, `restart`, and `status` act on. `exec` runs extra programs inside it.
- Templates are copied into an environment when it is created (later plan).
- Environment configs live in `~/.config/wineo/environments/`. Prefixes default to `~/.local/share/wineo/prefixes/<name>`, runners live in `~/.local/share/wineo/runners/`, runtime state in `~/.local/state/wineo/`.
- `set` takes `key=value` pairs with dotted keys that mirror the TOML, plus `--unset key`.
- Env var precedence, lowest to highest: inherited shell env, global `[env]`, environment `[env]`, `-e KEY=VAL`.
- `--runner wine` means the system wine on PATH.
- License: GPL-3.0-or-later.
- `status` without a name shows only what's running. `list` shows every environment with its state, and is built in phase 2.
- Wine's own menu entries and file associations are turned off, both with `WINEDLLOVERRIDES` on every launch and with a registry override written into the prefix.
- `restart` without options reuses the previous run's options. Any option means a fresh start with only what was passed.
- The supervisor records when the launched program exits separately from when the prefix goes idle, and `status` shows it.

## Proposed details

Everything in this section is a proposal for review.

### Workspace

```
Cargo.toml          workspace members, shared dependencies, lints
crates/
  wineo-core/       library
  wineo-cli/        binary named wineo
docs/plans/
LICENSE
```

- Edition 2024, Linux only.
- wineo-gui is not created until a toolkit is picked.
- Dependencies:
  - core: `serde`, `toml`, `toml_edit` (so `set` keeps comments and formatting in hand-edited files), `thiserror`, `rustix` (setsid, flock, kill, and dup2 without unsafe code), `jiff` (timestamps), `signal-hook` (stop signals in the run loop)
  - cli: `clap` (derive), `anyhow`, `signal-hook` (Ctrl+C in `exec`)
  - dev: `tempfile`
- XDG base dirs are resolved by hand, honoring `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, and `XDG_STATE_HOME`. Tests use this to point everything at a temp dir.

### Config files

Global config, every key optional:

```toml
# ~/.config/wineo/config.toml
stop_timeout = 10        # seconds before stop escalates to a kill

[defaults]               # used by `wineo new`
runner = "wine"
arch = "win64"
prefix_root = "~/.local/share/wineo/prefixes"

[env]                    # applied to every environment at launch
WINEDEBUG = "-all"
```

Environment config:

```toml
# ~/.config/wineo/environments/notepad.toml
prefix = "~/.local/share/wineo/prefixes/notepad"
runner = "wine"
arch = "win64"

[env]
WINEDEBUG = "fixme-all"

[targets.notepad]
exe = "notepad"
args = []                # optional
cwd = 'C:\users\Public'  # optional, defaults to the exe's directory
default = true           # optional
```

- The environment name is the file name. Environment and target names allow letters, digits, `-`, and `_`.
- Unknown keys are errors, so typos surface instead of being silently ignored.
- Paths under `$HOME` are stored as `~/...` so configs survive a username change or a move to another machine.
- `WINEPREFIX` in any `[env]` table is an error since wineo owns it.
- `arch` only matters when the prefix is created, so `set` refuses to change it afterwards.

### Target resolution

`--target` if given, else the target with `default = true`, else the only target if there is exactly one, else an error listing the targets. More than one `default = true` is a config error.

### Launch

- Command: `<runner wine> <exe> <args...>`, with any `-- args` from the command line appended.
- `WINEPREFIX` is always set. `WINEARCH` is only set while creating the prefix.
- `start`, `run`, and `exec` refuse to run if the prefix dir is missing, instead of letting wine silently create a fresh one.
- Working directory: the target's `cwd`, else the exe's directory, else the prefix's `drive_c`. Windows paths map to Unix paths through the prefix's `dosdevices` symlinks, matching components case-insensitively.
- Exes given as a path are checked before launch so typos fail fast. Bare names like `notepad` or `winecfg` go straight to wine.
- `.msi` files run through `msiexec /i`.

### Menu entries

Wine's menu builder writes file associations every time a prefix starts, and menu and desktop entries when installers create shortcuts. wineo turns it off and will write its own entries later (see `docs/features.md`).

- Every wine command wineo runs adds `winemenubuilder.exe=d` to `WINEDLLOVERRIDES`, unless the value from the shell or the config already mentions `winemenubuilder`, so users can opt back in.
- `new` writes `"winemenubuilder.exe"=""` (disabled) under `HKCU\Software\Wine\DllOverrides` in the prefix it creates or adopts, so wine runs outside wineo don't make entries either. For a new prefix this happens right after `wineboot --init`, before wine shuts down.

### Verbs

`wineo new -n <name> [--dir PATH] [--runner NAME] [--arch win64|win32] [--install-exe PATH]`

- Creates the prefix with `wineboot --init`, waits on `wineserver -w`, then writes the environment config.
- A `--dir` that already holds a prefix (has `system.reg`) is adopted as is. A non-empty dir that is not a prefix is refused.
- If creation fails, the prefix dir is removed only if this run created it, and no config is written.
- `--install-exe` runs the installer attached after creation, like `exec`, then prints a `wineo set` hint for adding a target.
  - A Unix path is checked before the prefix is created, so a typo fails before the wait.
  - After the installer exits, `new` waits for the prefix to go idle, since installers often hand off to another process.
  - If the installer fails, the environment is kept and `new` exits with the installer's code.
- `wineo new` without `-n` errors for now. The guided wizard is a later plan.

`wineo set <name> key=value... [--unset key]...` and `wineo set --global ...`

- Values are parsed by the schema: strings need no quoting, booleans are `true`/`false`, lists use TOML syntax (`'["-a", "-b"]'`).
- The edited document is validated against the schema before it is written.
- `targets.X.default=true` clears `default` on the other targets.
- If the environment is running, prints a note that changes apply on the next start.

`wineo config [name] [--path]`

- Prints the global config, or the environment's config, as stored. `--path` prints the file path instead.

`wineo start <name> [--target T] [-e KEY=VAL]... [-- args]`

- Starts the environment detached (see Supervisor) and prints `Started <name>` once wine is running. Spawn errors, like a missing wine binary, are reported by `start` itself.
- Already running with the same target: prints a note and exits 0, like systemctl. If options were passed, the note says they weren't applied and points at `restart`. A different target is an error.

`wineo run <name> [--target T] [-e KEY=VAL]... [-- args]`

- Same launch as `start`, but attached. Output goes straight to the terminal and is not logged.
- Holds the same lock and records state, so `status` shows it and `start` won't launch a second copy.
- Ctrl+C runs the stop sequence. A second Ctrl+C forces it.
- `wineo stop` from another terminal stops it the same way Ctrl+C does.

`wineo exec <name> [-e KEY=VAL]... <exe> [args...]`

- Runs another program attached, with the environment's runner, prefix, and env. It does not take the lock or change status.

`wineo stop <name> [--force]`

- Not running: prints a note and exits 0.
- Otherwise asks apps to close with `wineboot --end-session`, waits up to `stop_timeout`, then runs `wineserver -k`. `--force` goes straight to the kill.
- The process that owns the run (the supervisor, or `run`) carries out the stop, so it can record `stopped` (see Stopping). `stop` waits for the run to end, then prints `Stopped <name>`.
- If the supervisor died while the run was active, `stop` kills whatever is left in the prefix with `wineserver -k`.

`wineo restart <name> [--target T] [-e KEY=VAL]... [-- args]`

- `stop` then `start`, or just `start` if not running. A run that was attached comes back detached.
- Without options it reuses the previous run's `--target`, `-e` values, and args. Any option means a fresh start with only what was passed, same as `start`.
- Config is re-read, so `set` then `restart` applies changes. A run that used the default target restarts with whatever the default is now.

`wineo status [name]`

- With a name:

  ```
  notepad: running since 2026-09-16 14:02:11 (3 min ago)
    target  notepad (notepad)
    runner  wine (wine-11.17)
    prefix  ~/.local/share/wineo/prefixes/notepad
    pid     48213
  ```

  Once the launched program has exited but other processes keep the prefix running:

  ```
    pid     48213, exited 1 min ago with code 0
            other processes are keeping the prefix running
  ```

  Runs started with `run` say `running attached since`. Finished runs change the first line and drop the pid:

  ```
  notepad: exited with code 0 at 2026-09-16 14:05:02 (1 min ago)
  notepad: failed with code 3 at 2026-09-16 14:05:02 (1 min ago)
  notepad: stopped at 2026-09-16 14:05:02 (1 min ago)
  notepad: failed, wineo lost track of the run started at 2026-09-16 14:02:11 (3 min ago)
  notepad: inactive
  ```

- Without a name: one line per running environment with name, state, target, and time, or `Nothing is running`.

  ```
  notepad    running   notepad    since 14:02 (3 min ago)
  ```

- States: `running`, `exited` (code 0), `failed` (non-zero code, or the supervisor died), `stopped` (by `wineo stop`, Ctrl+C, or a signal), `inactive` (never started).
- The output is built from a struct in core, so `--json` stays cheap later.

`wineo list`

- One line per environment, running or not, with the same columns as `status` without a name. Configs aren't parsed, so a broken one still shows up.

  ```
  notepad    running   notepad    since 14:02 (3 min ago)
  photoshop  exited    photoshop  ended 16 h ago
  winecfg    inactive
  ```

`wineo logs <name> [-f] [-n N] [--previous]`

- Prints the latest detached run's log, all of it by default. `-f` follows, `-n` limits to the last N lines, `--previous` shows the run before.
- `-f` exits once the run ends. It can't be combined with `--previous`.
- No log yet: prints a note and exits 0. If the environment is running attached, a note says its output is in that terminal.

### Supervisor

`start` re-executes the wineo binary as a hidden `wineo __supervise` and passes the resolved launch spec on stdin. The logic lives in core so a GUI can expose the same entry point. `run` goes through the same run loop, attached to the terminal instead of a log.

1. The supervisor calls `setsid` to leave the terminal's session, then takes an exclusive `flock` on the environment's lock file and writes its pid into it. If the lock is held, it reports "already running" and exits.
2. It rotates the log, spawns wine with stdout and stderr appended to `output.log`, and writes `state.toml`.
3. It reports success, or a spawn error like a missing wine binary, back to `start` over its stdout. `start` prints the result and exits. From then on the supervisor's own messages, like a stop request, go to `output.log` with a `wineo:` prefix.
4. It waits for the wine process and records its exit code and time, then waits on `wineserver -w` so launchers that hand off to a child process still count as running.
5. It writes the final state, clears its pid from the lock file, and exits, which releases the lock.

A held lock means running. The kernel drops the lock if the supervisor dies, so stale state can never claim a dead run is alive. If `state.toml` still says running but nothing holds the lock, the supervisor died and the run counts as `failed`.

Stopping:

- SIGINT, SIGTERM, and SIGHUP ask the lock holder for a clean stop, and a second one forces it. SIGUSR1 forces it right away. Ctrl+C in `run`, `wineo stop`, and a plain `kill` all behave the same.
- `wineo stop` reads the holder's pid from the lock file and sends SIGTERM, or SIGUSR1 with `--force`. It then waits for the lock to be released, and gives up with an error after `stop_timeout` plus 10 seconds.

Runtime state:

```
~/.local/state/wineo/notepad/
  lock           held by the supervisor or `run` while running, holds its pid
  state.toml     target, cli overrides, wine's pid, start, program exit, and end times, exit code, how it ended
  output.log     latest detached run
  output.log.1   the run before
```

### Runners

Only `runner = "wine"` works in this plan. Any other name errors with a note that runners aren't implemented yet. `status` shows the runner's version.

### Testing

- Unit tests for config parsing, dotted-key edits, env precedence, target resolution, Windows path mapping (against a fake prefix tree), and name validation.
- Tests that need wine are `#[ignore]` and run with `cargo test -- --ignored`.
- CLI tests drive `start`, `status`, `list`, `logs`, `restart`, and `stop` against fake `wine` and `wineserver` scripts on PATH, so the supervisor is covered without wine installed.
- `cargo fmt` and `cargo clippy --all-targets -- -D warnings` stay clean. Once the skeleton exists, these commands go into AGENTS.md.

## Phases

1. Skeleton, paths, config, `new`, `set`, `config`, `run`, `exec`. Checkpoint: `wineo run notepad` opens notepad.
2. Supervisor, `start`, `stop`, `restart`, `status`, `list`, `logs`, `--install-exe`, and turning off wine's own menu entries (wineo will make its own, see `docs/features.md`).

Each phase ends with tests, fmt, and clippy passing, then a commit and push.

## Done when

With wine installed, this works end to end:

```
wineo new -n notepad
wineo set notepad targets.notepad.exe=notepad
wineo config notepad
wineo run notepad          # notepad opens, Ctrl+C closes it
wineo start notepad        # returns right away, notepad opens
wineo status               # shows notepad running
wineo list                 # lists every environment with its state
wineo logs notepad -f
wineo restart notepad
wineo stop notepad         # notepad closes, status says stopped
wineo start notepad        # close notepad by hand, status says exited
wineo exec notepad winecfg
```

None of it creates wine menu entries or file associations.

## Deferred to later plans

Tracked in [docs/features.md](../features.md).

## Open questions

Proposed answers in parentheses.

1. Mono and Gecko install prompts during prefix creation: suppress them, or rely on distro packages like `wine-mono` and `wine-gecko`? (supress by default, but use distro packages if installed. ability to bypass distro packages in config setting)
2. Apps that register Windows services, like Adobe's updater, can keep a prefix alive after their window closes, so `status` would stay `running`. Add a per-environment option to track only the launched process? (not now, revisit if it shows up. Phase 2 records when the launched program exits, and `status` says when other processes keep the prefix running.)
3. Arch's wine 11.17 is a pure WoW64 build and probably can't create win32 prefixes. Keep `--arch` and fail with a clear error when unsupported? (yes)
4. `list` and `rm` aren't in the initial verbs. (decided: both are verbs, `list` is built in phase 2 and `rm` in a later plan)
5. A `--json` output flag for scripting? (later, the GUI uses core directly)

## Verified with wine 11.17

- `wine wineboot --init` exits after about 4s, but `system.reg` and the other registry files only appear once `wineserver -w` returns, about 7s later. Prefix detection has to wait for it.
- `wineserver -w` returns immediately when no wineserver is running, and does not create a missing prefix dir.
- `wine wineboot --end-session` closed notepad cleanly (exit code 0) in about 150ms. The prefix went idle about 3s later.
- After `wineserver -k` the launched process also exits with code 0, so exit codes can't tell a kill from a normal exit. `stopped` has to come from the stop request.
- `WINEARCH=win32` fails with `WINEARCH is set to 'win32' but this is not supported in wow64 mode.` and exit code 1, without creating the prefix dir.
- A bare name like `notepad` runs through `start.exe /exec`, which waits for the program. A full path runs the program directly in the launched process. Console exit codes pass through.
- SIGINT sent to `wine notepad` did not close it, so Ctrl+C in `run` has to go through the stop sequence.
