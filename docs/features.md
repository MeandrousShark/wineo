# Features

Feature backlog and queued work. When a feature gets picked up, it gets its
own numbered plan in `docs/plans/`.

## Queue

The order the maintainer picked on 2026-10-06. Each item gets its own plan
when it comes up, and the rows below have the details.

1. `--dry-run` for import and non-guided `new`: drafting as
   [0044](plans/0044-dry-run.md).
2. Tag breakdown in `wineo stats`, before store games add their own playtime.
3. Pre-launch and post-exit hooks, while the supervisor is the only launch
   model.
4. Store library import, Steam installed games first. It adds the proxied
   environment kind.
5. Store library follow-ups: owned but uninstalled Steam games, then GOG and
   Epic through Heroic.
6. GUI library views, designed against a library that includes store games.
7. GUI themes, last, so they cover palette roles the store work adds.

## Backlog

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
Completed: [0039: GUI removal, trash, and validate](plans/0039-gui-trash-and-validate.md).
See [Trash](gui.md#trash) and [Checkup](gui.md#checkup) for the pages.

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
| `--dry-run --json` | Automation | **4** | 0044 | Let a script read a dry run's plan instead of grepping review lines | Left out of [0044](plans/0044-dry-run.md) by the maintainer (2026-10-06). Add it if a script ever needs it. |
| Pre-launch and post-exit hooks | Core | **2** | | Run a user command before a target starts or after it exits, e.g. mounting a drive or syncing saves | The supervisor already owns the process lifetime, so post-exit fits there. Needs decisions on failure handling, env vars passed in, and per-environment versus per-target hooks. |
| ProtonDB lookup in the CLI | UX | **3** | | Check a game's compatibility information before launching or changing runners | Resolve a wineo environment to the right ProtonDB game, handle missing matches, and make the result reusable by the future GUI. The command shape and online behavior need a plan. |
| Tag breakdown in `wineo stats` | UX | **3** | 0029, 0033 | See playtime and membership by tag, like how much time went into every `game` | Suggested by the maintainer (2026-09-27). Shape is open: `wineo stats --tag game` for one tag or a flag for all of them, or a `stats tags` form. `stats [NAME]` already takes an environment name, so a bare `tags` word would clash with an environment called `tags`. Needs a plan. |
| Manpages | UX | **3** | 0015 | `man wineo` is the first thing many Linux users try | Generate them from the clap definitions with `clap_mangen`, like completions, so they never drift from `--help`. The PKGBUILD installs them to `/usr/share/man/man1`. Done (2026-10-05) as a visible `wineo manpages DIR` command, which writes one page per visible command. |
| Store library import (Steam, GOG, Epic) | UX | **3** | wineo-gui | Show whole store libraries in wineo, installed or not, and launch through the store's launcher, like Playnite | From the maintainer (2026-10-02). Steam: installed games come from `libraryfolders.vdf` and each library's `appmanifest_<appid>.acf` (native `~/.steam/steam` and Flatpak `~/.var/app/com.valvesoftware.Steam`). Owned but uninstalled games need the Steam Web API's `GetOwnedGames` with a key and SteamID64 kept in the keyring, or Playnite's no-key fallback for public profiles. Launch with `steam://rungameid/<appid>` (or `steam -applaunch <appid> <args>` to pass arguments), install with `steam://install/<appid>` (Steam's own dialog), and watch the manifest's state and downloaded bytes for install progress. Steam starts the game, so running status, Stop, and playtime have to find the `reaper SteamLaunch AppId=<id>` process instead of supervising it. Env vars and wrappers only reach the game through its Steam launch options (`VAR=1 %command%` in `localconfig.vdf`, editable only while Steam is closed). GOG and Epic come through Heroic, which already caches each store's full library (installed or not) and launches with `heroic://launch`; going to GOG's own API directly is out of scope. Epic is a lower priority for the maintainer but rides along. Freebies: Steam art from `appcache/librarycache/<appid>/` without a SteamGridDB key, store playtime to import like 0029 does for Heroic, and SteamGridDB lookup by app ID. Settled: a new proxied environment kind, so wineo overrides can apply where the store allows (no snapshots, winetricks, exec, clone, or portable copies, since the store owns the prefix); installed games are scanned live and the owned list is cached and refreshed; uninstalled games show grayscaled and dimmed; Heroic store games use the same kind. Covers the Heroic half of the Lutris/Bottles/Heroic row. Needs a plan. |
| Steam shortcuts | UX | **3** | | Add wineo targets to Steam as non-Steam games, for Big Picture and Steam Deck | Steam's `shortcuts.vdf` is binary and Steam must be closed while it's edited. |
| Integrity checking | Portability/Data | **4** | | Detect damaged or tampered files | Scope still open: prefix or game files, snapshots, or re-verifying downloads (already sha256-checked on install). |
| Import from Lutris/Bottles and Heroic store metadata | Compatibility | **3** | 0008 | Extends generic import and Heroic sideloaded-game import | 0008 covers manually added Heroic games only. Store integration and other launchers remain separate work. |
| Managed copying of portable application folders | Portability/Data | **3** | 0007, 0008 | Optional organization for applications used in place today | 0008 copies prefixes but leaves external game folders where they are. Storage layout and removal semantics need a later plan. |
| wineo-gui | UX | **1** | [GUI entry criteria](#gui-entry-criteria) | Build on a mature CLI and shared core operations | iced. Plans [0036](plans/0036-gui-library-launcher.md) (library launcher), [0037](plans/0037-gui-settings.md) (settings), and [0038](plans/0038-gui-new-import-clone.md) (new, import, clone) are done. See [Desktop app](gui.md). [0039](plans/0039-gui-trash-and-validate.md) (removal, trash, validate) is done too. [0040](plans/0040-gui-art-and-downloads.md) (art and downloads) is done too. [0041](plans/0041-gui-snapshots-portable-exec-winetricks.md) (snapshots, portable copies, exec, and winetricks) is done too. [0042](plans/0042-gui-visual-polish.md) (visual polish) is done too. |
| GUI visual polish | UX | **1** | 0036 | Make the desktop app feel finished rather than spike-grade | [0042](plans/0042-gui-visual-polish.md) is done (0.23.0). It added compact detail actions, panel depth and animation, playful spring motion between library slots, grouped new-environment reviews, and matching Settings, Snapshots, Downloads, Checkup, and Trash pages. The [GUI style guide](gui-style.md) records the approved direction, including smaller library posters. |
| GUI context menu and actions | UX | **2** | 0036 | Reach per-environment actions without the CLI | Implemented across 0038-0041: Settings, Art, Clone, Checkup, Remove, Snapshots, portable copies, Run program, and Winetricks. [0042](plans/0042-gui-visual-polish.md) refines menu styling and bounds long action/reason content while keeping Close visible. |
| GUI themes and customization | UX | **3** | 0036 | Let people make the app their own | From the maintainer's 0036 testing (2026-09-28): custom themes with a few presets, added with a GUI settings page. Longer term, a deeply customizable GUI. The palette tokens from 0036 are the starting point. |
| GUI library views | UX | **3** | 0036 | Browse the library the way that fits the moment | A zoom slider to make the grid bigger or smaller, a list view as an alternative to the grid, and a coverflow-style picker that suits a controller (pairs with gamepad navigation). |
| Library names from desktop entries | UX | **3** | 0036 | Show each card with the name people know the program by | Added to [0042](plans/0042-gui-visual-polish.md) during acceptance on 2026-10-03: use the default target's explicit enabled desktop title, fall back to the environment name, and search both. Keep the identifier for operations and show it as muted context. Done in 0.23.0. |
| Mouse back button in the GUI | UX | **3** | 0036 | Mouse side buttons should step back like a browser | From the maintainer (2026-10-02). iced reports `mouse::Button::Back` and `Forward`, and Escape in `keys.rs` already closes the log, then the panel, then clears search, so Back can likely reuse that chain. Done (2026-10-05): settled as Back equals Esc on every page, including full-page flows and dialogs, and Forward does nothing. See [Keyboard](gui.md#keyboard). |
| Other lifecycle timing failures under load | Core | **3** | | A test that fails under load hides real regressions | The crashed Proton installer failure was fixed in 0.22.1: an exiting process could return `ESRCH` or a dead-task stat with cleared group fields during a `/proc` scan, which cleanup incorrectly treated as unreadable. The race predates 0040's build locks. Another timing test failed once during 0040 verification, but its name and failure output were not recorded. On 2026-10-05 a 64-thread run caught `a_proton_run_ends_when_umu_exits` reading `status` after the state file said failed but before the supervisor released its run lock; it and two tests reading the play log the same way now wait for the lock too. If that was the 0040 failure, this row can close; otherwise capture the next one before changing its timing. Also seen: `installer::tests::a_token_cancels_an_installer_without_a_signal` failed in full workspace runs on 2026-10-02 and 2026-10-05 (`installer.run` returned an error at installer.rs:725 instead of `Canceled`), passing alone and in 15 64-thread loops; its error text wasn't captured yet. |
| Runners from Steam, Heroic, and Lutris | Compatibility | **3** | 0003, 0004 | Use Proton and Wine builds that are already installed instead of downloading them again | [0043](plans/0043-external-runners.md) is done (0.24.0); see [Use builds from Steam, Heroic, and Lutris](downloads-and-imports.md#use-builds-from-steam-heroic-and-lutris). Before 0043, wineo only scans its own runners root. Other places: `~/.steam/root/compatibilitytools.d`, `~/.steam/steam/steamapps/common` (Valve's Proton), Heroic's `tools/proton` and `tools/wine` under its config dir (native or Flatpak, which `heroic.rs` already finds), and `~/.local/share/lutris/runners/wine`. Settled by the maintainer (2026-10-02): scanning is opt-in, offered once during the first environment add when it's a game; when a name exists in several places, wineo's own root wins, then Steam, then Heroic, then Lutris; Downloads never deletes builds wineo didn't install. The order is mostly arbitrary: copies of the same build with the same contents are interchangeable, so it only matters when same-named builds differ. Hashing a whole runner tree is slow, so the plan should pick a cheap identity check (for example a hash of the version file plus the main binaries). |
| Quiet Wine core dumps on cancel | Compatibility | **4** | | Canceling mid-winetricks can make a half-started Wine process dump core | Seen in 0036 acceptance: Proton-CachyOS's 32-bit `wine cmd /c echo init` (winetricks' prefix probe) got Wine's teardown SIGQUIT before installing its handler, and systemd-coredump raised a desktop notification. Harmless. A zero core-size limit on Wine children would silence it; revisit only if it's annoying in real use. Done (2026-10-05, 0.23.1): children wineo can cancel get a soft core limit of exactly 1, since systemd-coredump still runs and notifies at 0. Game launches keep normal limits. |
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
