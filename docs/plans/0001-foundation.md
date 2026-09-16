# 0001: Foundation and first run

Status: approved

## Goal

Set up the Cargo workspace and build enough of wineo-core and wineo-cli to create an environment with system wine and run notepad in it, attached and detached, with status, logs, stop, and restart working.

## Decisions

Settled by the maintainer:

- Rust with clap, serde, and toml. Crates: wineo-core (library), wineo-cli (the `wineo` binary), wineo-gui (later).
- Verbs: `new`, `start`, `set`, `stop`, `restart`, `status`, `run`, `exec`, `logs`, `config`, plus `list` and `rm` in a later plan.
- Process model: per-app supervisor. `start` spawns a small detached wineo process that owns the run. No daemon, no systemd dependency.
- The environment is the unit that `start`, `stop`, `restart`, and `status` act on. `exec` runs extra programs inside it.
- Templates are copied into an environment when it is created (later plan).
- Environment configs live in `~/.config/wineo/environments/`. Prefixes default to `~/.local/share/wineo/prefixes/<name>`, runners live in `~/.local/share/wineo/runners/`, runtime state in `~/.local/state/wineo/`.
- `set` takes `key=value` pairs with dotted keys that mirror the TOML, plus `--unset key`.
- Env var precedence, lowest to highest: inherited shell env, global `[env]`, environment `[env]`, `-e KEY=VAL`.
- `--runner wine` means the system wine on PATH.
- License: GPL-3.0-or-later.

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
  - core: `serde`, `toml`, `toml_edit` (so `set` keeps comments and formatting in hand-edited files), `thiserror`, `rustix` (setsid and flock without unsafe code), `jiff` (timestamps)
  - cli: `clap` (derive), `anyhow`, `signal-hook` (Ctrl+C in `run`)
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

### Verbs

`wineo new -n <name> [--dir PATH] [--runner NAME] [--arch win64|win32] [--install-exe PATH]`

- Creates the prefix with `wineboot --init`, waits on `wineserver -w`, then writes the environment config.
- A `--dir` that already holds a prefix (has `system.reg`) is adopted as is. A non-empty dir that is not a prefix is refused.
- If creation fails, the prefix dir is removed only if this run created it, and no config is written.
- `--install-exe` runs the installer attached after creation, then prints a `wineo set` hint for adding a target.
- `wineo new` without `-n` errors for now. The guided wizard is a later plan.

`wineo set <name> key=value... [--unset key]...` and `wineo set --global ...`

- Values are parsed by the schema: strings need no quoting, booleans are `true`/`false`, lists use TOML syntax (`'["-a", "-b"]'`).
- The edited document is validated against the schema before it is written.
- `targets.X.default=true` clears `default` on the other targets.
- If the environment is running, prints a note that changes apply on the next start.

`wineo config [name] [--path]`

- Prints the global config, or the environment's config, as stored. `--path` prints the file path instead.

`wineo start <name> [--target T] [-e KEY=VAL]... [-- args]`

- Starts the environment detached (see Supervisor).
- Already running with the same target: prints a note and exits 0, like systemctl. A different target is an error.

`wineo run <name> [--target T] [-e KEY=VAL]... [-- args]`

- Same launch as `start`, but attached. Output goes straight to the terminal and is not logged.
- Holds the same lock and records state, so `status` shows it and `start` won't launch a second copy.
- Ctrl+C runs the stop sequence. A second Ctrl+C forces it.

`wineo exec <name> [-e KEY=VAL]... <exe> [args...]`

- Runs another program attached, with the environment's runner, prefix, and env. It does not take the lock or change status.

`wineo stop <name> [--force]`

- Not running: prints a note and exits 0.
- Otherwise asks apps to close with `wineboot --end-session`, waits up to `stop_timeout`, then runs `wineserver -k`. `--force` goes straight to the kill.

`wineo restart <name> [--target T] [-e KEY=VAL]... [-- args]`

- `stop` then `start`, or just `start` if not running.
- Without options it reuses the previous run's target, `-e` values, and args. Config is re-read, so `set` then `restart` applies changes.

`wineo status [name]`

- With a name:

  ```
  notepad: running since 2026-09-16 14:02:11 (3 min ago)
    target  notepad (notepad)
    runner  wine (wine-11.17)
    prefix  ~/.local/share/wineo/prefixes/notepad
    pid     48213
  ```

- Without a name: one line per environment with name, state, target, and time.
- States: `running`, `exited` (code 0), `failed` (non-zero code, or the supervisor died), `stopped` (by `wineo stop`), `inactive` (never started).

`wineo logs <name> [-f] [-n N] [--previous]`

- Prints the latest detached run's log, all of it by default. `-f` follows, `-n` limits to the last N lines, `--previous` shows the run before.

### Supervisor

`start` re-executes the wineo binary as a hidden `wineo __supervise` and passes the resolved launch spec on stdin. The logic lives in core so a GUI can expose the same entry point.

1. The supervisor calls `setsid` to leave the terminal's session, then takes an exclusive `flock` on the environment's lock file. If the lock is held, it reports "already running" and exits.
2. It rotates the log, spawns wine with stdout and stderr appended to `output.log`, and writes `state.toml`.
3. It reports success, or a spawn error like a missing wine binary, back to `start` over a pipe. `start` prints the result and exits.
4. It waits for the wine process and records its exit code, then waits on `wineserver -w` so launchers that hand off to a child process still count as running.
5. It writes the final state and exits, which releases the lock.

A held lock means running. The kernel drops the lock if the supervisor dies, so stale state can never claim a dead run is alive, and pid reuse doesn't matter.

Runtime state:

```
~/.local/state/wineo/notepad/
  lock           held by the supervisor or `run` while running
  state.toml     target, cli overrides, pids, start and end times, exit code, how it ended
  output.log     latest detached run
  output.log.1   the run before
```

### Runners

Only `runner = "wine"` works in this plan. Any other name errors with a note that runners aren't implemented yet. `status` shows the runner's version.

### Testing

- Unit tests for config parsing, dotted-key edits, env precedence, target resolution, Windows path mapping (against a fake prefix tree), and name validation.
- Tests that need wine are `#[ignore]` and run with `cargo test -- --ignored`.
- `cargo fmt` and `cargo clippy --all-targets -- -D warnings` stay clean. Once the skeleton exists, these commands go into AGENTS.md.

## Phases

1. Skeleton, paths, config, `new`, `set`, `config`, `run`, `exec`. Checkpoint: `wineo run notepad` opens notepad.
2. Supervisor, `start`, `stop`, `restart`, `status`, `logs`, `--install-exe`.

Each phase ends with tests, fmt, and clippy passing, then a commit and push.

## Done when

With wine installed, this works end to end:

```
wineo new -n notepad
wineo set notepad targets.notepad.exe=notepad
wineo config notepad
wineo run notepad          # notepad opens, Ctrl+C closes it
wineo start notepad        # returns right away, notepad opens
wineo status               # lists notepad as running
wineo logs notepad -f
wineo restart notepad
wineo stop notepad         # notepad closes, status says stopped
wineo start notepad        # close notepad by hand, status says exited
wineo exec notepad winecfg
```

## Deferred to later plans

- Guided `wineo new` wizard
- Templates, gamemode, DXVK, vkd3d
- Custom runners in `runners/`, Proton through umu-launcher, runner downloads (GE-Proton, Proton-CachyOS)
- wineo-gui
- Portable layout with the config stored inside the prefix
- `list` and `rm` verbs, including how `list` relates to `status` without a name

## Open questions

Proposed answers in parentheses.

1. Mono and Gecko install prompts during prefix creation: suppress them, or rely on distro packages like `wine-mono` and `wine-gecko`? (rely on packages)
2. Apps that register Windows services, like Adobe's updater, can keep a prefix alive after their window closes, so `status` would stay `running`. Add a per-environment option to track only the launched process? (not now, revisit if it shows up)
3. Arch's wine 11.17 is a pure WoW64 build and probably can't create win32 prefixes. Keep `--arch` and fail with a clear error when unsupported? (yes)
4. `list` and `rm` aren't in the initial verbs. (decided: both are verbs, built in a later plan)
5. A `--json` output flag for scripting? (later, the GUI uses core directly)

## Verified with wine 11.17

- `wine wineboot --init` exits after about 4s, but `system.reg` and the other registry files only appear once `wineserver -w` returns, about 7s later. Prefix detection has to wait for it.
- `wineserver -w` returns immediately when no wineserver is running, and does not create a missing prefix dir.
- `wine wineboot --end-session` closed notepad cleanly (exit code 0) in about 150ms. The prefix went idle about 3s later.
- After `wineserver -k` the launched process also exits with code 0, so exit codes can't tell a kill from a normal exit. `stopped` has to come from the stop request.
- `WINEARCH=win32` fails with `WINEARCH is set to 'win32' but this is not supported in wow64 mode.` and exit code 1, without creating the prefix dir.
- A bare name like `notepad` runs through `start.exe /exec`, which waits for the program. A full path runs the program directly in the launched process. Console exit codes pass through.
- SIGINT sent to `wine notepad` did not close it, so Ctrl+C in `run` has to go through the stop sequence.
