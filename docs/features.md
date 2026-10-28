# Features

Feature backlog and queued work. When a feature gets picked up, it gets its
own numbered plan in `docs/plans/`. What has shipped is in
[`CHANGELOG.md`](../CHANGELOG.md).

## Queue

The order the maintainer picked on 2026-10-14, after 0051. Each item gets
its own plan when it comes up, and the rows below have the details.

1. GUI themes, so they cover every palette role.
2. 1.0, once the [1.0 criteria](#10-criteria) are met.
3. Release packages (deb, rpm, AppImage), shortly after 1.0.

Library views parts 3 to 5 (list view, saved presets and grouping,
coverflow) aren't in the order; they can land in 1.x (maintainer,
2026-10-17).

## Backlog

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

**Depends on** lists what has to be built first.

| Feature | Category | Priority | Depends on | Why | Notes |
|---|---|---:|---|---|---|
| GUI themes and customization | UX | **1** | 0036 | Let people make the app their own | From the maintainer's 0036 testing (2026-09-28): custom themes with a few presets, added with a GUI settings page. Longer term, a deeply customizable GUI. The palette tokens from 0036 are the starting point. Moved up to 1 by the maintainer (2026-10-14), to land before 1.0. |
| GUI library views | UX | **1** | 0036 | Browse the library the way that fits the moment | Split by the maintainer into five plans (2026-10-17): 1) sorting, filters, favorites, and hidden games, [0054](plans/0054-library-sorting-and-favorites.md), released in 0.36.0; 2) grid zoom and the poster-only toggle, [0055](plans/0055-grid-zoom-and-poster-only.md), released in 0.37.0; 3) list view; 4) saved presets and grouping; 5) coverflow with controller navigation. 0054's Series section keeps the ideas for each. Parts 1 and 2 land before 1.0; parts 3 to 5 can land in 1.x (maintainer, 2026-10-17). |
| GitHub mirror and CI | Distribution | **1** | | Publish wineo at 1.0 while the maintainer keeps working in Gitea | Gitea stays primary and push-mirrors to GitHub at 1.0, with Actions on outside PRs and tags. Details in the [sketch](sketches/github-mirror.md). |
| Edit hooks in GUI Settings | UX | **2** | 0046 | Set a game's hooks without editing TOML or using `wineo set` | Left out of [0046](plans/0046-launch-hooks.md) by the maintainer (2026-10-06). The GUI already runs hooks. |
| Bulk tagging | UX | **2** | 0033 | Add or remove a tag on many environments at once instead of one `wineo set` per environment | Suggested by the maintainer during 0045 acceptance (2026-10-07), mostly for the GUI. Today `tags=[...]` replaces the whole list, and GUI tags are edited one environment at a time in Settings. Likely shape: `wineo tag add TAG NAME...` and `wineo tag remove TAG NAME...`, plus multi-select in the Library with an add/remove tag action. The GUI part needs a design pass on how multi-select works with the grid and keyboard. |
| Exclude filters | UX | **2** | 0054 | Show everything except a tag, store, or platform, like "no co-op" | From the maintainer during 0054 acceptance (2026-10-19): right-clicking a Filters popup row would exclude it instead of including it. Needs a design pass: an excluded row's and chip's look ("not co-op"), a keyboard way to exclude since right-click alone isn't reachable from the keyboard (or a click cycling off, include, exclude), how excludes combine with includes in the same group, and whether the Filters (N) count includes them. Fits beside 0054's part 4 (saved presets and grouping), which reworks the same popup. |
| Release packages (deb, rpm, AppImage) | Distribution | **2** | | Install wineo on Debian, Ubuntu, Fedora, and other distros without building it, shortly after 1.0 | deb and rpm first, then an AppImage; Flatpak is out. Needs a plan. Details in the [sketch](sketches/release-packages.md). |
| GUI accessibility settings | UX | **2** | 0036 | Make the desktop app comfortable for people who need larger text, other fonts, or less motion | From the maintainer (2026-10-09), after finding that semibold text falls back to a monospace font on their machine. The patch fix after 0048 bundles a default sans font; this row lets people pick their own font and text size on top of it, and fits with reduced motion and high-contrast palettes. Text measurement for balanced titles already uses the rendered font, so a font change keeps wrapping correct, but fixed tile and card heights would need to follow the text size. Pairs with GUI themes (queue item 2). Needs a plan. |
| Embedded GOG login window | UX | **2** | 0049 | Log in to GOG without copying the success address back | From 0049 acceptance (2026-10-12). GOG's login only redirects to `embed.gog.com/on_login_success`, so today you copy that address from the browser and paste it into wineo. Heroic and Minigalaxy open the login page in their own browser window and catch the redirect there. iced can't show web pages, so this needs a WebKitGTK webview (for example `wry`) in a small helper window: a large GTK dependency for a one-time login, and more to bring to a macOS port. Watching the clipboard for the address was turned down for privacy. Needs a plan. |
| One shape for confirm prompts | UX | **2** | | Ask every review's final yes or no the same way | From 0049 acceptance (2026-10-12). The GOG migrate and update confirms now print one keys line, then `Migrate it? [Y/n]:`. Others still differ: the Steam import's game step prints `Press Enter to import it, Esc to go back, Ctrl+C to cancel.` before `Import it? [Y/n]:`, and an empty `review` (`wineo new`) asks `Create it?` with no `[Y/n]`. Survey every confirm and settle on one. |
| Completion status and notes | UX | **2** | | Track where you are with each game, like Playnite's Playing, Beaten, Completed, Abandoned, and Plan to play | A status and free-form notes per environment, shown in the panel and filterable once library views can filter. From the Playnite comparison (2026-10-14). If [0053](plans/0053-config-contract.md) question 8 goes as recommended, it lives in `library.toml` beside favorites, so it can land in any 1.x. |
| "Started" hook and launch variables | Core | **2** | 0046 | Run something once the game is up, and write paths in targets that work anywhere | A third hook beside `before` and `after` that runs once the target has started (the place a companion like GOG's comet would go), and variables like `{install_dir}` or `{prefix}` in a target's `exe`, `args`, and `cwd`. From the Playnite comparison (2026-10-14). Variables change what an existing value means, so under [0053](plans/0053-config-contract.md)'s version rule they'd be the first `schema_version = 2`: only files that use them get stamped, and v2 needs an escape for literal braces (like `{{` and `}}`) from the start. |
| Tray icon and hide on launch | UX | **2** | 0036 | Get the app out of the way while a game runs, and reach common actions without opening it | A StatusNotifierItem tray icon (over D-Bus; the app already has zbus) with a menu: open the library, add something, and stop what's running. A setting hides the window to the tray when a game starts and brings it back when the game exits. From the maintainer (2026-10-14). From the Playnite comparison (2026-10-14). Hyprland has no minimize, so hiding needs the tray; test with a bar that shows tray icons. |
| Clean up leftovers no environment owns | Core | **2** | 0028, 0030 | Find and remove prefixes and GOG folder records left behind after an environment is gone | `validate` warnings and fixes for prefixes and GOG folder records that nothing owns anymore. Details in the [sketch](sketches/leftover-cleanup.md). |
| New Steam installs in the library | UX | **2** | 0047 | Notice when Steam finishes installing a game that isn't in wineo yet, instead of waiting for a manual import | From the maintainer (2026-10-28). Today a new install only shows up after **Add > Import from Steam** or `wineo import steam --all`; games imported before they were installed already follow Steam live. By default the library shows a nudge, like "2 new Steam games", with an Import button that opens the usual Steam import review, so wineo still never creates an environment on its own. An opt-in setting imports new installs automatically, with `import steam --all`'s defaults. The library's refresh already reads Steam's manifests, so noticing is cheap; it needs a record of games already offered, so a dismissed one doesn't come back, and should skip soundtracks, demos, and tools the way `--list` skips Steam's own tools. Needs a plan. |
| ProtonDB lookup in the CLI | UX | **3** | | Check a game's compatibility information before launching or changing runners | Resolve a wineo environment to the right ProtonDB game, handle missing matches, and make the result reusable by the future GUI. The command shape and online behavior need a plan. |
| Store library import: Epic | UX | **3** | wineo-gui | Show whole store libraries in wineo, installed or not, and launch through the store's launcher, like Playnite | Steam is done in [0047](plans/0047-steam-library.md) and [0048](plans/0048-steam-owned-games.md), GOG in [0049](plans/0049-gog-games.md), [0050](plans/0050-gog-installs.md), and [0052](plans/0052-galaxy-builds.md); Epic remains. Needs a plan. The Steam and GOG design background is in the [sketch](sketches/store-library-import.md). |
| Steam shortcuts | UX | **3** | | Add wineo targets to Steam as non-Steam games, for Big Picture and Steam Deck | Steam's `shortcuts.vdf` is binary and Steam must be closed while it's edited. |
| Import from Lutris/Bottles and Heroic store metadata | Compatibility | **3** | 0008 | Extends generic import and Heroic sideloaded-game import | 0008 covers manually added Heroic games only. Store integration and other launchers remain separate work. |
| Managed copying of portable application folders | Portability/Data | **3** | 0007, 0008 | Optional organization for applications used in place today | 0008 copies prefixes but leaves external game folders where they are. Storage layout and removal semantics need a later plan. |
| Copy a portable environment back home | Portability/Data | **3** | 0019 | Bring an environment made or changed in a portable folder back into the host library | Found in the review of [0054](plans/0054-library-sorting-and-favorites.md) (2026-10-17): `portable` copies only into a portable folder, and refuses a portable source (`portable.rs:102`, `610`), so there's no way back. A return copy needs conflict rules for a name that exists at home, and would carry the environment's `library.toml` entry (favorite, hidden, added) home with it. Priority set by the primary session; the maintainer may move it. |
| Bundled fonts in GUI layout tests | Testing | **3** | | Make headless layout tests measure text the same on every machine | From the 0048 patch batch (2026-10-10): the GUI bundles Noto Sans, but `layout_probe` and the tile title tests still shape text with whatever system fonts are installed, so `PKGBUILD` checks on other distros can measure differently. Loading the bundled fonts into the shared font system for tests shifts every measured width once, so expected layouts need a review pass. The maintainer deferred it out of the patch batch. |
| Supervisor force-stop deadline | Core | **3** | | Keep a hung `wineserver -k` from stalling a run's give-up deadline | From 0049 step 4's review (2026-10-11). `supervisor.rs` calls `kill_live` synchronously, so if `wineserver -k` hangs, the 10s give-up bound for `start`, the installer, and GOG execs is never checked. Older than step 4 and only reachable with a broken runner. Running the force step on its own thread with a timeout would fix it. |
| Exact Steam platforms | UX | **3** | 0054 | Put every installed Steam game under Native or Windows, even before its first run | Option B of [0054](plans/0054-library-sorting-and-favorites.md)'s acceptance question A5 (2026-10-18). Today a Steam game is Windows only once Steam shows it runs through Proton (its own `CompatToolMapping` entry or a `compatdata` prefix), so a native game like Stardew Valley and a Windows game never launched have no platform. Reading each installed app's `oslist` from the binary `appcache/appinfo.vdf` (cached by its stamp) would place them: no Linux build, or its own Proton entry, is Windows; otherwise Native. Steam has changed that format before, so it falls back to today's rule when it can't be read. |
| Exit with background work stuck | Core | **3** | 0054 | Let the desktop app quit when a background job hangs, like a write to a stalled network drive | Found in the review of [0054](plans/0054-library-sorting-and-favorites.md) step 3 (2026-10-18). The window closes, but the process stays until every blocking task the Tokio runtime started returns, since dropping the runtime waits for them. 0054 bounds its own sort saves at 6 s; the process exit isn't bounded. A fix could end the process once the window is gone, after flushing what must be saved. |
| Interruptible GOG requests | UX | **3** | 0049 | Make a cancel stop a GOG or SteamGridDB request already in flight | Left out of [0049](plans/0049-gog-games.md) ("Where canceling stops"). A cancel is honored at checkpoints, so a request already sent waits for its answer: up to 30 s in the worst case, the GOG client's request timeout in `crates/wineo-gog/src/client.rs`. Canceling the blocking request itself, or running it on a thread the cancel can abandon, would make Cancel and Ctrl+C feel instant on a slow network. |
| Move a prefix | UX | **3** | | Relocate an environment's prefix in one command | From removing Heroic (2026-10-12): Disco Elysium's prefix, adopted in place by GOG migration, still lived in Heroic's folder, and moving it took a manual `mv` plus `wineo set prefix=`. A command could check the environment is idle, rename on one filesystem or copy across, update the config, and refuse when snapshots or other environments depend on the old path. |
| Emulated games | UX | **3** | 0026 | Add and launch games from emulators like Dolphin, RetroArch, and Cemu, with playtime, art, and desktop entries | A native environment can launch one by hand today; this adds a guided flow, a RetroArch core picker, and ROM folder imports. Needs a plan. Details in the [sketch](sketches/emulated-games.md). |
| Playtime tracking by folder or process name | Core | **3** | | Keep counting playtime when a launcher hands off to a game process it doesn't wait for | Playnite tracks the launched process, any process running from the install folder, or a process by name. wineo follows the process group, plus its handoff check for native launchers. Matters most for future store launchers like Epic's. From the Playnite comparison (2026-10-14). |
| Game metadata from the stores | UX | **3** | | Show a description, genres, developer, publisher, release date, and links in the panel | GOG's and Steam's APIs, which wineo already calls, have most of it; IGDB would cover the rest. From the Playnite comparison (2026-10-14). |
| Simplification finds | Perf | **3** | | Do less repeated work and share duplicated code, without changing what wineo does | Eight finds from a code review, looked over by the maintainer (2026-10-27): reading GOG folder states once per listing, one fewer TOML parse per config load, the batch trash scan in `environment::tags`, reusing a loaded config in stats, a few needless allocations, and three spots of duplicated editing, parsing, and GUI settings code. Each lands with tests showing the same output. Details in the [sketch](sketches/simplification-finds.md). |
| `--dry-run --json` | Automation | **4** | 0044 | Let a script read a dry run's plan instead of grepping review lines | Left out of [0044](plans/0044-dry-run.md) by the maintainer (2026-10-06). Add it if a script ever needs it. |
| Ctrl+Enter on focused controls in flows | UX | **4** | 0056 | Stop Ctrl+Enter from pressing a focused control and also submitting the flow | Found in the review of [0056](plans/0056-gui-polish.md) step 7 (2026-10-22), parked by the maintainer. List rows ignore Enter while Ctrl is held, but other focusable controls in flows, like the runner offer card, take it, so Ctrl+Enter on one presses it and submits the dialog. Widening the rule to every `focus_button` control is small, but changes what Ctrl+Enter does on those controls. |
| Menu shadows in the software renderer | UX | **4** | wineo-gui | Keep popup menus white when the GUI falls back to software rendering | Found in [0054](plans/0054-library-sorting-and-favorites.md) step 4's review (2026-10-18). iced_tiny_skia 0.14.1 draws a quad's shadow without the clip mask its fill uses, so a partial redraw (a hover or focus change) greys a menu everywhere but the changed row, darker with each redraw. The Sort, Filters, and right-click menus all show it; wgpu doesn't. Fixed upstream in iced 055355982 (2026-02-11), not in any release yet, so recheck when wineo moves to the iced release that carries it, rather than carrying a patched crate. |
| Fix notes in `validate --json --fix` | Automation | **4** | 0054 | Let a script see what a fix reported, like how much a `library.toml` repair recovered | Left out of [0054](plans/0054-library-sorting-and-favorites.md) by the maintainer (2026-10-18): `--json` output has no field for a fix's notes, so a scripted repair only learns that it ran. Adding one is a compatible change; design it with the rest of the JSON contract. |
| Global hooks | Core | **4** | 0046 | Run the same hook around every game, like turning notifications off while anything plays | Left out of [0046](plans/0046-launch-hooks.md) by the maintainer (2026-10-06): they'd surprise people using wineo for regular software, and templates carry hooks for new games. |
| Epic Games Launcher setup | Compatibility | **4** | | Install the official Epic Games Launcher into its own environment in a few steps, and let Epic games live there | From the maintainer (2026-10-09). The launcher manages its own games inside that environment, and wineo doesn't list them one by one. Epic integration is Heroic's original purpose, so wineo leaves that space to Heroic. Needs a plan. |
| Edit Steam launch options | UX | **4** | 0047 | Set environment variables and wrappers like MangoHud for a Steam game from wineo | Left out of [0047](plans/0047-steam-library.md) by the maintainer (2026-10-07): only arguments reach a Steam game through `steam -applaunch`. Env vars and wrappers live in Steam's launch options (`VAR=1 %command%` in `localconfig.vdf`), which can only be edited while Steam is closed, and wineo stays read-only toward Steam's files. |
| Cheaper Steam refresh in the desktop app | Perf | **4** | 0047 | Keep the library's 2-second refresh light for people with many Steam games | Found in 0047's review (2026-10-09). Each refresh already reads Steam's manifests and `config.vdf` once, but still parses Steam's playtime files and scans `/proc` for a reaper once per Steam card. Share one playtime read and one `/proc` scan per refresh, or refresh Steam data less often than run state. [0051](plans/0051-lean-and-simple.md) took most of the cost away: no rescans while the app is unfocused, and one cheaper `/proc` sweep per read. |
| Integrity checking | Portability/Data | **4** | | Detect damaged or tampered files | Scope still open: prefix or game files, snapshots, or re-verifying downloads (already sha256-checked on install). |
| Race-free process group waits | Core | **4** | | Stop installer runs, hooks, and Proton run settling from calling a process group empty while a member is still running | The `/proc` group scan can miss a member that forks and exits mid-scan; a real fix likely needs cgroups or pidfds. Parked by the maintainer. Details in the [sketch](sketches/race-free-group-waits.md). |
| Winetricks settle under Proton | Core | **3** | | Wait for whatever a winetricks verb leaves running in a Proton prefix before letting go of the environment | Found while fixing Proton run settling (2026-10-17). Winetricks' own `settle` (`winetricks.rs`) waits on wineserver, which Proton runs don't have, so it returns at once and anything a verb started in umu's group can still be running when the winetricks lock is released. Runs no longer have this gap: they wait for umu's process group to empty. The same group wait would fit here. Priority set by the primary session; the maintainer may move it. |
| Full backup and restore | Portability/Data | **4** | 0018 | Great eventually, but this could become a whole subsystem | Covers filesystems without snapshot support. |
| macOS port | Compatibility | **4** | | Run wineo and wineo-gui on Apple Silicon Macs with Wine, CrossOver-style Wine builds, and Game Porting Toolkit | Parked by the maintainer (2026-10-17) until there is a route to an ARM64 Wine that is free to use and runs signed. Spike findings, engines, signing, and CrossOver notes are in the [sketch](sketches/macos-port.md). |
| Copy GOG game folders with snapshots, clones, and portable copies | Portability/Data | **4** | 0049 | Take a GOG game's files along when its environment is snapshotted, cloned, or copied to a portable folder | Left out of [0049](plans/0049-gog-games.md) (question 13). Today those copy the environment and prefix, and the GOG game folder stays where it is, shared by the copies and protected by its owner's folder state. Copying it needs a second folder state, its own owner, and rules for updates of the copy. Needs a plan. |
| GOG games in portable roots | Portability/Data | **4** | 0049 | Migrate and update GOG games inside a `WINEO_PORTABLE_DIR` folder | Left out of [0049](plans/0049-gog-games.md) (question 17): GOG game folders and their state live in the host root, and `--portable` refuses migrating and updating. Logging in and listing already work there. Needs folder states that travel with the portable folder. Needs a plan. |
| Use Heroic's native file list | Compatibility | **4** | 0049 | Know which files a migrated native GOG game shipped before its first update | Left out of [0049](plans/0049-gog-games.md). heroic-gogdl keeps `.gogdl-linux-manifest` (each file's name, size, CRC32, and whether it's a symlink) in the game folder. Importing it, checked against the folder, would give a migrated game winning files at once, so its first update could tell your changes from GOG's. Until then the first rebuild overwrites and lists files that differ. |
| Write through symlinked configs | Portability/Data | **4** | | Keep a config that's a symlink (a dotfiles repo, say) a symlink when wineo edits it | From the data-loss audit (2026-10-12), parked by the maintainer. `wineo set` and attaching a GOG game both replace the file with an atomic rename, which turns a symlinked `environments/<name>.toml` or `config.toml` into a regular file, so the link's target silently stops being the live config. Writing through the link (temp file beside the target, rename onto the target) would keep it. |
| Close the last nested-prefix gaps | Core | **4** | | Catch nested prefixes that slip past the creation checks | From the data-loss fixes (2026-10-12), where creating, cloning, importing, restoring, or moving an environment so its prefix nests inside another's became refused. Two gaps remain: two `new`s at the same moment can still nest into each other, since locks cover exact paths and not their ancestors, and `wineo edit` skips the prefix checks entirely. A `validate` check for nested prefixes would catch both, plus any nesting a library already has. |
| Achievements | UX | **4** | 0052, emulated games | Earn and see achievements for GOG and emulated games outside Steam | Parked by the maintainer (2026-10-14): GOG through comet and RetroAchievements. Details in the [sketch](sketches/achievements.md). |
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
   Three gaps were in the code, not the docs. The maintainer settled them
   (2026-10-12): `update --check --json` and `import gog --list --json`
   gained `unfinished`, this week's and this month's playtime became
   text-only for every environment, and `validate --fix` leaving an
   interrupted snapshot restore for `wineo snapshot recover` stays as
   documented. Done (2026-10-12): the two code changes shipped in 0.30.1.
4. Make `schema_version` a guarantee: a config written by any 1.x release
   loads in every later 1.x, and an older one either loads or migrates
   cleanly. Add upgrade tests that load fixture configs and state written
   by earlier releases. Done (2026-10-16): [0053](plans/0053-config-contract.md)
   in 0.35.0, with fixture sets written by seven real releases and upgrade
   tests over every set.
5. Add a deprecation path: a renamed or removed config key or flag keeps
   working in 1.x with a warning, and `wineo validate` reports it as
   deprecated. 2.0 is where those become errors. Done (2026-10-16):
   [0053](plans/0053-config-contract.md) in 0.35.0, with `deprecated_key` in
   `validate`, warning aliases for renamed flags and commands, and four
   renames made before the freeze.
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
   adds no features, only the promise. Closed: 0052 landed in 0.34.0
   (2026-10-16).
9. A stretch of daily use, across at least two releases, where upgrading a
   real library needs no manual fixes.
