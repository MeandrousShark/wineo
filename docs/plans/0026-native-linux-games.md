Status: draft

# 0026: Native Linux games

## Goal

Let an environment launch a native Linux binary or script directly, with no
prefix and no runner, so native games sit next to Wine ones in `list`,
`status`, desktop entries, art, and later the GUI library. Heroic's sideloaded
native games become importable too.

This is GUI entry criterion 1. It lands before `validate`, trash, wrappers,
and the rest so those features are built knowing some environments have no
prefix.

## Existing foundation

- `EnvironmentConfig` (`config.rs`) requires `prefix`, `runner`, and `arch`,
  and denies unknown fields. Plan 0023 kept them required and deferred this
  question to native support.
- `Launch` always holds a `Runner` and a prefix, and `Launch::command` always
  goes through `Runner::command`, which sets `WINEPREFIX` and
  `WINEDLLOVERRIDES`. The exe resolver already has a Unix path branch that
  needs no prefix.
- Proton runs already end through the process group: `end_session` sends
  SIGTERM to the group, `kill` sends SIGKILL, and settling waits for the
  group to empty. A native run needs exactly that, with no wineserver.
- `RunState` and `OperationState` already store `runner` and `prefix` as
  `Option`. Lost-run cleanup in `stop.rs` treats a missing runner or prefix as
  unresolved, so it needs a native case.
- Desktop entries run `wineo start <env> --target <target>`, which works for
  any kind. `desktop::reconcile` already takes `Option<&Runner>`. Icons come
  from art, then from `winemenubuilder` for the exe, which is Wine only.
- `picker.rs` already accepts a `Place` with no wine prefix and takes Unix
  paths there.
- Heroic import already detects native sideloaded games
  (`HeroicGameKind::Native`, from `install.platform = "linux"`) and refuses
  them as `native games are not Windows prefix imports`. The maintainer's
  Heroic library has one: Ship of Harkinian, with
  `executable = "/usr/bin/soh"`.
- `wineo list` and `status --json` without a name show no runner or prefix,
  so they work as is. `status <name>`, its JSON, and `EnvironmentReport`
  always show both.

## Proposed decisions for maintainer approval

### 1. How a config says it's native

Recommended: a top-level `kind` key. Missing means `wine`, which covers both
Wine and Proton runners, so every existing config keeps working unchanged.
`kind = "native"` forbids `prefix`, `runner`, `arch`, `gameid`, `store`,
`dxvk`, and `vkd3d`, each with its own one-line error.

```toml
schema_version = 1
kind = "native"
gamemode = true

[targets.game]
exe = "/usr/bin/soh"
default = true
desktop = true
name = "Ship of Harkinian"
```

```
native environment soh can't set runner
```

Alternatives, doing the same thing:

```toml
# B: a reserved runner name, prefix and arch left out
runner = "native"

# C: no prefix and no runner means native
[targets.game]
exe = "/usr/bin/soh"
```

B reads naturally but makes `runner` mean two things, and a runner
directory named `native` would clash with it. C is the shortest, but a
Wine config that loses its `prefix` line to a typo would quietly become
native instead of failing.

In code, the Wine-only fields move behind the kind (for example
`EnvironmentConfig { kind: Kind::Wine(WineConfig) | Kind::Native, .. }`),
so anything that needs a prefix has to match on the kind rather than find
out at run time. This is the main point of landing native before the other
GUI criteria.

### 2. Schema version

Recommended: stay at `schema_version = 1`. `kind` is a new optional key, and
0018 kept strict unknown-field checks, so an older wineo refuses a native
config with `unknown field kind` instead of misreading it. Wine configs never
write `kind`, so they stay readable by older wineo.

Alternative: native configs are written as version 2. An older wineo would
then say `unsupported schema_version 2`, which is clearer, but it splits
versions by kind rather than by format, and 0018 has no rule for that.

### 3. What a native target's `exe` can be

Recommended:

| `exe` | Meaning |
|---|---|
| `/usr/bin/soh`, `~/Games/Celeste/Celeste` | That file. It must exist and be executable. |
| `soh` (no `/`) | Looked up on `PATH` at launch, like a shell would |
| `C:\...` or anything ending in `.exe`, `.msi`, `.bat`, `.lnk` | Refused: `soh is native, so its target can't be a Windows program` |
| A relative path like `bin/game` | Refused outside portable mode, as for prefixes. In a portable folder it resolves against the folder, as Unix target paths do today. |

The file runs directly, not through `sh`, so a script needs its shebang and
execute bit. A file that exists but isn't executable is refused with
`~/Games/foo/start.sh is not executable. Run chmod +x on it.`

`cwd` defaults to the exe's own directory, like Wine targets. For a bare
name found on `PATH` (the exe is in `/usr/bin`), it defaults to the home
directory instead.

### 4. Launching and stopping

- The command is `[gamemoderun] <exe> <args...>`, with the global `[env]`,
  the environment's `[env]`, and `-e` overrides applied. No `WINE*`
  variables are set, and `WINEPREFIX` is no longer reserved in a native
  environment's `[env]`.
- It runs in its own process group like a Proton run. `start` supervises it
  as usual, and the run is over when the whole group is gone, so a launcher
  that hands off to the game and exits is still tracked.
- `stop` sends SIGTERM to the group, waits `stop_timeout`, then sends
  SIGKILL. Lost-run cleanup kills a recorded group the same way.
- A program that leaves its own process group (`setsid`, or something that
  asks a running Steam to launch it) isn't tracked after it leaves. That's
  the same limit Proton has today.

### 5. Creating one

Recommended: `new --native`, and a first question in guided `new`.

```
$ wineo new -n soh --native
Created native environment soh
Add a target with: wineo set soh targets.<target>.exe=<path>
```

`--native` refuses `--runner`, `--arch`, `--dir`, `--template`, and
`--install-exe`. `--art` works as it does today.

Guided `new` asks first, before the name:

```
What kind of environment?
  1) Windows program (Wine or Proton)
  2) Native Linux program

Press Enter to pick 1, or pick a number. Ctrl+C to cancel.
Choice:
```

The native branch asks for the name, then the executable path with the
existing path editor, then the target name, desktop entry, and art, the
same way the "already installed" branch does today. It skips location,
runner, arch, and installer.

Alternative: `new -n soh --exe /usr/bin/soh`, where giving an `--exe` with
no `--runner` means native and creates a `game` target in one step. That's
shorter, but it adds a second way to make targets from `new`, and guided
`new` would still need the question.

A hand-written native config works with no build step, since there's no
prefix to build.

### 6. Commands

| Command | Native behavior |
|---|---|
| `start`, `run`, `restart`, `stop`, `status`, `list`, `logs` | Work |
| `set` | `gamemode`, `env.*`, and `targets.*` work. Wine-only keys get the decision 1 error. `kind` can't be changed: `make a new environment instead` |
| `setup` | Skips the prefix and program scan and goes straight to adding a target by path |
| `desktop`, `art` | Work. The icon comes from art only, since `winemenubuilder` can't read a Linux binary, and an entry with no art uses the theme's generic icon |
| `clone` | Copies the config and art under the new name, with desktop entries off as today. No prefix to copy |
| `portable copy` | Copies the config and art. Absolute `exe` paths stay host paths and are listed as external dependencies in the review |
| `rm` | Works. `--delete-prefix` is refused: `soh has no prefix. Run wineo rm soh` |
| `exec`, `winetricks`, `programs`, `snapshot` | Refused: `soh is a native environment, so it has no prefix` |
| `import` (a prefix) | Can't make a native environment, unchanged |
| `templates` | Wine only. `--native` refuses `--template` |

Desktop entry categories follow today's rule minus the Proton part:
`Game;` when gamemode is on, otherwise `Utility;`.

`status soh` prints `kind: native` in place of the runner and prefix lines.
`status soh --json` gains `"kind": "wine"` or `"native"`, and for native,
`runner` and `prefix` are `null`. That's an additive change plus two newly
nullable fields, which `json-output.md` documents.

### 7. Heroic import

Recommended: include it. A native sideloaded game imports as a native
environment:

- `exe` from `install.executable`, one `game` target, default, with the
  desktop entry following today's import rules.
- `gamemode` from `useGameMode`, `[env]` from `enviromentOptions`, launch
  arguments as today, and `heroic_id` so a re-import is recognized.
- Heroic's `winePrefix`, `wineVersion`, DXVK, and other Wine settings are
  ignored. Heroic stores them for native games too, but never uses them.
- `import heroic --all` includes native games, one line each as today.
- Wrapper options stay reported as not imported, as they are for Wine games.

```
$ wineo import heroic
...
Import Ship of Harkinian as soh
  kind: native
  exe: /usr/bin/soh
  gamemode: off
```

Alternative: leave Heroic native import for a follow-up plan and keep today's
refusal.

### 8. Example config

A second file, `docs/native-environment.toml.example`, with the same test as
`environment.toml.example`: it parses as is and with every commented key
uncommented. A native section inside the existing file can't pass that test,
since uncommenting it would set `kind = "native"` next to `prefix`. The
PKGBUILD installs both.

## Implementation sequence

1. Core config: `kind`, the Wine-only fields moved behind it, native
   validation and errors, and every reader of `prefix` or `runner` updated to
   match on the kind. `transfer::wait_idle` and other prefix-only helpers
   return early for native.
2. Core launch: a native `Launch` without a runner, exe and cwd resolution
   from decision 3, and supervisor, stop, and lost-run cleanup using the
   process group path.
3. CLI: `new --native`, guided `new`'s kind question and native branch,
   `set`, `setup`, `status` and its JSON, the refusals in decision 6,
   `clone`, `portable copy`, `rm`, and desktop entries.
4. Heroic import of native games (if decision 7 is approved).
5. Docs: a native section in `config-files.md`, the example file and its
   test, `json-output.md`, `heroic` import docs, README, PKGBUILD line, and
   `features.md`.

## Verification and acceptance

- Config tests: a native config parses; each Wine-only key is refused with
  its message; an existing config with no `kind` reads as Wine unchanged,
  byte for byte after a no-op edit.
- Launch tests with fake native scripts: exe on a path, on `PATH`, not
  executable, a Windows path refused; env and args applied; no `WINE*`
  variables set; gamemode wraps it.
- Supervisor tests: a handoff script keeps the run going until its child
  exits; `stop` ends a stubborn one with SIGKILL after the timeout; a lost
  native run is cleaned up by `stop`.
- CLI tests for each row of decision 6, including `clone` and
  `portable copy` of a native environment and `status --json` output.
- Pty tests for guided `new`'s native branch.
- Heroic tests: a native sideloaded game imports with its exe, gamemode, and
  env; `--all` includes it.
- The maintainer imports Ship of Harkinian from Heroic, launches it from its
  desktop entry, stops it with `wineo stop`, and makes one native
  environment by hand from the example file.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`.

## Scope limits

- No wrappers beyond gamemode. Gamescope and MangoHud are GUI criterion 4
  and will apply to both kinds.
- No icons read from Linux binaries, `.desktop` files, or AppStream. Art is
  the only icon source for native targets.
- No Steam, Lutris, or Flatpak app discovery. Games in other launchers are
  added by path.
- No native equivalent of `exec`. Running an arbitrary command with an
  environment's variables could be a later feature.
- No mac or other non-Linux platforms, even though Heroic groups `mac` with
  `linux` as native.
