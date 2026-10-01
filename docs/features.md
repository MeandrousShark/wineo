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
Completed: [0026: Native Linux games](plans/0026-native-linux-games.md).
See [Native Linux games](native.md) for creating, launching, and importing them.
Completed: [0028: `wineo validate`](plans/0028-validate.md).
See [Validate](validate.md) for the checks, fixes, and exit status.
Completed: [0029: Playtime statistics](plans/0029-playtime-stats.md).
See [Playtime](stats.md) for what counts and where it's kept.
Completed: [0030: Recoverable removal and prune](plans/0030-trash-and-prune.md).
See [Trash and removal](trash.md) for restoring and pruning.
Completed: [0031: Wine Wayland, gamescope, and MangoHud](plans/0031-wayland-gamescope-mangohud.md).
See [Launch options](launch-options.md) for the settings and how they combine.
Completed: [0032: Declarative winetricks dependencies](plans/0032-declarative-winetricks.md).
See [Winetricks dependencies](config-files.md#winetricks-dependencies) for the key and when verbs install.
Completed: [0033: Tags and `wineo list --tag`](plans/0033-tags.md).
See [Tags](config-files.md#tags) for the key and filtering.
Completed: [0034: GUI-facing core API review](plans/0034-core-api-review.md).
See [Core API for frontends](core-api.md) for the operations a GUI calls.
Completed: [0035: Heroic import in core](plans/0035-heroic-import-in-core.md).
See [Heroic import](core-api.md#heroic-import) for the plan-then-apply API.
Completed: [0036: GUI library launcher](plans/0036-gui-library-launcher.md).
See [Desktop app](gui.md) for using it.
Completed: [0037: GUI settings](plans/0037-gui-settings.md).
See [Settings](gui.md#settings) for the pages.
Completed: [0038: GUI new, import, and clone](plans/0038-gui-new-import-clone.md).
See [Add, import, and clone](gui.md#add-import-and-clone) for the current flows.

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
| `--dry-run` for `import` and non-guided `new` | Automation | **2** | | Preview what would be created, copied, or registered without touching disk | Import already builds a review before writing, which a dry run could print and stop at. |
| Pre-launch and post-exit hooks | Core | **2** | | Run a user command before a target starts or after it exits, e.g. mounting a drive or syncing saves | The supervisor already owns the process lifetime, so post-exit fits there. Needs decisions on failure handling, env vars passed in, and per-environment versus per-target hooks. |
| ProtonDB lookup in the CLI | UX | **3** | | Check a game's compatibility information before launching or changing runners | Resolve a wineo environment to the right ProtonDB game, handle missing matches, and make the result reusable by the future GUI. The command shape and online behavior need a plan. |
| Tag breakdown in `wineo stats` | UX | **3** | 0029, 0033 | See playtime and membership by tag, like how much time went into every `game` | Suggested by the maintainer (2026-09-27). Shape is open: `wineo stats --tag game` for one tag or a flag for all of them, or a `stats tags` form. `stats [NAME]` already takes an environment name, so a bare `tags` word would clash with an environment called `tags`. Needs a plan. |
| Manpages | UX | **3** | 0015 | `man wineo` is the first thing many Linux users try | Generate them from the clap definitions with `clap_mangen`, like completions, so they never drift from `--help`. The PKGBUILD installs them to `/usr/share/man/man1`. |
| Steam shortcuts | UX | **3** | | Add wineo targets to Steam as non-Steam games, for Big Picture and Steam Deck | Steam's `shortcuts.vdf` is binary and Steam must be closed while it's edited. |
| Integrity checking | Portability/Data | **4** | | Detect damaged or tampered files | Scope still open: prefix or game files, snapshots, or re-verifying downloads (already sha256-checked on install). |
| Import from Lutris/Bottles and Heroic store metadata | Compatibility | **3** | 0008 | Extends generic import and Heroic sideloaded-game import | 0008 covers manually added Heroic games only. Store integration and other launchers remain separate work. |
| Managed copying of portable application folders | Portability/Data | **3** | 0007, 0008 | Optional organization for applications used in place today | 0008 copies prefixes but leaves external game folders where they are. Storage layout and removal semantics need a later plan. |
| wineo-gui | UX | **1** | [GUI entry criteria](#gui-entry-criteria) | Build on a mature CLI and shared core operations | iced. Plans [0036](plans/0036-gui-library-launcher.md) (library launcher), [0037](plans/0037-gui-settings.md) (settings), and [0038](plans/0038-gui-new-import-clone.md) (new, import, clone) are done. See [Desktop app](gui.md). Next are removal, trash, restore, prune, and validate, then snapshots, portable copies, general art and download management, exec, and winetricks. |
| GUI visual polish | UX | **2** | 0036 | Make the desktop app feel finished rather than spike-grade | From the maintainer's 0036 testing (2026-09-28): redesign the Play, Stop, and Log buttons, which look like spike or built-in styles, and give them one consistent shape (Log differs from Play and Stop today). Give the detail panel some depth, like a shadow or elevation, and animate it opening and closing. From 0038 testing (2026-09-30): lay out the new environment review more nicely, since it's a plain list of lines today. |
| GUI context menu and actions | UX | **2** | 0036 | Reach per-environment actions without the CLI | 0038 adds Settings and Clone to the card menu and detail actions dropdown. Other actions, including remove, run an exe, and browse art, remain future GUI work. |
| GUI themes and customization | UX | **3** | 0036 | Let people make the app their own | From the maintainer's 0036 testing (2026-09-28): custom themes with a few presets, added with a GUI settings page. Longer term, a deeply customizable GUI. The palette tokens from 0036 are the starting point. |
| GUI library views | UX | **3** | 0036 | Browse the library the way that fits the moment | A zoom slider to make the grid bigger or smaller, a list view as an alternative to the grid, and a coverflow-style picker that suits a controller (pairs with gamepad navigation). |
| Quiet Wine core dumps on cancel | Compatibility | **4** | | Canceling mid-winetricks can make a half-started Wine process dump core | Seen in 0036 acceptance: Proton-CachyOS's 32-bit `wine cmd /c echo init` (winetricks' prefix probe) got Wine's teardown SIGQUIT before installing its handler, and systemd-coredump raised a desktop notification. Harmless. A zero core-size limit on Wine children would silence it; revisit only if it's annoying in real use. |
| Artwork from SteamGridDB | UX | **4** | wineo-gui | Posters, backgrounds, and icons for games, which a GUI can show | 0021 landed the art store, the `wineo art` commands, and SteamGridDB fetching, and 0024 landed desktop entry icons; see [Artwork](art.md). Remaining: showing posters and heroes in wineo-gui. |
| Full backup and restore | Portability/Data | **4** | 0018 | Great eventually, but this could become a whole subsystem | Covers filesystems without snapshot support. |
| macOS port | Compatibility | **4** | | Run wineo and wineo-gui on Apple Silicon Macs with Wine, CrossOver-style Wine builds, and Game Porting Toolkit | Requested by the maintainer (2026-09-30). An audit on 2026-09-26 found the build breaks on Linux-only calls: pidfd in process ownership, `statx` mount identity in trash, `ioctl_ficlone` in import and portable, and libdbus for the keyring. Rough order: a cfg-gated build with Keychain and Heroic's macOS paths, then process ownership (`kern.boottime`, `proc_pidinfo`, kqueue through a safe crate, since `unsafe_code` is forbidden) and trash mount identity. rustix's `fclonefileat` and `renameatx_np` keep the clone and exchange designs. umu/Proton, gamemode, DXVK/vkd3d, the Linux downloads, and `.desktop` entries don't apply; a Mac needs D3DMetal/DXMT runners and app bundles instead. iced already runs on macOS. Matching CrossOver (discussed 2026-10-01): its Wine changes are LGPL and published, and community macOS builds use them, so wineo should reuse existing builds like Heroic does instead of building Wine. D3DMetal comes from Apple's Game Porting Toolkit; point at the user's own install rather than bundling it until Apple's redistribution terms are checked. A per-game graphics backend choice (D3DMetal, DXMT, or DXVK over MoltenVK) fits templates and per-target settings. The real gap is per-game fixes: Linux gets protonfixes through umu, and macOS has no open equivalent, which is what CrossOver's recipes and testing provide. Risks to verify when planning: Rosetta 2's announced wind-down after macOS 27, and Whisky's 2025 discontinuation as a sign of the maintenance load. Needs a plan. |
| Event-driven supervisor exit watch | Core | **4** | | Only if there's literally nothing else to do | The supervisor polls `waitid` every 20 ms while a game runs. A pidfd plus a wakeup fd would make it sleep until the exit or a release request. Overhead today is already negligible. |

## GUI entry criteria

These change the config schema or core entry points the GUI builds on, so
they land first.

The GUI toolkit is iced, picked on 2026-09-28 after hands-on spikes of iced,
gpui, Slint, Godot, and Flutter.

All eight criteria were met on 2026-09-28.

1. Add native Linux games, so the features after it know some environments have no prefix.
2. Add `wineo validate`.
3. Add playtime statistics.
4. Make prefix deletion recoverable with trash and `prune`.
5. Add Wine Wayland and Gamescope/MangoHud support, including supported Heroic import settings.
6. Add declarative winetricks dependencies.
7. Add tags and `wineo list --tag`.
8. Complete the GUI-facing core API review, including a simplification pass that moves CLI logic into core, and resolve any blockers it finds.
