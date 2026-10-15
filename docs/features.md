# Features

Feature backlog and queued work. When a feature gets picked up, it gets its
own numbered plan in `docs/plans/`.

## Queue

The order the maintainer picked on 2026-10-14, after 0051. Each item gets
its own plan when it comes up, and the rows below have the details.

1. GOG Galaxy builds: [0052](plans/0052-galaxy-builds.md), approved 2026-10-14.
2. The 1.0 contract: the JSON gaps (criterion 3), and the config
   compatibility guarantee with a deprecation path (criteria 4 and 5).
3. GUI library views, designed against a library that includes store games.
4. GUI themes, after library views, so they cover every palette role.
5. 1.0, once the [1.0 criteria](#10-criteria) are met.
6. Release packages (deb, rpm, AppImage), shortly after 1.0.

The previous queue (2026-10-06) is done: `--dry-run` (0044), tags in
`stats` (0045), hooks (0046), Steam installed and owned games (0047,
0048), GOG account, migration, and updates (0049), GOG installs (0050),
and the lean-and-simple pass (0051).

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
Completed: [0040: GUI art and downloads](plans/0040-gui-art-and-downloads.md),
[0041: GUI snapshots, portable copies, exec, and winetricks](plans/0041-gui-snapshots-portable-exec-winetricks.md),
and [0042: GUI visual polish](plans/0042-gui-visual-polish.md), which with
0036 to 0039 make up wineo-gui, its context menu, its posters, and library
names from desktop entries.
Completed: [0043: Runners from Steam, Heroic, and Lutris](plans/0043-external-runners.md),
[0044: `--dry-run`](plans/0044-dry-run.md),
[0045: Tags in `wineo stats`](plans/0045-stats-by-tag.md), and
[0046: Launch hooks](plans/0046-launch-hooks.md).
Completed: store libraries in [0047](plans/0047-steam-library.md),
[0048](plans/0048-steam-owned-games.md), [0049](plans/0049-gog-games.md),
and [0050](plans/0050-gog-installs.md), and
[0051: Lean and simple](plans/0051-lean-and-simple.md), which also made the
supervisor wait on events instead of polling.
Completed without a plan: manpages (`wineo manpages`, 2026-10-05), the
mouse back button in the app (2026-10-05), quiet Wine core dumps on cancel
(0.23.1), and the lifecycle timing failures under load (2026-10-13).

**Category**

- Core: makes Wineo meaningfully better at managing environments
- Compatibility: runners, winetricks, imports, graphics/runtime plumbing
- UX: guided flows, completion, desktop entries, GUI
- Automation: scripting/output behavior
- Portability/Data: clone, portable layouts, schema/migration, snapshots, backups
- Distribution: packages and installs
- Perf: resource use
- Testing: test reliability and coverage

**Priority**

Redefined by the maintainer on 2026-10-14, now that the foundation is in,
around the next milestone, 1.0:

- 1 = next: on the road to 1.0, either a [1.0 criterion](#10-criteria) or
  something a public 1.0 shouldn't ship without
- 2 = soon: early 1.x, polish people meet in daily use, small and high
  value
- 3 = later: wanted, but bigger, or nothing is asking for it yet
- 4 = someday: parked until a real need shows up

**Depends on** lists what has to be built first. `0001 phase 2` is the second half of the foundation plan, which has landed, so the rows naming it are unblocked.

| Feature | Category | Priority | Depends on | Why | Notes |
|---|---|---:|---|---|---|
| GOG Galaxy builds | Compatibility | **1** | 0050 | Install and update GOG Windows games through Galaxy, so updates download only what changed, with branches and rollback | [0052](plans/0052-galaxy-builds.md). Closes 1.0 criterion 8. |
| JSON contract gaps | Automation | **1** | | Close the three code gaps the 1.0 `--json` audit found | 1.0 criterion 3: the docs side is done (2026-10-12), and three gaps in the code wait on the maintainer. |
| Config compatibility guarantee and deprecation path | Portability/Data | **1** | 0018 | Promise that 1.x configs keep loading, and give renamed keys and flags a warning period | 1.0 criteria 4 and 5, planned together: upgrade tests with fixture configs from earlier releases, and `validate` reporting deprecated keys. Needs a decision on newer keys, since `deny_unknown_fields` makes an older wineo reject them. |
| GUI themes and customization | UX | **1** | 0036 | Let people make the app their own | From the maintainer's 0036 testing (2026-09-28): custom themes with a few presets, added with a GUI settings page. Longer term, a deeply customizable GUI. The palette tokens from 0036 are the starting point. Moved up to 1 by the maintainer (2026-10-14), to land before 1.0. |
| GUI library views | UX | **1** | 0036 | Browse the library the way that fits the moment | A zoom slider to make the grid bigger or smaller, a list view as an alternative to the grid, and a coverflow-style picker that suits a controller (pairs with gamepad navigation). A poster-only view like Heroic's (from the maintainer, 2026-10-09): titles hidden until hover, then shown wrapped over the poster, with keyboard focus revealing them too. Titles and state/playtime sit beneath the art today per `gui-style.md`, so this view amends the style guide on purpose, and art-less environments need their text tiles to stay readable. Moved up to 1 by the maintainer (2026-10-14), to land before 1.0. Also from the Playnite comparison (2026-10-14): sorting (last played, playtime, date added, name), filtering and grouping (tags, source, state) with saved filter presets, and favorites and hidden games, since they're the same screen and new config keys are best added before 1.0. |
| GitHub mirror and CI | Distribution | **1** | | Publish wineo at 1.0 while the maintainer keeps working in Gitea | From the maintainer (2026-10-14). Gitea stays primary and push-mirrors to GitHub, tags included. Changes still land by merging into main directly; PRs are for other people. Since the mirror force-pushes GitHub to match Gitea, an outside PR is merged locally and pushed to Gitea, never with GitHub's merge button; GitHub marks it merged when its commits reach main unchanged (a squash or rebase means closing it by hand). GitHub Actions runs fmt, clippy, and the normal tests on outside PRs (no secrets for forks) and on tags, and a tag also builds release binaries for a GitHub Release, where the release packages row's packages land later. Not on every push to main, which is reviewed and tested before merging. `--ignored` tests stay local: they need Wine, umu, and a display. Tags from the mirror trigger Actions since Gitea pushes with a personal access token. The mirror syncs on every push, not just on its interval. The published AUR PKGBUILD clones the GitHub mirror over HTTPS, since the Gitea SSH source only works for the maintainer; a local PKGBUILD can keep using Gitea. Confirmed Gitea primary over GitHub primary on 2026-10-14. |
| Edit hooks in GUI Settings | UX | **2** | 0046 | Set a game's hooks without editing TOML or using `wineo set` | Left out of [0046](plans/0046-launch-hooks.md) by the maintainer (2026-10-06). The GUI already runs hooks. |
| Bulk tagging | UX | **2** | 0033 | Add or remove a tag on many environments at once instead of one `wineo set` per environment | Suggested by the maintainer during 0045 acceptance (2026-10-07), mostly for the GUI. Today `tags=[...]` replaces the whole list, and GUI tags are edited one environment at a time in Settings. Likely shape: `wineo tag add TAG NAME...` and `wineo tag remove TAG NAME...`, plus multi-select in the Library with an add/remove tag action. The GUI part needs a design pass on how multi-select works with the grid and keyboard. |
| Release packages (deb, rpm, AppImage) | Distribution | **2** | | Install wineo on Debian, Ubuntu, Fedora, and other distros without building it, shortly after 1.0 | From the maintainer (2026-10-09). deb and rpm first, through `cargo-deb` and `cargo-generate-rpm` metadata in Cargo.toml, installing what the PKGBUILD installs (both binaries, desktop file, icon, manpages, completions, example configs). AppImage after that, built on an old distro for a low glibc floor, without bundling GPU libraries. It needs wineo to know where it's installed: `current_exe()` re-execs (supervisor in `start.rs`, GUI worker, `supervisor/steam.rs`, winetricks) would point into the AppImage's temporary mount, so they should use `$APPIMAGE`, and desktop entries' `Exec=wineo ...` assumes `wineo` is on `PATH`. One AppImage has one entrypoint, so the CLI needs a dispatch. Flatpak is out: its sandbox blocks spawning Wine and umu, scanning `/proc` for Steam's reaper, and reading `~/.steam`. Needs a plan. |
| GUI accessibility settings | UX | **2** | 0036 | Make the desktop app comfortable for people who need larger text, other fonts, or less motion | From the maintainer (2026-10-09), after finding that semibold text falls back to a monospace font on their machine. The patch fix after 0048 bundles a default sans font; this row lets people pick their own font and text size on top of it, and fits with reduced motion and high-contrast palettes. Text measurement for balanced titles already uses the rendered font, so a font change keeps wrapping correct, but fixed tile and card heights would need to follow the text size. Pairs with GUI themes (queue item 7). Needs a plan. |
| Keyboard focus for list rows | UX | **2** | 0036 | Reach choice lists from the keyboard | From 0048 acceptance (2026-10-10). The quiet list rows (`ui::list_row`) in **Add**, New's program list, and Heroic's source list take hover and press but never keyboard focus, like the stock buttons they replaced. **Add** has its 1 to 4 keys, but the program and source lists need the mouse. Wrap the rows in `focus_button` controls with arrow keys and the usual primary focus ring, which changes focus order on those pages. Fits with GUI accessibility settings. |
| Embedded GOG login window | UX | **2** | 0049 | Log in to GOG without copying the success address back | From 0049 acceptance (2026-10-12). GOG's login only redirects to `embed.gog.com/on_login_success`, so today you copy that address from the browser and paste it into wineo. Heroic and Minigalaxy open the login page in their own browser window and catch the redirect there. iced can't show web pages, so this needs a WebKitGTK webview (for example `wry`) in a small helper window: a large GTK dependency for a one-time login, and more to bring to a macOS port. Watching the clipboard for the address was turned down for privacy. Needs a plan. |
| One shape for confirm prompts | UX | **2** | | Ask every review's final yes or no the same way | From 0049 acceptance (2026-10-12). The GOG migrate and update confirms now print one keys line, then `Migrate it? [Y/n]:`. Others still differ: the Steam import's game step prints `Press Enter to import it, Esc to go back, Ctrl+C to cancel.` before `Import it? [Y/n]:`, and an empty `review` (`wineo new`) asks `Create it?` with no `[Y/n]`. Survey every confirm and settle on one. |
| Display titles for every card | UX | **2** | | Show a game's real title on its library card, not its environment name | Seen 2026-10-12: RimWorld and palworld cards show `rimworld` and `palworld` while imported games show titles like "Disco Elysium - The Final Cut". A migrated GOG game knows its title from GOG's product details, so its card could use that; other environments could take a title from Steam, art, or a `title` key. |
| Completion status and notes | UX | **2** | | Track where you are with each game, like Playnite's Playing, Beaten, Completed, Abandoned, and Plan to play | A status and free-form notes per environment, shown in the panel and filterable once library views can filter. From the Playnite comparison (2026-10-14). New config keys, so before 1.0 if possible. |
| "Started" hook and launch variables | Core | **2** | 0046 | Run something once the game is up, and write paths in targets that work anywhere | A third hook beside `before` and `after` that runs once the target has started (the place a companion like GOG's comet would go), and variables like `{install_dir}` or `{prefix}` in a target's `exe`, `args`, and `cwd`. From the Playnite comparison (2026-10-14). |
| Tray icon and hide on launch | UX | **2** | 0036 | Get the app out of the way while a game runs, and reach common actions without opening it | A StatusNotifierItem tray icon (over D-Bus; the app already has zbus) with a menu: open the library, add something, and stop what's running. A setting hides the window to the tray when a game starts and brings it back when the game exits. From the maintainer (2026-10-14). From the Playnite comparison (2026-10-14). Hyprland has no minimize, so hiding needs the tray; test with a bar that shows tray icons. |
| ProtonDB lookup in the CLI | UX | **3** | | Check a game's compatibility information before launching or changing runners | Resolve a wineo environment to the right ProtonDB game, handle missing matches, and make the result reusable by the future GUI. The command shape and online behavior need a plan. |
| Store library import: Epic | UX | **3** | wineo-gui | Show whole store libraries in wineo, installed or not, and launch through the store's launcher, like Playnite | From the maintainer (2026-10-02). Steam: installed games come from `libraryfolders.vdf` and each library's `appmanifest_<appid>.acf` (native `~/.steam/steam` and Flatpak `~/.var/app/com.valvesoftware.Steam`). Owned but uninstalled games need the Steam Web API's `GetOwnedGames` with a key kept in the keyring (no keyless route works anymore, see [0048](plans/0048-steam-owned-games.md)). Launch with `steam://rungameid/<appid>` (or `steam -applaunch <appid> <args>` to pass arguments), install with `steam://install/<appid>` (Steam's own dialog), and watch the manifest's state and downloaded bytes for install progress. Steam starts the game, so running status, Stop, and playtime have to find the `reaper SteamLaunch AppId=<id>` process instead of supervising it. Env vars and wrappers only reach the game through its Steam launch options (`VAR=1 %command%` in `localconfig.vdf`, editable only while Steam is closed). Changed by the maintainer (2026-10-09): GOG goes direct, without Heroic, so store libraries don't depend on it. GOG games are DRM-free, so an installed GOG game can be a normal wineo environment rather than proxied: log in through GOG's OAuth (the Galaxy client ID that Heroic's gogdl, Minigalaxy, and lgogdownloader use, which is unofficial), list owned games from the account endpoints, and install by running the game's offline installer into a new prefix (native `.sh` installers tie into the native installers idea). Epic moved to its own backlog row. Freebies: Steam art from `appcache/librarycache/<appid>/` without a SteamGridDB key, store playtime to import like 0029 does for Heroic, and SteamGridDB lookup by app ID. Settled: a new proxied environment kind, so wineo overrides can apply where the store allows (no snapshots, winetricks, exec, clone, or portable copies, since the store owns the prefix); installed games are scanned live and the owned list is cached and refreshed; uninstalled games show grayscaled and dimmed. [0047](plans/0047-steam-library.md) covers Steam installed games. Needs a plan. Steam is done in [0047](plans/0047-steam-library.md) and [0048](plans/0048-steam-owned-games.md), GOG in [0049](plans/0049-gog-games.md), [0050](plans/0050-gog-installs.md), and [0052](plans/0052-galaxy-builds.md); Epic remains. |
| Steam shortcuts | UX | **3** | | Add wineo targets to Steam as non-Steam games, for Big Picture and Steam Deck | Steam's `shortcuts.vdf` is binary and Steam must be closed while it's edited. |
| Import from Lutris/Bottles and Heroic store metadata | Compatibility | **3** | 0008 | Extends generic import and Heroic sideloaded-game import | 0008 covers manually added Heroic games only. Store integration and other launchers remain separate work. |
| Managed copying of portable application folders | Portability/Data | **3** | 0007, 0008 | Optional organization for applications used in place today | 0008 copies prefixes but leaves external game folders where they are. Storage layout and removal semantics need a later plan. |
| Bundled fonts in GUI layout tests | Testing | **3** | | Make headless layout tests measure text the same on every machine | From the 0048 patch batch (2026-10-10): the GUI bundles Noto Sans, but `layout_probe` and the tile title tests still shape text with whatever system fonts are installed, so `PKGBUILD` checks on other distros can measure differently. Loading the bundled fonts into the shared font system for tests shifts every measured width once, so expected layouts need a review pass. The maintainer deferred it out of the patch batch. |
| Supervisor force-stop deadline | Core | **3** | | Keep a hung `wineserver -k` from stalling a run's give-up deadline | From 0049 step 4's review (2026-10-11). `supervisor.rs` calls `kill_live` synchronously, so if `wineserver -k` hangs, the 10s give-up bound for `start`, the installer, and GOG execs is never checked. Older than step 4 and only reachable with a broken runner. Running the force step on its own thread with a timeout would fix it. |
| Interruptible GOG requests | UX | **3** | 0049 | Make a cancel stop a GOG or SteamGridDB request already in flight | Left out of [0049](plans/0049-gog-games.md) ("Where canceling stops"). A cancel is honored at checkpoints, so a request already sent waits for its answer: up to 30 s in the worst case, the GOG client's request timeout in `crates/wineo-gog/src/client.rs`. Canceling the blocking request itself, or running it on a thread the cancel can abandon, would make Cancel and Ctrl+C feel instant on a slow network. |
| Move a prefix | UX | **3** | | Relocate an environment's prefix in one command | From removing Heroic (2026-10-12): Disco Elysium's prefix, adopted in place by GOG migration, still lived in Heroic's folder, and moving it took a manual `mv` plus `wineo set prefix=`. A command could check the environment is idle, rename on one filesystem or copy across, update the config, and refuse when snapshots or other environments depend on the old path. |
| Emulated games | UX | **3** | 0026 | Add and launch games from emulators like Dolphin, RetroArch, and Cemu, with playtime, art, and desktop entries | From the maintainer (2026-10-14), who may pull it earlier. A native environment can already launch one by hand, since a target is `exe`, `args`, and `cwd`: `dolphin-emu -b -e GAME`, `retroarch -L CORE ROM`, `cemu -g GAME`. The feature adds the rest: a guided flow that finds installed emulators (native or Flatpak) and fills in their arguments, a RetroArch core picker, and importing a ROM folder per system through the import grid. Emulators must exit when the game does (Dolphin's `-b`), or playtime keeps counting. Open questions: one environment per game or per emulator, Flatpak emulators under `flatpak run`, and per-game emulator config (Dolphin `-u`, RetroArch `--config`). Needs a plan. |
| Playtime tracking by folder or process name | Core | **3** | | Keep counting playtime when a launcher hands off to a game process it doesn't wait for | Playnite tracks the launched process, any process running from the install folder, or a process by name. wineo follows the process group, plus its handoff check for native launchers. Matters most for future store launchers like Epic's. From the Playnite comparison (2026-10-14). |
| Game metadata from the stores | UX | **3** | | Show a description, genres, developer, publisher, release date, and links in the panel | GOG's and Steam's APIs, which wineo already calls, have most of it; IGDB would cover the rest. From the Playnite comparison (2026-10-14). |
| `--dry-run --json` | Automation | **4** | 0044 | Let a script read a dry run's plan instead of grepping review lines | Left out of [0044](plans/0044-dry-run.md) by the maintainer (2026-10-06). Add it if a script ever needs it. |
| Global hooks | Core | **4** | 0046 | Run the same hook around every game, like turning notifications off while anything plays | Left out of [0046](plans/0046-launch-hooks.md) by the maintainer (2026-10-06): they'd surprise people using wineo for regular software, and templates carry hooks for new games. |
| Epic Games Launcher setup | Compatibility | **4** | | Install the official Epic Games Launcher into its own environment in a few steps, and let Epic games live there | From the maintainer (2026-10-09). The launcher manages its own games inside that environment, and wineo doesn't list them one by one. Epic integration is Heroic's original purpose, so wineo leaves that space to Heroic. Needs a plan. |
| Edit Steam launch options | UX | **4** | 0047 | Set environment variables and wrappers like MangoHud for a Steam game from wineo | Left out of [0047](plans/0047-steam-library.md) by the maintainer (2026-10-07): only arguments reach a Steam game through `steam -applaunch`. Env vars and wrappers live in Steam's launch options (`VAR=1 %command%` in `localconfig.vdf`), which can only be edited while Steam is closed, and wineo stays read-only toward Steam's files. |
| Cheaper Steam refresh in the desktop app | Perf | **4** | 0047 | Keep the library's 2-second refresh light for people with many Steam games | Found in 0047's review (2026-10-09). Each refresh already reads Steam's manifests and `config.vdf` once, but still parses Steam's playtime files and scans `/proc` for a reaper once per Steam card. Share one playtime read and one `/proc` scan per refresh, or refresh Steam data less often than run state. [0051](plans/0051-lean-and-simple.md) took most of the cost away: no rescans while the app is unfocused, and one cheaper `/proc` sweep per read. |
| Integrity checking | Portability/Data | **4** | | Detect damaged or tampered files | Scope still open: prefix or game files, snapshots, or re-verifying downloads (already sha256-checked on install). |
| Race-free process group waits | Core | **4** | | Stop installer runs, hooks, and Proton run settling from calling a process group empty while a member is still running | From 0049 step 4 (2026-10-11). `process::group_has_live_members` lists `/proc` and then reads each entry, so a member that forks and exits between the two hides its new child, and a thread leader that exits while other threads run looks like a zombie. Only tight fork loops hit it. Exec moved to wineserver's idle wait instead. A real fix likely needs cgroups or pidfd tracking. The maintainer parked it rather than patching. |
| Full backup and restore | Portability/Data | **4** | 0018 | Great eventually, but this could become a whole subsystem | Covers filesystems without snapshot support. |
| macOS port | Compatibility | **4** | | Run wineo and wineo-gui on Apple Silicon Macs with Wine, CrossOver-style Wine builds, and Game Porting Toolkit | Requested by the maintainer (2026-09-30). An audit on 2026-09-26 found the build breaks on Linux-only calls: pidfd in process ownership, `statx` mount identity in trash, `ioctl_ficlone` in import and portable, and the D-Bus Secret Service keyring. Rough order: a cfg-gated build with Keychain and Heroic's macOS paths, then process ownership (`kern.boottime`, `proc_pidinfo`, kqueue through a safe crate, since `unsafe_code` is forbidden) and trash mount identity. rustix's `fclonefileat` and `renameatx_np` keep the clone and exchange designs. umu/Proton, gamemode, DXVK/vkd3d, the Linux downloads, and `.desktop` entries don't apply; a Mac needs D3DMetal/DXMT runners and app bundles instead. iced already runs on macOS. Matching CrossOver (discussed 2026-10-01): its Wine changes are LGPL and published, and community macOS builds use them, so wineo should reuse existing builds like Heroic does instead of building Wine. D3DMetal comes from Apple's Game Porting Toolkit; point at the user's own install rather than bundling it until Apple's redistribution terms are checked. A per-game graphics backend choice (D3DMetal, DXMT, or DXVK over MoltenVK) fits templates and per-target settings. The real gap is per-game fixes: Linux gets protonfixes through umu, and macOS has no open equivalent, which is what CrossOver's recipes and testing provide. Risks to verify when planning: Rosetta 2's announced wind-down after macOS 27, and Whisky's 2025 discontinuation as a sign of the maintenance load. Needs a plan. |
| Copy GOG game folders with snapshots, clones, and portable copies | Portability/Data | **4** | 0049 | Take a GOG game's files along when its environment is snapshotted, cloned, or copied to a portable folder | Left out of [0049](plans/0049-gog-games.md) (question 13). Today those copy the environment and prefix, and the GOG game folder stays where it is, shared by the copies and protected by its owner's folder state. Copying it needs a second folder state, its own owner, and rules for updates of the copy. Needs a plan. |
| GOG games in portable roots | Portability/Data | **4** | 0049 | Migrate and update GOG games inside a `WINEO_PORTABLE_DIR` folder | Left out of [0049](plans/0049-gog-games.md) (question 17): GOG game folders and their state live in the host root, and `--portable` refuses migrating and updating. Logging in and listing already work there. Needs folder states that travel with the portable folder. Needs a plan. |
| Use Heroic's native file list | Compatibility | **4** | 0049 | Know which files a migrated native GOG game shipped before its first update | Left out of [0049](plans/0049-gog-games.md). heroic-gogdl keeps `.gogdl-linux-manifest` (each file's name, size, CRC32, and whether it's a symlink) in the game folder. Importing it, checked against the folder, would give a migrated game winning files at once, so its first update could tell your changes from GOG's. Until then the first rebuild overwrites and lists files that differ. |
| Write through symlinked configs | Portability/Data | **4** | | Keep a config that's a symlink (a dotfiles repo, say) a symlink when wineo edits it | From the data-loss audit (2026-10-12), parked by the maintainer. `wineo set` and attaching a GOG game both replace the file with an atomic rename, which turns a symlinked `environments/<name>.toml` or `config.toml` into a regular file, so the link's target silently stops being the live config. Writing through the link (temp file beside the target, rename onto the target) would keep it. |
| Close the last nested-prefix gaps | Core | **4** | | Catch nested prefixes that slip past the creation checks | From the data-loss fixes (2026-10-12), where creating, cloning, importing, restoring, or moving an environment so its prefix nests inside another's became refused. Two gaps remain: two `new`s at the same moment can still nest into each other, since locks cover exact paths and not their ancestors, and `wineo edit` skips the prefix checks entirely. A `validate` check for nested prefixes would catch both, plus any nesting a library already has. |
| Achievements | UX | **4** | 0052, emulated games | Earn and see achievements for GOG and emulated games outside Steam | From the maintainer (2026-10-14), parked until it seems worth the trouble. Two integrations are enough: GOG through [comet](https://github.com/imLinguin/comet), an open-source stand-in for Galaxy's communication service that runs beside a Windows game under Wine or Proton and records real unlocks on the GOG account (GOG's Linux builds don't use the Galaxy SDK, so they have none), and RetroAchievements, which emulators like RetroArch, Dolphin, PCSX2, and DuckStation already support once logged in, with wineo showing progress through its web API. Check comet's status first: its first release was July 2024, with Heroic and Lutris support then planned. Not planned: manual checklists borrowed from a store's list, like Playnite's SuccessStory offers. |
| Random game picker | UX | **4** | 0036 | Pick something to play when you can't decide | A button that picks from the current filter. From the Playnite comparison (2026-10-14). |
| Controller fullscreen mode | UX | **4** | GUI library views | A whole controller-first mode for the couch, like Playnite's Fullscreen or Steam's Big Picture | Beyond the library views row's coverflow picker: every page reachable from a gamepad. From the Playnite comparison (2026-10-14). |

## 1.0 criteria

Approved by the maintainer on 2026-10-06. 1.0 is a promise, not a
feature count: after it, anything that breaks the public contract waits for
2.0, and features keep landing in 1.x. Wineo stays on 0.x until the
maintainer calls it stable, which these criteria are meant to inform.

1. Define the public contract in a doc page: config files, CLI commands and
   flags, exit codes, `--json` output, and where wineo keeps its files
   (including the portable layout). Human-readable output, the GUI, and
   `wineo-core`'s Rust API are not part of it. Done (2026-10-10): see
   [Stability](stability.md). The maintainer settled the borderline cases:
   desktop entries and the snapshot and trash layouts are internal, keyring
   item attributes and the settings merge order are stable, and the built-in
   `gaming` template's settings and shell completion output aren't promised.
2. Document exit codes for every command in one place. Today they're spread
   across feature pages, and only [Validate](validate.md) has a section.
   In progress (2026-10-06): an audit found 15 inconsistencies. The
   maintainer settled them: declining a final confirmation or backing out
   of the first guided question exits 130 like any cancel, `prune` with no
   selector is a usage error (2), a canceled direct or bulk Heroic import
   exits 130, and a closed pipe ends output quietly instead of panicking.
   After review: a closed pipe exits 141, like a SIGPIPE death, so a
   command cut short never looks successful, and Esc at the first question
   of any guided flow cancels, with the hint `Esc or Ctrl+C to cancel.`
   The rest are documented as they are. Done (2026-10-06): see
   [Exit codes](exit-codes.md).
3. Document a `--json` schema for every command that has `--json`, and say
   which changes count as compatible (new fields yes, renamed or removed
   fields no). [JSON output](json-output.md) covers most of this already.
   In progress (2026-10-12): an audit of every `--json` command found the
   page missing `hooks`, two validate codes, enum lists, and nullable
   fields; those are documented now, with a compatible-changes section.
   Three gaps are in the code, not the docs, and wait on the maintainer.
4. Make `schema_version` a guarantee: a config written by any 1.x release
   loads in every later 1.x, and an older one either loads or migrates
   cleanly. Add upgrade tests that load fixture configs and state written
   by earlier releases.
5. Add a deprecation path: a renamed or removed config key or flag keeps
   working in 1.x with a warning, and `wineo validate` reports it as
   deprecated. 2.0 is where those become errors.
6. Close or explain the "Other lifecycle timing failures under load" row,
   so a red test means a real regression.
   Done (2026-10-13): two root causes fixed in 0.30.1 and every test
   executable swept onto a child-process writer; see that row.
7. No known data-loss bugs in trash, snapshots, imports, clone, or portable
   copies.
   Audited (2026-10-12) by two reviewers from different model families;
   everything they found was fixed in 0.30.1, with a delete-by-delete
   inventory of trash and snapshots. Remaining limits are external races
   and the two rows under "Close the last nested-prefix gaps".
8. Land, or decide against, the last big shape change in view: the proxied
   environment kind from store library import (queue items 4 and 5).
   Settled (2026-10-12): the proxied kind landed in 0047 and GOG went
   direct. 0050 and 0052 ship as regular 0.x releases before 1.0, so their
   new flags, JSON, and state get used before they're frozen; 1.0 itself
   adds no features, only the promise. Closes once 0052 lands.
9. A stretch of daily use, across at least two releases, where upgrading a
   real library needs no manual fixes.

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
