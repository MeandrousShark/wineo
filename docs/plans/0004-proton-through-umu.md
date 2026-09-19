Status: done

# 0004: Proton through umu-launcher

## Goal

Run games on Proton by installing a Proton build as a runner and letting umu-launcher do the work: the Steam Linux Runtime container, protonfixes, and Proton's own prefix layout.

## Decisions

Settled by the maintainer:

- A Proton environment's `prefix` is the compatdata directory, the way Heroic and Steam use it. The real wine prefix is `pfx/` inside it, and wineo appends that itself wherever it needs one. Carried over from 0003.
- A Proton run is over when `umu-run` exits. `PROTON_VERB` defaults to `waitforexitandrun`, so umu already waits for the game. `wineo stop` signals the `umu-run` process group, then forces it after `stop_timeout`. Nothing talks to a wineserver inside the container.
- `exec` and `winetricks` go through umu too: `umu-run <program>` and `umu-run winetricks <verbs>`. Tools then run in the same container and Proton build as the game, so what winetricks installs is what the game sees.
- Proton comes only from wineo's runners root, and `PROTONPATH` is always an absolute path wineo resolved. umu never downloads a Proton build during a launch. Runner downloads stay their own `features.md` row.
- An environment can set `gameid` and `store`, unset by default, so umu applies protonfixes when asked to.
- The Steam Linux Runtime stays on umu's own update schedule. Wineo does not set `UMU_RUNTIME_UPDATE`, and `wineo new` says the first run downloads the runtime.
- A detached run records the process group it started, next to the pid it already records, so a lost Proton run can still be cleaned up once its supervisor is gone.

## Details

Everything in this section is a proposal for review.

### Detecting a Proton runner

- A directory in the runners root is a Proton runner when it has an executable `proton` script at its root. That is exactly what `PROTONPATH` has to point at, so it is the honest test.
- The wine-build rule from 0003 is unchanged: `bin/wine` and `bin/wineserver`, both executable. Proton is checked first, since a Proton tree also contains wine binaries under `files/bin/`.
- `wineo runners` gains the kind column 0003 deferred, now that there is more than one kind.
- `umu-run` missing from `PATH` is only an error when something actually needs it, so `wineo runners` still lists Proton runners on a machine without umu installed.

### What a runner can do

Because every Proton operation goes through umu, wineo never runs Proton's own `files/bin/wine`. That makes the two kinds genuinely different rather than two paths to the same binaries, so `Runner` stops being a pair of executable paths:

```rust
pub struct Runner { name: String, kind: Kind }

enum Kind {
    Wine { wine: PathBuf, wineserver: PathBuf },
    Proton { root: PathBuf },
}
```

- Callers stop reaching for `wine_path()` and `wineserver_path()` and ask the runner for what they want instead: a command to run a program in a prefix, a way to create a prefix, and how a run ends. A wine runner answers with today's behaviour; a Proton runner answers with umu.
- `wineo winetricks` currently sets `WINE` and `WINESERVER` for the system winetricks binary. That stays exactly as it is for wine runners. A Proton runner ignores it and runs `umu-run winetricks` instead, so `winetricks` on `PATH` is not required at all.
- `Runner::version` reads Proton's own `version` file rather than running anything, since running wine is no longer part of the abstraction. A missing or unreadable file means no version, the same as today.

### The prefix

- A prefix is no longer just a path. Resolving one needs the runner, since the wine prefix is the directory itself for wine and `pfx/` inside it for Proton.
- Everything that means "the directory wineo owns" keeps using the configured path: `rm --delete-prefix`, the shared-prefix check, operation locks, and the `prefix` shown by `status`.
- Everything that means "the wine prefix" uses the resolved one: `is_prefix`, and anything setting `WINEPREFIX` for a wine runner.
- umu is given the configured path as `WINEPREFIX`, not the resolved one. umu and Proton create `pfx/` underneath it themselves.
- A prefix that is not yet created is still recognized by its parent directory being absent, so adoption and `new` behave as they do now.

### Creating a prefix

- `umu-run ""` with `WINEPREFIX` and `PROTONPATH` set creates the prefix, which is the documented way to do it. Wineo runs it with output going to `create.log`, the same as `wineboot --init` does now.
- The first ever umu run downloads the Steam Linux Runtime into `~/.local/share/umu`, which takes a while. `wineo new` says so before it starts, the way it already warns that creating a wine prefix takes a few seconds.
- Adoption works on a compatdata directory that already has a `pfx/`, so a Steam or Heroic Proton prefix can be adopted with `--dir` as-is.
- The menu builder override from 0001 is applied inside `pfx/` after creation, so a Proton prefix does not scatter desktop entries either.

### Launching

- The launch command becomes `umu-run <program> <args...>` with:
  - `WINEPREFIX` set to the configured prefix directory.
  - `PROTONPATH` set to the runner's absolute root.
  - `GAMEID` set from the environment's `gameid`, left unset when it has none so umu uses `umu-default` and applies no fixes.
  - `STORE` set from the environment's `store`, left unset otherwise.
  - The merged `[env]` tables and `-e` values, exactly as now.
- `PROTON_VERB` is left at umu's default, `waitforexitandrun`, so `umu-run` does not return until the game does.
- `UMU_RUNTIME_UPDATE` is left alone, so umu keeps the Steam Linux Runtime current on its own terms. A launch can therefore reach the network; anyone who wants it not to can set the variable in the environment's `[env]` table.
- Wineo owns `WINEPREFIX`, `PROTONPATH`, `GAMEID`, and `STORE` for a Proton launch. Explicit `[env]` or `-e` assignments to them are rejected, the same way `wineo winetricks` already rejects its managed variables.
- gamemode still wraps the launch when it is on, so the command becomes `gamemoderun umu-run <program>`.
- A target's `exe` resolves the same way it does for wine. Windows paths are resolved against `pfx/`.

### The run lifecycle

- `umu-run` is spawned in its own process group, so its whole tree can be signalled as one. Attached runs already do this; detached runs start doing it too.
- The run ends when `umu-run` exits. There is no separate wait for the prefix to go idle, because there is no wineserver wineo can reach.
- `stop` sends SIGTERM to the process group, then SIGKILL once `stop_timeout` passes. `stop --force` goes straight to SIGKILL. The existing `Stopping` and `Killing` notices still describe it.
- `status` reports the program's exit and the run's end at the same moment for a Proton run, since they are the same event. The separate `program_exited_at` from 0001 stays null.
- A lost Proton run cannot be cleaned up with `wineserver -k`, so the run state records the process group alongside the pid. `stop` kills that group, and says so when it is already gone. The field is written for every detached run, not just Proton ones, so the state file has one shape.

### `exec` and `winetricks`

- `wineo exec <name> <program>` runs `umu-run <program>`, holding the same shared operation lock as now.
- `wineo winetricks <name> <verbs...>` runs `umu-run winetricks <verbs...>`. umu's winetricks support requires GE-Proton or UMU-Proton, so a Proton build without it fails inside umu; wineo passes that failure through rather than trying to detect it.
- Both keep the existing refusals: a running environment, a missing prefix, and a lost run.
- `winetricks` on `PATH` is only required for wine runners. The error saying it was not found is not reachable for a Proton environment.

### protonfixes settings

```toml
# ~/.config/wineo/environments/hades.toml
gameid = "umu-hades"
store = "gog"
```

- Both are optional strings on an environment, settable and unsettable with `wineo set`. Neither has a global default: they identify one game.
- Values are passed to umu as given. umu treats an unknown id as "no fixes", so wineo does not validate against the umu database.
- Both are rejected on a wine environment, since nothing would read them.
- `status <name>` shows a `gameid` line when one is set.

### Changing a runner

- 0003's `set runner=` prints a note that the prefix is not migrated. Crossing kinds is worse than that: a wine prefix has no `pfx/`, and a Proton prefix's contents are not where a plain wine expects them.
- So `set <name> runner=<other kind>` is refused outright, saying the two prefix layouts are not compatible and that a new environment is the way to switch.
- `wineo new --runner` is unaffected, since it creates the prefix to match.

### Testing

- Runner detection: a Proton tree, a wine build, a Proton tree missing its `proton` script, and a directory with both layouts. `wineo runners` shows the kind for each, and `--all` still explains the skips.
- A fake `umu-run` on `PATH`, mirroring the existing fake wine: it records its arguments and the managed variables, creates `pfx/system.reg` when given an empty program, exits with a chosen code, and can be made to ignore SIGTERM so the force path is covered. No umu or Proton installed is needed.
- Creation: `new --runner <proton>` creates `pfx/` under the configured prefix, writes the config, and leaves nothing behind when umu fails. Adoption of a directory that already has a `pfx/`.
- Launching: the fake umu records `WINEPREFIX`, `PROTONPATH`, `GAMEID`, and `STORE`, and that `GAMEID` and `STORE` are absent when unset. Explicit assignments to any managed variable are rejected. gamemode still wraps.
- Lifecycle: start, status, logs, stop against the fake umu; a program exiting on its own; `stop` on something ignoring SIGTERM reaching the force path; `stop --force`; a lost run cleaned up through its recorded process group; and a lost run whose group is already gone.
- Run state written before this plan has no process group recorded, so it still parses and `stop` falls back to reporting that it cannot clean it up.
- `exec` and `winetricks` both reach the fake umu with the right arguments, and `winetricks` works with no `winetricks` binary on `PATH` at all.
- `gameid` and `store` round-trip through `set` and `--unset`, are rejected on a wine environment, and show up in `status`.
- `set runner=` refuses a cross-kind change in both directions, and still allows a change within a kind.
- `rm --delete-prefix` removes the whole compatdata directory, and refuses one whose `pfx/` is not a wine prefix.
- Ignored real tests, needing umu and a Proton build installed: create a prefix, run `winetricks list-installed`, start and stop a target, and remove it with its prefix.

## Phases

1. Proton runners: detection, the runner kinds, the prefix layout, and creating a prefix through umu.
2. Launching and the run lifecycle, including stop and the process group.
3. `exec`, `winetricks`, and the `gameid` and `store` settings.

Commit each logical change separately. Each phase ends with tests, fmt, and clippy passing, then a push.

## Done when

With umu-launcher installed and a Proton build extracted to `~/.local/share/wineo/runners/GE-Proton9-20`:

```
wineo runners                                  # lists it as a proton runner
wineo new -n hades --runner GE-Proton9-20      # creates the prefix through umu
wineo set hades gameid=umu-hades
wineo set hades targets.hades.exe='C:\Hades.exe'
wineo winetricks hades dxvk                    # runs umu-run winetricks
wineo start hades
wineo status hades                             # running, with the runner and gameid
wineo stop hades
wineo rm hades --delete-prefix
```

`wineo set hades runner=wine` is refused, naming the incompatible prefix layouts.
