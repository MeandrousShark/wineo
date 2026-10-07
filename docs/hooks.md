# Hooks

```toml
# environments/hades.toml
[hooks]
before = 'mount /mnt/games'
after = 'rclone sync ~/Saves/Hades remote:saves/hades'
```

A hook is a shell command wineo runs before a target starts or after its
run ends, like mounting the drive a game lives on, syncing saves, or
turning notifications off while you play. Each one runs with `sh -c`, so
pipes, `&&`, and `~` work as they do in a terminal.

## Where they go

`[hooks]` goes on an environment, a target, or a template, with `before`,
`after`, or both. A target's table replaces the environment's whole table,
like [gamescope](launch-options.md#gamescope), and `hooks = false` on a
target turns them off for it:

```toml
[hooks]
before = 'mount /mnt/games'
after = 'umount /mnt/games'

[targets.editor.hooks]
after = 'notify-send "Editor closed"'   # no before hook for the editor

[targets.server]
exe = 'C:\Game\Server.exe'
hooks = false
```

From the command line:

```sh
wineo set hades 'hooks.before=mount /mnt/games'
wineo set hades --unset hooks.after
wineo set hades targets.server.hooks=false
```

A template's hooks are copied into the environments made from it, and
`wineo new` and the desktop app's Add flow show them when you pick the
template. There are no global hooks.

Hooks run for game launches: `start`, `run`, `restart`, desktop entries,
and the desktop app's Play. They never run for `exec`, `winetricks`,
`setup`, or installers. Wine and native environments work the same.

## When they run

```text
before   first thing, before wineo checks the exe and its folder
run      the game, as usual
after    once the run ends, after its playtime is recorded
```

`before` runs before anything else about the launch is checked, so a hook
that mounts the drive a game lives on works. Both hooks run in your home
folder, since the game's folder may not exist yet; `cd` somewhere else if a
hook needs to.

While a hook runs, the environment is busy: `wineo status` says `running
its before hook` or `running its after hook`, and another launch of it is
refused with that message. `restart` waits for `after` to finish, then
starts again. Hook time never counts as played.

`after` runs whenever the launch got past `before`, like a `finally` block,
so a hook that turned something off gets the chance to turn it back on:

| What happened | `after` runs | `WINEO_END` |
|---|---|---|
| `before` failed or was canceled | no | |
| The launch failed after `before`, say the exe was missing | yes | `failed` |
| The game ended | yes | `exited` or `failed` |
| `wineo stop` or Stop in the app | yes | `stopped` |
| `wineo stop --force` | yes | `stopped` |
| wineo itself died, a lost run | no | |

If the game turns out to be running already, which only happens when an
older wineo started it, the launch skips `after` and leaves cleanup to
that run.

## Failures and stopping

A `before` hook that exits with anything but 0 cancels the launch:

```
$ wineo start hades
Launch canceled: the before hook exited with code 1.
```

The desktop app shows the same error with the hook's last lines of output.
A failing `after` hook only prints a warning; the run's own result doesn't
change.

- `wineo stop`, Ctrl+C, or Cancel in the app during `before` stops the hook
  and cancels the launch. The hook gets SIGTERM, then SIGKILL after
  `stop_timeout`, along with anything it started.
- `wineo stop` during `after` waits for it, since cutting a sync off halfway
  is worse than waiting: it prints `Waiting for the after hook.` Ctrl+C
  stops waiting and leaves the hook running. `wineo stop --force` kills it.
  The desktop app's Stop asks before killing it.
- `wineo stop --force` while the program is still running kills the program,
  but `after` still runs, so cleanup like turning notifications back on
  still happens. It says so, and a second `stop --force` kills the hook.
- In `wineo run`, the first Ctrl+C during `after` says it's waiting, and a
  second one kills the hook. If Ctrl+C already stopped the program, it says
  it's waiting as soon as `after` starts, so the next one kills it.
- A hook that exits normally leaves alone anything it started in the
  background.

## Output

`before` prints to the terminal for `wineo start` and `wineo run`. The
desktop app keeps it and only shows it if the hook fails. `after` writes to
the run log (`wineo logs NAME`) for `start`, and to the terminal for `wineo
run`.

## Variables

Hooks get your environment, minus `WINEPREFIX`, plus:

| Variable | Value |
|---|---|
| `WINEO_ENVIRONMENT` | The environment name |
| `WINEO_TARGET` | The target name |
| `WINEO_EXE` | The target's exe as configured |
| `WINEO_PREFIX` | Wine only: the prefix the game runs in, so the `pfx` folder for Proton |
| `WINEO_HOOK` | `before` or `after` |
| `WINEO_END` | `after` only: `exited`, `failed`, or `stopped` |
| `WINEO_EXIT_CODE` | `after` only: the game's exit code, empty if it's unknown |
| `WINEO_SECONDS` | `after` only: how long the run lasted |
| `WINEO_PORTABLE_DIR` | For a [portable](portable.md) environment, so a hook that runs `wineo` sees the same folder |

Variables that don't apply are removed rather than left empty. The
environment's `env` table isn't passed, since it's for the game. A hook
that wants to run `wine` in the game's prefix can set
`WINEPREFIX="$WINEO_PREFIX"`.

## Elsewhere

- `wineo status NAME` shows a `hooks` line, and `status --json` has
  `"hooks": {"before": true, "after": false}`, or `null` when neither is
  set. The commands themselves are never printed.
- [Heroic import](downloads-and-imports.md#import-a-manually-added-heroic-game)
  carries Heroic's before and after launch scripts over as hooks, and the
  review lists them.
- `wineo validate` doesn't check hooks, since a shell command can't be
  checked without running it.
- A [dry run](dry-run.md) never runs hooks.
