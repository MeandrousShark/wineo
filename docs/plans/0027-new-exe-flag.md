Status: draft

# 0027: `--native` as a kind, and `--exe` for both kinds

## Goal

Make `wineo new --native` a plain flag that picks the environment's kind, the
way `-t` picks settings, and move the program to launch into a separate
`--exe` flag that works for Wine environments too. Today `--native <EXE>`
mixes the kind and the target in one flag, and a Wine environment has no
unguided way to add a game that is already on disk.

```
$ wineo new -n rimworld --native --exe ~/Games/RimWorld/start.sh
Created native environment rimworld with target start

$ wineo new -n palworld --exe ~/Games/Palworld/Palworld.exe
Creating a wine prefix in ~/.local/share/wineo/prefixes/palworld, this takes a few seconds
Created environment palworld with target palworld
```

## Existing foundation

- `new --native <EXE>` (0026) checks the exe with the native launch
  resolution, names the target with `targets::suggest_name`, and makes it
  the default. It refuses `--runner`, `--arch`, `--dir`, `--template`, and
  `--install-exe`.
- A Wine `new` without `--install-exe` prints
  `Add a target with: wineo set <name> targets.<target>.exe=<path>`.
- Guided `new` has an "already installed" branch that asks for an exe path
  and makes it the default target, and a native branch that asks for the
  exe. `--native <EXE>` currently skips the kind question and prefills the
  exe answer.
- `Launch::check_target` and `check_native_target` already check a target
  without launching it.

## Proposed decisions for maintainer approval

### 1. `--native` takes no value

`--native` only sets the kind. It still refuses `--runner`, `--arch`,
`--dir`, `--template`, and `--install-exe`. The old form `--native <EXE>` is
gone, and `wineo new -n x --native dusklight` fails with clap's
`unexpected argument 'dusklight'`, which the changelog calls out.

### 2. `--exe PATH` adds a default target, for either kind

The target is named with `targets::suggest_name` and made the default, as
`--native <EXE>` does today. Desktop entries stay off, like every unguided
`new`.

- Native: the same exe rules as 0026 decision 3, checked before anything is
  written, so a bad exe creates nothing.
- Wine: the same rules as a Wine target set with `set`. It's checked after
  the prefix exists, since a `C:\` path can only exist then, for example
  with `--dir` pointing at an existing prefix. If the check fails, the
  environment is kept without the target, and the error says so:
  `Created environment palworld, but ~/Games/Palworld/Palworld.exe was not
  found. Add a target with: wineo setup palworld`
- `--exe` and `--install-exe` refuse each other, since the installer is what
  finds the targets.

### 3. `--native` without `--exe`

It makes a native environment with no targets and says how to add one:

```
$ wineo new -n rimworld --native
Created native environment rimworld
Add a target with: wineo setup rimworld
```

This relaxes 0026 decision 6a, which required the exe at creation. A native
config with no targets was already valid, and `start` already gives the
`has no targets` error for it.

### 4. Guided `new`

- `--native` without `-n` skips the kind question and goes to the native
  branch, the way `--native <EXE>` does today.
- `--exe` prefills the exe answer: the native branch's exe question, or the
  Wine "already installed" branch's path, which it also picks as the way to
  add the game.

### 5. Wine output

A Wine `new` with `--exe` prints `Created environment palworld with target
palworld` in place of `Created environment palworld` and the `Add a target`
hint. Without `--exe`, Wine output is unchanged.

## Implementation sequence

1. CLI: `--native` as a bool, `--exe`, the conflicts, the native path with
   and without an exe, and the Wine path with the check after the prefix is
   built. Update the no-terminal hint to
   `wineo new -n <name> [-t <template>] [--native] [--exe <path>] [--install-exe <path>]`.
2. Guided prefills from decision 4.
3. Docs: `native.md`, the README row, `config-files.md` if it mentions the
   flag, completion notes, and the changelog.

## Verification and acceptance

- CLI tests:
  - `--native --exe` with a path, with a name on `PATH`, and with a bad exe,
    which creates nothing
  - `--native` alone gives an empty native environment and the hint
  - `--native <value>` is refused
  - a Wine `--exe` with a Unix path, and one that doesn't exist, which keeps
    the environment with the combined error
  - `--exe` with `--install-exe` is refused
- Pty tests for guided `new` with `--native` and with `--exe` for each kind.
- Completion: `--exe` completes any file (`FilePath`), not only executable
  ones, since a Windows exe usually has no execute bit.
- The maintainer runs `wineo new -n rimworld --native --exe
  ~/Games/RimWorld/start.sh`, launches it, and removes it.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`.

## Scope limits

- No `--desktop` or `--target-name` on `new`. `set` and `setup` still cover
  those.
- `import heroic` is unchanged.
- The Wine `Add a target` hint keeps its `wineo set` form.
