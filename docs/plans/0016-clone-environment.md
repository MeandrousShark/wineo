Status: draft

# 0016: Clone an environment

## Goal

Copy an existing Wineo environment into an independent prefix and register it
under a new name. The source remains usable and unchanged. Use reflinks for
regular files when supported, with a normal copy when they are not.

## Existing foundation

- Environment configs are TOML files under the XDG config directory. A config
  records its prefix, runner, architecture, compatibility settings, environment
  variables, and targets. Some imported configs also record a Heroic game ID.
- Generic prefix import already copies Wine prefixes and complete Proton
  compatdata trees through a staging directory. It preserves hardlinks,
  symlinks, modes, and timestamps; rebases absolute links inside the copied
  tree; rejects special files; checks source changes; and publishes without
  replacing an existing destination. Regular files currently use `fs::copy`.
- Import locks the source and destination prefix identities, checks the runtime
  state of environments sharing the source, and registers only after a valid
  copy is published. The clone path can reuse these rules and the copy code.
- `rm --delete-prefix` deletes the configured prefix only after checking for
  shared users. Desktop entries are generated from target `desktop` settings.

## Proposed decisions for maintainer approval

### Command and destination

- Add `wineo clone SOURCE NEW_NAME [--dir PATH]`. Both names are required, so
  the command works without a terminal. Shell completion offers existing
  environments for SOURCE. The destination defaults to the configured prefix
  root plus NEW_NAME. `--dir` selects a different new prefix directory.
- This command always copies. It does not adopt, merge, overwrite, change the
  runner, or create a new Wine prefix. Refuse an existing environment name,
  existing destination, or overlapping source and destination paths.
- Print the source name, destination, runner, prefix layout, copied targets,
  external dependencies, and any settings that change before starting. A
  terminal confirmation is not required for the fully specified command.

### Config and target behavior

- Copy the source TOML while retaining its formatting and comments. Change
  only `prefix`, target paths described below, `heroic_id`, and desktop flags.
  Keep the runner selection, architecture, gamemode, gameid, store, DXVK,
  vkd3d, environment variables, target names, arguments, and default target.
- Remove `heroic_id` from the clone. It identifies a Heroic import and should
  not make the new environment match the same Heroic game during later imports.
  Keep `gameid` and `store`, which are Proton launch settings.
- Rebase absolute Unix `targets.*.exe` and `targets.*.cwd` paths that point
  inside the source prefix to the same relative location in the new prefix.
  Keep Windows paths unchanged because they resolve against the active prefix.
  Keep paths outside the source prefix unchanged, including game folders and
  saves used in place. Do not rewrite arguments, environment variable values,
  registry contents, or arbitrary files. Report literal source-prefix paths in
  arguments or environment variables so the user can edit them afterward.
- Set cloned targets' `desktop` flags to false. This avoids creating duplicate
  launchers as a side effect of copying. The user can enable clone launchers
  afterward with `wineo set` or `wineo setup`. Preserve each target's display
  name in the config.

### Copy, coordination, and recovery

- Copy the entire configured prefix root, including Proton's compatdata parent
  and adjacent metadata. Preserve import's symlink, hardlink, metadata,
  cancellation, and staged-validation behavior. The copy must not modify the
  source, run Wine, or launch the game.
- Attempt a reflink for each new regular file. Fall back to the current normal
  copy only when reflinks are unavailable for that file or filesystem. Other
  read/write failures remain errors. Preserve hardlink relationships in the
  destination and never hardlink destination files to source files.
- Hold locks for the source and destination prefix identities and new name.
  Refuse a running or unresolved Wineo operation on any environment sharing
  the source. Explain that external launchers do not take Wineo locks and must
  be idle. Recheck the source config and prefix after review and during copy;
  refuse a detected change rather than registering an inconsistent clone.
- Stage beside the destination, validate the completed copy, then publish
  without replacement. A failed or canceled copy leaves no registered clone
  or destination. Write the clone config after publication. If registration
  fails, retain the complete destination and print its path plus recovery
  instructions; do not imply that rerunning clone can overwrite it. Never
  remove the source or a published clone during error cleanup.
- Do not copy runtime records, logs, import logs, or existing desktop files.
  `rm NEW_NAME` removes only the clone registration; `rm NEW_NAME
  --delete-prefix` removes its copied prefix under the existing safeguards.

## Implementation sequence

1. Extract the reusable staged prefix copier from import without changing
   import behavior. Add reflink attempts and precise fallback handling.
2. Add core clone inspection and execution, including config transformation,
   source checks, lock coordination, and recovery messages.
3. Add the CLI command, source-name completion, progress, and concise review
   output. Document the command and update the feature backlog after it lands.
4. Add focused core and CLI coverage, then perform a real prefix smoke test.

## Verification and acceptance

- Core tests cover Wine and Proton layouts; config fidelity; internal Unix
  path rebasing; unchanged Windows and external paths; Heroic ID removal;
  disabled clone desktop entries; symlinks, hardlinks, metadata, reflink
  success and fallback; collisions, overlap, unsafe runtime state, source
  changes, cancellation, and failed registration recovery.
- CLI tests with isolated XDG roots and fake runners show a complete clone,
  unchanged source config and prefix, no Wine launch, and correct removal.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`
  pass. Run applicable ignored tests and independent QA before shipping.
- The maintainer authorized using Dispatch or Zomboid for testing. Clone one
  into a temporary new environment and destination. Compare target resolution
  and prefix layout, then remove only the temporary clone. Do not launch the
  game or modify the source environment during this test.

## Scope limits

This is a local environment copy, not a snapshot or portable backup. It does
not copy external game folders, downloads, runners, runtime state, or desktop
entries. It does not migrate registry paths or rewrite arbitrary strings.
