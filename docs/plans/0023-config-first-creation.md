Status: approved

# 0023: Create a prefix from a hand-written config

## Goal

Let a user write `environments/<name>.toml` by hand, or restore one from
dotfiles, and have wineo build its prefix. Today only `new` and `import`
build prefixes, and `new` refuses a name whose config already exists, so
`wineo start` on a hand-written config fails with `prefix ... is missing or
is not a wine prefix`. Ship a commented example config with it, listing every
key, what it does, and whether it is required.

Desktop entry icons from art move from 0023 to plan 0024.

## Existing foundation

- `Environment::create` (`environment.rs`) builds a prefix and then writes a
  fresh config. It refuses when the config exists, so it can't be reused as
  is, but its pieces can: `prefix::prepare_dir` makes a missing dir, accepts
  an empty one, and refuses a non-empty non-prefix dir with `NotAPrefix`;
  `prefix::create` runs `wineboot --init` for Wine or an empty umu run for
  Proton and fails with the last wine message and the log path; a failed
  build removes the dir only if `prepare_dir` made it.
- `launch::open` is the one check behind `start`, `run`, and `exec`. It
  returns `MissingPrefix` without telling "missing" from "not a prefix", and
  runs before the launch lock is taken. `winetricks` has its own copy of the
  check, and the detached supervisor checks again.
- `start` already runs `components::reconcile_config` and
  `desktop::reconcile` after the lock, so a freshly built prefix gets its
  DXVK, vkd3d, and desktop entries from the normal start path.
- `prefix`, `runner`, and `arch` are required keys. `Paths::check_prefix`
  keeps a portable prefix a direct child of `DIR/prefixes`.
- Config writes follow plan 0018: a legacy file is only stamped with
  `schema_version = 1` during a real change.

## Proposed decisions for maintainer approval

### 1. Which commands build a missing prefix

Recommended: `start`, `run`, `exec`, `winetricks`, and `restart` build it,
then carry on as if it had always been there. `restart` checks everything
before it stops a run, and a missing prefix means nothing can be running in
it, so it builds like `start`. Checks that fail fast, like an unknown target
or winetricks missing from PATH, come before the build, so a typo never waits
on one. These are the commands that would hand the
prefix to wine, and the backlog row already leans toward `start`.

```
$ wineo start palworld
Creating a Proton prefix for palworld in ~/Games/prefixes/palworld, this takes a while
Started palworld
```

`setup` offers to build it instead, then shows its normal menu. Today it
says `Its prefix is missing, so there is nothing installed to pick from.` and
goes on with only the path entry, so that line becomes an offer in the review
menu form from plan 0022:

```
palworld: runs on GE-Proton-Latest, prefix ~/Games/prefixes/palworld
Its prefix doesn't exist yet.
  1) Skip for now
Enter create it, Ctrl+C cancel
Choice: 
```

Enter builds it, prints the same note as `start`, and continues to the
setup menu with the new prefix to pick programs from. `Skip for now` keeps
today's behavior. When decision 2 refuses the path, `setup` prints the
reason and goes on as today, with no offer.

Everything else keeps refusing with today's error: `programs`, `set`,
`clone`, `snapshot`, `portable copy`, and `rm`. They read or copy a prefix,
and an empty fresh one gives them nothing useful.

Considered and dropped: a separate `wineo build` command. `start` building
on its own and `setup` offering it cover the same ground.

### 2. When a missing prefix is safe to build

A config can point at a prefix that exists but isn't reachable right now,
like one on an unmounted drive. Building a fresh prefix there would hide the
real one. Recommended rule, checked before anything is created:

| What the prefix path holds | Result |
|---|---|
| A prefix of the runner's layout | Nothing to build |
| A prefix of the other layout (a Wine prefix with a Proton runner, or `pfx/` with a Wine runner) | Refused: `~/Games/prefixes/palworld holds a Wine prefix, but runner GE-Proton10-4 needs a Proton one.` |
| Nothing, and its parent exists and isn't empty | Built |
| Nothing, its parent is empty, and the path is outside the prefix root | `start` and the others refuse: `prefix /mnt/games/palworld is missing, and /mnt/games is empty. Run wineo setup palworld to create it there.` `setup` offers to build it |
| Nothing, its parent is empty, and the path is under the prefix root | Built |
| Nothing, the parent is missing too, and the path is under the prefix root | Built, making the parent dirs |
| Nothing, the parent is missing too, and the path is outside the prefix root | Refused: `prefix /mnt/games/prefixes/palworld is missing, and so is /mnt/games/prefixes.` |
| An empty dir under the prefix root | Built in it, and left in place if the build fails |
| An empty dir outside the prefix root | `start` and the others refuse: `prefix /mnt/game-ssd is empty. Run wineo setup palworld to create it there.` `setup` offers to build it |
| Anything else | Refused, with today's `is not empty and is not a wine prefix` |

The parent rule is what catches an unmounted drive: `/mnt/games` is an empty
mount point, so `/mnt/games/prefixes` is missing, and wineo refuses rather
than build under it. When the prefix dir is itself the mount point, an
unmounted drive looks like an empty dir, which can't be told from one the
user made on purpose, so outside the prefix root only `setup` builds there,
after asking. The same goes for a prefix directly under the mount point,
like `/mnt/games/palworld`: an unmounted `/mnt/games` is an empty parent. A
user who sets up an empty dir by hand after that has made their choice. The prefix root is wineo's own, so a fresh machine
restoring dotfiles can still build `~/.local/share/wineo/prefixes/palworld`
before that dir exists.

A portable environment's prefix must still be a direct child of
`DIR/prefixes`, as `Paths::check_prefix` already enforces.

### 3. Keep `prefix` required

Recommended: `prefix`, `runner`, and `arch` stay required. Revisit this when
native app support gets planned, since those environments may have no prefix. The example config
shows the default location, so writing it by hand is one copied line, and
`~/` keeps it working across machines:

```toml
prefix = "~/.local/share/wineo/prefixes/palworld"
```

Alternative: make `prefix` optional, defaulting to `<prefix root>/<name>`,
and write the resolved path back into the file after the build. It saves a
line, but every reader of `prefix` (status, list, rm, clone, snapshots,
portable copy) would need an "unbuilt" case, and `start` would edit a
hand-written file, which dirties it in a dotfiles repo. Leaving `prefix` out
would also tie the environment to `defaults.prefix_root`, so changing that
later would make wineo build a new, empty prefix instead of finding the old
one.

### 4. What the build does and prints

- It uses the config's runner and arch, and resolves the runner exactly as
  `start` does, so an uninstalled runner fails with today's `runner ... is
  not installed` before anything is created.
- It holds the `Create` lock on the environment and prefix while building,
  like `new`. A second command started meanwhile gets today's busy error.
- One line on stderr, matching `new`'s wording with the environment named, so
  `--json` output on stdout stays clean:
  - `Creating a wine prefix for palworld in <dir>, this takes a few seconds`
  - `Creating a Proton prefix for palworld in <dir>, this takes a while`
- The config file is never written. DXVK, vkd3d, and desktop entries come
  from the start path that follows, as they do today.
- A failed build prints today's `wine failed to create the prefix` error with
  the log path, removes the dir only if the build made it, and doesn't start
  anything.

### 5. The example config

- `docs/environment.toml.example`, named so it still fits once native apps
  have their own config.
- Required keys are live lines with example values. Optional keys are
  commented out, each with one short comment saying what it does and the
  default when left out. Targets get one example with a `C:\` path and one
  with a Unix path.
- The PKGBUILD installs it to `/usr/share/doc/wineo/environment.toml.example`,
  and `docs/config-files.md` and the README point at it.
- A test keeps it honest: the file parses as it is, and it also parses with
  every commented key uncommented, with each optional field ending up set.
  So a new config key without an example line fails the test.

Excerpt:

```toml
schema_version = 1

# Required. The prefix directory, built on first start if it doesn't exist.
prefix = "~/.local/share/wineo/prefixes/palworld"
# Required. A runner name from `wineo runners`, `wine` for the system wine,
# or an alias like `GE-Proton-Latest`.
runner = "GE-Proton-Latest"
# Required. `win64` or `win32`. Proton is always win64.
arch = "win64"

# Run under gamemode. Default: the global setting.
# gamemode = true
```

## Implementation sequence

1. Core: a `prefix::build_plan`-style check that turns the table in decision
   2 into a result, and `Environment::build_prefix`, which takes the lock,
   builds, and cleans up. Reuse `prepare_dir` and `prefix::create`, and
   factor the shared part out of `Environment::create`.
2. CLI: `start`, `run`, `exec`, `winetricks`, and `restart` call it when the prefix is
   missing, print the note, then go on as today. `setup` offers it before its
   menu. Split `MissingPrefix` so
   the remaining commands can keep their message.
3. The example config, its test, the PKGBUILD install line, and docs:
   `config-files.md` replaces "`wineo start` refuses it" with the new
   behavior, and `features.md` marks the row done. Renumber the icon plan
   references in 0021 and `features.md` to 0024.

## Verification and acceptance

- Unit tests for each row of the table in decision 2, including the portable
  direct-child rule and a failed build leaving a pre-existing empty dir.
- CLI tests: `start`, `run`, `exec`, and `winetricks` on a hand-written
  config with a fake runner build the prefix and print the note; `programs`
  still refuses; an empty dir outside the prefix root is refused with the
  `setup` hint; the config file is byte for byte unchanged.
- Pty tests for `setup`: Enter builds the prefix and lists its programs,
  `Skip for now` keeps today's message, and a refused path shows its reason
  with no offer.
- An ignored real-wine test builds a Wine prefix from a hand-written config
  through `exec`.
- The example config test from decision 5.
- The maintainer writes a config by hand from the example, runs `wineo start`
  on it with a Wine runner and with a Proton runner, and sees the prefix get
  built once and reused after. Then another through `wineo setup`.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`.

## Scope limits

No `prefix`-less configs (see decision 3), no arch checks on an existing
prefix, and no new command that prints the example config. A hand-written
config whose targets live inside the new, empty prefix still fails to start
until the game is installed there; `wineo exec` or `wineo setup` is the way
in, as it is after `new`.
