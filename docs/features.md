# Features

Feature backlog and queued work. When a feature gets picked up, it gets its
own numbered plan in `docs/plans/`.

Completed: [0007: Guided game setup](plans/0007-guided-game-setup.md) and
[0008: Downloads and prefix imports](plans/0008-downloads-and-imports.md),
including launch settings for manually added Heroic games.
See [Downloads and imports](downloads-and-imports.md) for usage.
Completed: [0010: JSON output for status and list](plans/0010-json-output.md).
See [JSON output](json-output.md) for the command schema and examples.

Completed: [0014: Dynamic environment and target completion](plans/0014-dynamic-shell-completion.md).
See [Shell completion](shell-completion.md) for setup and supported values.
Completed: [0015: Arch package for Wineo and shell completions](plans/0015-arch-package.md).
Completed: [0016: Clone an environment](plans/0016-clone-environment.md).
See [Clone an environment](clone.md) for command behavior and limits.
Completed: [0017: Prefix snapshots](plans/0017-prefix-snapshots.md).
See [Prefix snapshots](snapshots.md) for command behavior and limits.
Completed: [0018: Config schema versioning](plans/0018-config-schema-versioning.md).
See [Config file versioning](config-files.md) for the key and compatibility rules.

**Category**

- Core: makes Wineo meaningfully better at managing environments
- Compatibility: runners, winetricks, imports, graphics/runtime plumbing
- UX: guided flows, completion, desktop entries, GUI
- Automation: scripting/output behavior
- Portability/Data: clone, portable layouts, schema/migration, snapshots, backups

**Priority**

- 1 = next: should land soon, small, high value, or foundational
- 2 = soon: important, but depends on more core stability
- 3 = later: useful expansion or polish
- 4 = someday: explicitly nonessential or large scope

**Depends on** lists what has to be built first. `0001 phase 2` is the second half of the foundation plan, which has landed, so the rows naming it are unblocked.

| Feature | Category | Priority | Depends on | Why | Notes |
|---|---|---:|---|---|---|
| `wineo validate` | Core | **2** | 0018 | Find config mistakes before a launch fails, and offer safe fixes | Checks syntax, unknown keys, and `schema_version`, plus that prefixes, runners, target executables, and working dirs exist. Also stale locks, lost runs, and dangling desktop entries. Only safe fixes are offered; an unsupported version or a missing prefix is reported, never rewritten. |
| `--dry-run` for `import` and non-guided `new` | Automation | **2** | | Preview what would be created, copied, or registered without touching disk | Import already builds a review before writing, which a dry run could print and stop at. |
| Pre-launch and post-exit hooks | Core | **2** | | Run a user command before a target starts or after it exits, e.g. mounting a drive or syncing saves | The supervisor already owns the process lifetime, so post-exit fits there. Needs decisions on failure handling, env vars passed in, and per-environment versus per-target hooks. |
| Declarative winetricks dependencies | Compatibility | **2** | 0018 | List verbs in config, e.g. `winetricks = ["vcrun2022", "corefonts"]`, and have wineo install whatever is missing | Needs a record of installed verbs so it stays idempotent. A new config key is the first real test of 0018's versioning rules, since older wineo rejects unknown keys. |
| Gamescope and MangoHud support | Compatibility | **2** | 0018 | Common gaming wrappers that currently need manual setup | Heroic import already detects both and reports them as not imported, so it could carry them over once supported. Gamemode's existing setting is a model for the config shape. |
| Trash prefixes instead of deleting, plus `prune` | Portability/Data | **2** | | Make `rm --delete-prefix` recoverable, with `wineo prune` doing the real delete | Needs a trash location (XDG Trash or wineo's own), a restore path, and rules for snapshots and portable folders. |
| Tags and `wineo list --tag` | UX | **3** | 0018 | Sort a long environment list into groups like game, work, or creative | e.g. `tags = ["game"]` in the environment config. Could also filter `status` and completion. |
| Steam shortcuts | UX | **3** | | Add wineo targets to Steam as non-Steam games, for Big Picture and Steam Deck | Steam's `shortcuts.vdf` is binary and Steam must be closed while it's edited. |
| Integrity checking | Portability/Data | **4** | | Detect damaged or tampered files | Scope still open: prefix or game files, snapshots, or re-verifying downloads (already sha256-checked on install). |
| Portable layout | Portability/Data | **3** | 0018 | Nice design feature, but standard layout should stabilize first | |
| Import from Lutris/Bottles and Heroic store metadata | Compatibility | **3** | 0008 | Extends generic import and Heroic sideloaded-game import | 0008 covers manually added Heroic games only. Store integration and other launchers remain separate work. |
| Managed copying of portable application folders | Portability/Data | **3** | 0007, 0008 | Optional organization for applications used in place today | 0008 copies prefixes but leaves external game folders where they are. Storage layout and removal semantics need a later plan. |
| wineo-gui | UX | **4** | | Explicitly not the main product; let the CLI mature first | |
| Artwork from SteamGridDB | UX | **4** | wineo-gui | Posters, backgrounds, and icons for games, which a GUI can show | Could also fill in desktop entry icons for games whose exe has no shortcut to take one from. |
| Full backup and restore | Portability/Data | **4** | 0018 | Great eventually, but this could become a whole subsystem | Covers filesystems without snapshot support. |
