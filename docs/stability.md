# Stability

This is the promise wineo makes from 1.0 on. Until then, a 0.x release can
still break any of it, and [the changelog](../CHANGELOG.md) calls out each
break as a behavior change.

From 1.0, anything on the stable list keeps working through every 1.x
release. A change that would break it waits for 2.0, and the old form keeps
working until then with a warning, which `wineo validate` also reports. A
renamed config key is read under either name and `validate --fix` renames
it ([Deprecated keys](config-files.md#deprecated-keys)). A renamed flag or
command keeps its old spelling as a hidden alias that warns, a renamed
`WINEO_*` variable is read under both names and hooks get both, and a
renamed `--json` field is written under both names.
Features keep landing in 1.x as long as they leave the stable list intact.
Fixes for security problems and plain bugs are the exception: when the
documented behavior was never what wineo actually did, wineo is fixed to
match the docs.

## Stable

- **Config files.** `config.toml`, `environments/<name>.toml`,
  `templates/<name>.toml`, and `art.toml`: their keys, value types, and
  defaults. A file written by any 1.x release loads in every later 1.x
  release, and an older file loads or migrates cleanly. The promise runs
  forward only: an older release refuses a key added after it, with a hint
  to update wineo, rather than ignoring it. A 1.x release raises
  `schema_version` only in files that use an existing value with a new
  meaning, and every later 1.x reads every older version. See
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
  The GOG login (`account=gog`) and private branch passwords
  (`account=gog-branch`) are wineo's own and not promised.

## Not promised

- **Human-readable output.** Wording, layout, colors, prompts, and key
  hints. Scripts should use `--json` and exit codes instead.
- **The desktop app.** Its layout, behavior, and shortcuts, and the files
  it keeps for itself: `library.toml` and the custom theme files in
  `themes/`. They're read leniently, so a value wineo can't use falls back
  to its default, and an older release ignores what it doesn't know.
- **`wineo-core`'s Rust API.**
- **The built-in `gaming` template's settings.** Its name stays, but what it
  turns on can improve. To keep a fixed set, write your own
  `templates/gaming.toml`, which replaces the built-in one.
- **Shell completion output.** It follows the CLI, which is stable.
- **Files wineo manages for itself.** Run records, locks, and logs in the
  state directory, GOG folder states, Galaxy records, support files,
  the redistributable store, and `gog-branches.key` in the data directory, caches like
  `steam-owned-<steamid>.json` and the GOG lists, details, and downloads,
  temporary
  files, the desktop entries wineo writes, and the on-disk layout of
  snapshots and the trash. Use the commands to work with them. Their
  formats aren't promised, but upgrades read them, and a newer release only
  adds fields an older one can skip. When wineo can't read one of them,
  cleanup that depends on it keeps everything instead of guessing.
- **Test-only variables,** like `WINEO_TEST_*`.
- **Anything marked experimental** in its docs.
