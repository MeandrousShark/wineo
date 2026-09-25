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
Completed: [0019: Portable layout](plans/0019-portable-layout.md).
See [Portable layout](portable.md) for selecting a folder, its layout, and moving it.
Completed: [0021: Artwork framework](plans/0021-artwork-framework.md).
See [Artwork](art.md) for the art store, key, and `wineo art` commands.
Completed: [0023: Create a prefix from a hand-written config](plans/0023-config-first-creation.md).
See [Creating an environment by hand](config-files.md#creating-an-environment-by-hand) for the behavior and safety rules.
Completed: [0024: Desktop entry icons from art](plans/0024-desktop-entry-icons.md).
See [Desktop entries](art.md#desktop-entries) for the behavior.
Completed: [0025: Fetch art during new and Heroic import](plans/0025-art-in-new-and-import.md).
See [Fetching during new and import](art.md#fetching-during-new-and-import) for the flows and flags.

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
| Wait for a lingering wineserver before snapshots | Core | **2** | 0017 | `snapshot create` right after `exec` fails with a vague "reviewed environment changed; inspect it again" | wineserver stays up about 3 seconds after the program exits and keeps writing the registry, so fingerprints differ. Wait for idle first, or say the prefix is still busy. Found in 0019's real-wine smoke test; XDG does the same. |
| Wait for a lingering wineserver before `rm --delete-prefix` | Core | **2** | | Deleting under a live wineserver makes it fail to save the registry | Seen as `wineserver: could not save registry branch to system.reg` after rm. `runner.wait_idle` already exists. Found in 0019's real-wine smoke test. |
| `wineo validate` | Core | **2** | 0018 | Find config mistakes before a launch fails, and offer safe fixes | Checks syntax, unknown keys, and `schema_version`, plus that prefixes, runners, target executables, and working dirs exist. Also stale locks, lost runs, and dangling desktop entries. Only safe fixes are offered; an unsupported version or a missing prefix is reported, never rewritten. |
| `--dry-run` for `import` and non-guided `new` | Automation | **2** | | Preview what would be created, copied, or registered without touching disk | Import already builds a review before writing, which a dry run could print and stop at. |
| Pre-launch and post-exit hooks | Core | **2** | | Run a user command before a target starts or after it exits, e.g. mounting a drive or syncing saves | The supervisor already owns the process lifetime, so post-exit fits there. Needs decisions on failure handling, env vars passed in, and per-environment versus per-target hooks. |
| Declarative winetricks dependencies | Compatibility | **2** | 0018 | List verbs in config, e.g. `winetricks = ["vcrun2022", "corefonts"]`, and have wineo install whatever is missing | Needs a record of installed verbs so it stays idempotent. A new config key is the first real test of 0018's versioning rules, since older wineo rejects unknown keys. |
| Gamescope and MangoHud support | Compatibility | **2** | 0018 | Common gaming wrappers that currently need manual setup | Heroic import already detects both and reports them as not imported, so it could carry them over once supported. Gamemode's existing setting is a model for the config shape. |
| Wine Wayland support | Compatibility | **2** | 0018 | Make the native Wayland driver an opt-in launch setting instead of requiring manual environment variables | Define behavior for supported Wine and Proton runners and report unsupported combinations. Heroic import currently omits its Wine Wayland setting; carry it over when supported. GE-Proton can already be tried with `env.PROTON_ENABLE_WAYLAND=1`. |
| ProtonDB lookup in the CLI | UX | **3** | | Check a game's compatibility information before launching or changing runners | Resolve a wineo environment to the right ProtonDB game, handle missing matches, and make the result reusable by the future GUI. The command shape and online behavior need a plan. |
| Trash prefixes instead of deleting, plus `prune` | Portability/Data | **2** | | Make `rm --delete-prefix` recoverable, with `wineo prune` doing the real delete | Needs a trash location (XDG Trash or wineo's own), a restore path, and rules for snapshots and portable folders. |
| GUI-facing core API review | Core | **2** | GUI entry criteria 1-8 | Keep GUI actions on shared core operations | Review setup, import, launch, stop, and validation entry points for useful progress, cancellation, and errors. Resolve blockers found in the review before GUI work starts. |
| Playtime statistics | UX | **2** | | Track launch count, total and per-session play time, and when a game was first and last played, for `status`, `list`, and a GUI library | The supervisor already owns the process lifetime, so it can record start and exit. Stats aren't config, so they don't touch the schema. Store them under `$XDG_DATA_HOME/wineo` (and the portable folder) rather than `$XDG_STATE_HOME`, since people often skip `~/.local/state` in backups and lost playtime can't be rebuilt; clone, copy, and rm rules would follow art's. Needs decisions on per-target versus per-environment totals, how a lost or crashed run counts, JSON output, and extras like longest session or recently played sorting. |
| Tags and `wineo list --tag` | UX | **2** | 0018 | Sort a long environment list into groups like game, work, or creative | e.g. `tags = ["game"]` in the environment config. Could also filter `status` and completion. |
| Manpages | UX | **3** | 0015 | `man wineo` is the first thing many Linux users try | Generate them from the clap definitions with `clap_mangen`, like completions, so they never drift from `--help`. The PKGBUILD installs them to `/usr/share/man/man1`. |
| Steam shortcuts | UX | **3** | | Add wineo targets to Steam as non-Steam games, for Big Picture and Steam Deck | Steam's `shortcuts.vdf` is binary and Steam must be closed while it's edited. |
| Integrity checking | Portability/Data | **4** | | Detect damaged or tampered files | Scope still open: prefix or game files, snapshots, or re-verifying downloads (already sha256-checked on install). |
| Import from Lutris/Bottles and Heroic store metadata | Compatibility | **3** | 0008 | Extends generic import and Heroic sideloaded-game import | 0008 covers manually added Heroic games only. Store integration and other launchers remain separate work. |
| Managed copying of portable application folders | Portability/Data | **3** | 0007, 0008 | Optional organization for applications used in place today | 0008 copies prefixes but leaves external game folders where they are. Storage layout and removal semantics need a later plan. |
| wineo-gui | UX | **3** | [GUI entry criteria](#gui-entry-criteria) | Build on a mature CLI and shared core operations | |
| Artwork from SteamGridDB | UX | **4** | wineo-gui | Posters, backgrounds, and icons for games, which a GUI can show | 0021 landed the art store, the `wineo art` commands, and SteamGridDB fetching, and 0024 landed desktop entry icons; see [Artwork](art.md). Remaining: showing posters and heroes in wineo-gui. |
| Native Linux games | UX | **2** | 0018 | Let a GUI library show native games next to Wine ones, so everything launches from one place | Launches a binary or script directly. Needs an environment kind with no prefix or runner, which is a config schema change, so it lands before the GUI builds on the data model. Targets, art, gamemode, and desktop entries would carry over. Wine-only commands like `winetricks`, `exec`, and snapshots don't apply. |
| Full backup and restore | Portability/Data | **4** | 0018 | Great eventually, but this could become a whole subsystem | Covers filesystems without snapshot support. |
| Event-driven supervisor exit watch | Core | **4** | | Only if there's literally nothing else to do | The supervisor polls `waitid` every 20 ms while a game runs. A pidfd plus a wakeup fd would make it sleep until the exit or a release request. Overhead today is already negligible. |

## GUI entry criteria

These change the config schema or core entry points the GUI builds on, so
they land first.

1. Both lingering wineserver fixes.
2. Add native Linux games, so the features after it know some environments have no prefix.
3. Add `wineo validate`.
4. Make prefix deletion recoverable with trash and `prune`.
5. Add Wine Wayland and Gamescope/MangoHud support, including supported Heroic import settings.
6. Add declarative winetricks dependencies.
7. Add tags and `wineo list --tag`.
8. Add playtime statistics.
9. Complete the GUI-facing core API review and resolve any blockers it finds.
