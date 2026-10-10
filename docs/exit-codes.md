# Exit codes

Every `wineo` command ends with one of these:

| Code | Meaning |
|---|---|
| 0 | It worked, including when there was nothing to do, like `list --tag nope` or `stop` on something that isn't running. |
| 1 | It failed. The reason is on stderr, usually as `error: ...`, or in the `errors` array with `--json`. |
| 2 | The command line was wrong: an unknown flag, a missing argument, or two flags that don't go together. Nothing ran. |
| 130 | It was canceled and wineo did nothing more: Ctrl+C, Esc or `q` at the first question of a guided flow, or answering no to a final confirmation. |
| 141 | Whatever was reading wineo's output went away, like `wineo list \| head -1`. Wineo stops quietly. |
| N | `run`, `exec`, and `winetricks`, and installers from `new --install-exe` or `setup`, end with the program's own exit code. A program killed by a signal shows as 128 plus the signal, so 137 for SIGKILL. |

`--help` and `--version` exit 0.

## Partial results

`list`, `status` without a name, `stats` without a name, and `validate`
print everything they could read and exit 1 if any environment couldn't be
read. With `--json`, the readable ones are in the output and the rest are in
`errors`. See [JSON output](json-output.md).

`validate` also exits 1 when an error-level problem is left after checking
and fixing; warnings alone exit 0. See [Validate](validate.md).

`import heroic --all` imports what it can and exits 1 if any game failed,
after a summary of what did and didn't import. Its [dry run](dry-run.md)
does the same for games that would fail.

## Canceling

Ctrl+C exits 130 wherever wineo is waiting for you or doing something it
can stop safely: guided flows, copies, downloads, prefix creation,
winetricks, and launches before the program starts. A canceled copy or
import cleans up after itself. Commands that finish quickly, like `rm` or
`trash`, just end, and the shell reports 130 all the same.

A few things to know:

- `wineo run` hands Ctrl+C to the game once it's running, asking it to close.
  The exit code is then the game's own, not 130.
- `import heroic --all` stops the whole batch on Ctrl+C. Games already
  imported stay imported.
- Answering no at a final confirmation, like `prune`'s or `snapshot
  restore`'s, exits 130, since nothing was done. With `--yes` there is no
  question.
- Esc goes back a question everywhere except the first one, where there's
  nowhere to go back to, so it cancels. The key hint says so: `Esc or Ctrl+C
  to cancel.`

## Commands worth knowing about

- `start` exits 0 when the environment is already running, and says so. `run`
  exits 1 in that case, since it can't attach to a run it didn't start.
- `status NAME` exits 0 whether or not the environment is running. Use
  `status NAME --json` and its `state` to tell.
- `stop` exits 0 when nothing was running, and 1 when it couldn't confirm a
  lost run or installer stopped.
- `new --exe PATH` with a missing program still creates the environment,
  says so, and exits 1, so a script notices the target wasn't added.
- `set` can exit 1 after the config was already saved, when a later step like
  updating desktop entries failed. The message says which.
- `art fetch --only KIND` exits 1 when nothing of that kind was found. Without
  `--only`, finding nothing is not an error.
- An installer's own exit code is passed through, so an installer that exits
  1 or 130 looks like a wineo failure or cancel. The message on stderr says
  which it was.
- `run` on a [Steam game](steam.md) exits 0 when the game exits on its own,
  130 when it's stopped, and 1 when Steam never starts it, since the game's
  own exit code can't be collected.
- `install -f` and `uninstall -f` exit 0 when Steam finishes, 1 when Steam
  cancels partway, and 130 on Ctrl+C, which stops following but never
  Steam's work. Steam canceling prints `Steam canceled the install.` without
  an `error:` prefix, since nothing in wineo failed. Without `-f`, and for a no-op like `Already installed.`,
  they exit 0 once the request is handed to Steam.
- The process wineo starts for a detached run never reports the game's exit
  code. It's recorded with the run instead: see `status NAME`.

Exit codes are part of what wineo promises to keep stable; see the 1.0
criteria in [Features](features.md#10-criteria).
