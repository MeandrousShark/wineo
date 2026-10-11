# Stability

This is the promise wineo makes from 1.0 on. Until then, a 0.x release can
still break any of it, and [the changelog](../CHANGELOG.md) calls out each
break as a behavior change.

From 1.0, anything on the stable list keeps working through every 1.x
release. A change that would break it waits for 2.0, and the old form keeps
working until then with a warning, which `wineo validate` also reports.
Features keep landing in 1.x as long as they leave the stable list intact.
Fixes for security problems and plain bugs are the exception: when the
documented behavior was never what wineo actually did, wineo is fixed to
match the docs.

## Stable

- **Config files.** `config.toml`, `environments/<name>.toml`,
  `templates/<name>.toml`, and `art.toml`: their keys, value types, and
  defaults. A file written by any 1.x release loads in every later 1.x
  release, and an older file loads or migrates cleanly. See
  [Config files](config-files.md).
- **How settings combine.** The order `[env]` variables and other settings
  merge in: global config, then template, then environment, then target.
- **CLI commands and flags.** Names, short forms, and what they do.
- **Exit codes.** Everything in [Exit codes](exit-codes.md).
- **`--json` output.** Fields keep their names, types, and meanings, and new
  fields can appear. Enum fields like `install` can gain new values, so
  scripts should handle values they don't recognize. `wineo validate`'s
  issue codes are stable. See [JSON output](json-output.md).
- **Where files live.** The config and data directories, and the
  [portable folder](portable.md) layout.
- **Environment variables wineo reads.** `WINEO_PORTABLE_DIR`,
  `WINEO_STEAMGRIDDB_KEY`, and `WINEO_STEAM_KEY`.
- **Hook variables.** The `WINEO_*` variables [hooks](hooks.md) receive, and
  when each one is set.
- **Keyring items.** Keys are stored with `service=wineo` and
  `account=steamgriddb` or `account=steam`, so other tools can find them.

## Not promised

- **Human-readable output.** Wording, layout, colors, prompts, and key
  hints. Scripts should use `--json` and exit codes instead.
- **The desktop app.** Its layout, behavior, and shortcuts.
- **`wineo-core`'s Rust API.**
- **The built-in `gaming` template's settings.** Its name stays, but what it
  turns on can improve. To keep a fixed set, write your own
  `templates/gaming.toml`, which replaces the built-in one.
- **Shell completion output.** It follows the CLI, which is stable.
- **Files wineo manages for itself.** Run records, locks, and logs in the
  state directory, GOG folder states in the data directory, caches like
  `steam-owned-<steamid>.json`, temporary
  files, the desktop entries wineo writes, and the on-disk layout of
  snapshots and the trash. Use the commands to work with them.
- **Test-only variables,** like `WINEO_TEST_*`.
- **Anything marked experimental** in its docs.
