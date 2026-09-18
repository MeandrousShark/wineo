Status: draft

# 0003: Custom runners and gamemode

## Goal

Let an environment run on a wine build other than the system one, installed by dropping it into a runners directory, and let games ask for gamemode. This is the foundation the Proton and DXVK work sits on, so it settles what a runner is before umu stresses it.

## Decisions

Settled by the maintainer:

- This plan covers two `docs/features.md` rows: custom runners in `runners/`, and gamemode. Proton through umu-launcher, DXVK/vkd3d, templates, and runner downloads each get their own later plan.
- A runner is a drop-in directory under a runners root. Its kind is detected from its layout, not declared in a config file. There is no per-runner TOML.
- Only wineo's own runners root is scanned. Discovering runners already installed by Steam, Lutris, or Heroic is a separate features.md row.
- For a Proton environment, the `prefix` in an environment config is the compatdata directory, the way Heroic and Steam use it. The real wine prefix is `pfx/` inside it, and wineo appends that itself wherever it needs a wine prefix. Recorded here so the Proton plan does not reopen it.
- `gamemode` is configurable globally, per environment, and per target, with the most specific setting winning.
- A launch with gamemode on and no `gamemoderun` on `PATH` is an error, not a warning. Wineo does not quietly start a game without something the config explicitly asked for.

## Details

Everything in this section is a proposal for review.

### The runners root

- `~/.local/share/wineo/runners/`, next to `prefixes/`. Runners are large extracted trees, so they belong in the data directory rather than the config directory.
- `defaults.runner_root` overrides it in the global config, mirroring `defaults.prefix_root`. `wineo set --global defaults.runner_root=~/Games/runners` works like the prefix root does.
- A missing runners root is not an error. It means no custom runners are installed.

### What a runner is

- One directory per runner. Its name is its directory name, and that is what goes in a config's `runner` value.
- Installing a runner is extracting a tarball into the runners root. Wineo never downloads, extracts, or verifies anything in this plan.
- The kind is detected from the layout, checked in this order:
  - `files/bin/wine` exists, or a `proton` script sits at the root: a Proton runner.
  - `bin/wine` exists: a wine runner.
  - Anything else: not a runner. It is skipped when listing, and naming it is an error that says why.
- A wine runner uses `<root>/bin/wine` and `<root>/bin/wineserver`. Both must exist; a directory with only one is not a runner.
- A Proton runner's wine binaries are `<root>/files/bin/wine` and `<root>/files/bin/wineserver`. Heroic uses exactly these for winetricks, winecfg, and running an exe in a Proton prefix, so wineo's existing prefix and lifecycle commands can use them too. Launching a target through Proton needs umu, so in this plan a Proton runner is detected and named but refuses to launch, with an error pointing at the unimplemented feature rather than a generic "unknown runner".
- Nothing about a runner is copied into an environment's config beyond its name. Replacing the directory with a newer build of the same name is how a runner is upgraded.

### Names

- A runner name is ASCII letters, digits, `-`, `_`, and `.`, and is never `.` or `..`. Dots are allowed because upstream builds are named things like `wine-10.0` and renaming them to install them is a papercut. Environment names keep their existing stricter rule, since those become dotted config keys and runner names do not.
- `wine` is reserved and always means the system wine found on `PATH`, which is what every existing config already says. A directory named `wine` in the runners root is ignored, with a note saying the name is reserved.
- A directory whose name is not valid is skipped when listing runners, and cannot be named.

### Resolving a runner

- `Runner::resolve` needs the runners root, so it takes `&Paths` as well as the name. Its callers already have one.
- Resolution order for a name other than `wine`: look for `<runner_root>/<name>`. If there is no such directory, the error lists the runners that are installed, or says the runners root is empty.
- `Runner` gains a kind, and keeps its resolved absolute `wine` and `wineserver` paths. Everything downstream, `wait_idle`, `end_session`, `kill`, `version`, and the `WINE`/`WINESERVER` variables `wineo winetricks` sets, keeps working unchanged on those paths.
- `Runner::version` runs `<wine> --version`. For a Proton runner the result is wine's version, not Proton's; reading Proton's own `version` file is left to the Proton plan.
- A custom wine build is assumed to be relocatable, the way wine-tkg and Proton builds are. Wineo sets no `LD_LIBRARY_PATH` of its own. If a build needs one, the environment's `[env]` table is where it goes.

### `wineo runners`

- A new verb listing installed runners: name, kind, and version, in the same table style as `wineo list`.
- The system `wine` is always the first row, with its resolved path, or a note when it is not on `PATH`.
- Directories that are not runners are left out. `--all` also lists them with the reason they were skipped, so a bad extraction is diagnosable.
- The runners root's path is printed when nothing is installed, so it is obvious where to extract to.

### Choosing a runner

- `wineo new -n hades --runner GE-Proton9-20` already exists and starts working for real names. The runner is validated before the prefix is created, so a typo fails immediately.
- `wineo set hades runner=wine-tkg` changes an existing environment's runner. It is refused while the environment is running, and it participates in operation coordination the same way a prefix change does, since the next launch uses a different wine against the same prefix.
- Changing the runner does not migrate the prefix. A prefix created by one wine build and then run by an older one can break, so `set` prints a note saying so. Wineo does not try to detect or prevent it.
- `defaults.runner` in the global config continues to pick what `wineo new` uses when `--runner` is not passed.

### gamemode

Config:

```toml
# ~/.config/wineo/config.toml
gamemode = true

# ~/.config/wineo/environments/hades.toml
gamemode = true

[targets.launcher]
exe = 'C:\launcher.exe'
gamemode = false
```

- A boolean at three levels. The target's value wins over the environment's, which wins over the global one. Unset at every level means off.
- `wineo set --global gamemode=true`, `wineo set hades gamemode=true`, and `wineo set hades targets.launcher.gamemode=false` all work, and all three keys can be unset.
- When it is on, the launch command becomes `gamemoderun <runner wine> <args...>`. Nothing else about the launch changes.
- `gamemoderun` missing from `PATH` while gamemode is on refuses the launch: `` gamemode is on for `hades`, but `gamemoderun` was not found on PATH ``. It names the setting as well as the binary, since `gamemoderun` ships in the `gamemode` package and the two names do not match. Nothing is started.
- The check happens where the launch is resolved, so `start` reports it before handing anything to a supervisor, and a detached run never fails half-spawned.
- gamemode applies only to launching a target, so `run`, `start`, and `restart`. `exec`, `winetricks`, and prefix creation are not games and never take it, whatever the config says.
- `status <name>` shows a `gamemode` line only when it is on for that environment's last run.

### Testing

- Runner discovery: a runners root with a wine runner, a Proton runner, a directory with neither layout, a directory with `bin/wine` but no `bin/wineserver`, an invalid name, and a directory named `wine`. Verify which are listed, which are skipped, and that `--all` gives a reason for each skip. No wine installed is needed; fake trees of empty executable files are enough.
- Resolution: the reserved `wine` name still resolves from `PATH`; a custom name resolves to its absolute binaries; an unknown name's error lists what is installed; an empty or missing runners root says where to extract to.
- A Proton runner resolves, appears in `wineo runners`, and is usable by `exec` and `winetricks` against `<prefix>/pfx`, but `start` and `run` refuse with the not-yet-implemented error.
- `defaults.runner_root` moves where runners are found.
- `new --runner` validates before creating a prefix, so a typo leaves nothing behind. `set runner=` is refused while running and while another operation holds the environment, and prints the prefix-compatibility note.
- Lifecycle against a custom runner: the existing fake wine and wineserver scripts, installed as `runners/fake/bin/{wine,wineserver}` instead of on `PATH`, drive a full start, status, logs, stop cycle. This proves resolution reaches the supervisor and that nothing still assumes `PATH`.
- gamemode: merging across all three levels including a target turning it off, a fake `gamemoderun` on `PATH` that records its arguments and then executes them, and that `exec` and `winetricks` never wrap.
- The missing-gamemoderun error, with `PATH` pointing somewhere without it: `run` and `start` both refuse, the message names the environment and the binary, and nothing is left running or recorded afterwards.
- Ignored real wine tests: none. This plan adds nothing that needs a real wine that the existing ignored tests do not already cover.

## Phases

1. The runners root, runner discovery and resolution, `wineo runners`, and `set runner=`.
2. gamemode.

Commit each logical change separately. Each phase ends with tests, fmt, and clippy passing, then a push.

## Done when

With a wine build extracted to `~/.local/share/wineo/runners/wine-tkg`:

```
wineo runners                              # lists wine (system) and wine-tkg
wineo new -n hades --runner wine-tkg
wineo set hades targets.hades.exe='C:\Hades.exe'
wineo status hades                         # runner line names wine-tkg and its version
wineo set --global gamemode=true
wineo start hades                          # runs under gamemoderun
wineo stop hades
```

`wineo new -n x --runner nope` fails before creating anything and lists the installed runners. A GE-Proton directory in the runners root shows up in `wineo runners` as a Proton runner, works with `wineo winetricks`, and refuses to `start` with an error naming the Proton plan.

## Open questions

Proposed details above are for review. These are the calls worth making explicitly:

1. Should a Proton runner be detected and refuse to launch, or be invisible until the Proton plan lands? (proposed: detected and refusing, so a dropped-in GE-Proton gives a useful error and its prefix is still serviceable with `exec` and `winetricks`)
2. Should runner names allow dots, unlike environment names? (proposed: yes, so upstream builds extract and work without renaming)
3. `features.md` loses the custom runners and gamemode rows once implementation is approved, and the rows depending on custom runners point at 0003. (proposed: yes)
