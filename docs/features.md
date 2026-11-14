# Features

Feature backlog and queued work. When a feature gets picked up, it gets its
own numbered plan. What has shipped is in
[`CHANGELOG.md`](../CHANGELOG.md).

## Queue

Harder Better Faster Stronger, the order the maintainer picked on
2026-10-30, played backwards, finished in 1.2.0: Stronger shipped in 1.1.1,
Faster and Better in 1.1.2, and Harder in 1.2.0. Library views parts 3 and 4,
with Bulk tagging, shipped in 1.3.0 and 1.4.0 (plan 0062), and GUI
accessibility settings with Tray icon and hide on launch in 1.5.0 (plan
0063). The rest get a plan when they come up, and the rows below have the
details.

1. Publish to the repositories in parallel (plan 0065).
2. The new environment flow: kind and starting point first, Review folded
   into Details, and art chosen after Create (plan 0064).
3. Coverflow with controller navigation, library views part 5 (plan
   0066).

Store imports come after that.

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

Redefined by the maintainer on 2026-10-14 around 1.0, and anchored on
the first 1.x releases from 1.0 on:

- 1 = next: what the first 1.x releases should bring
- 2 = soon: polish people meet in daily use, small and high value
- 3 = later: wanted, but bigger, or nothing is asking for it yet
- 4 = someday: parked until a real need shows up

**Depends on** lists what has to be built first.

| Feature | Category | Priority | Depends on | Why | Notes |
|---|---|---:|---|---|---|
| GUI library views | UX | **1** | 0036 | Browse the library the way that fits the moment | Split by the maintainer into five plans (2026-10-17): 1) sorting, filters, favorites, and hidden games, plan 0054, released in 0.36.0; 2) grid zoom and the poster-only toggle, plan 0055, released in 0.37.0; 3) list view and 4) saved presets and grouping, plan 0062, released in 1.4.0 with exclude filters and the random pick in 1.3.0; 5) coverflow with controller navigation. 0054's Series section keeps the ideas for each. Parts 1 and 2 land before 1.0; parts 3 to 5 can land in 1.x (maintainer, 2026-10-17). |
| Edit hooks in GUI Settings | UX | **2** | 0046 | Set a game's hooks without editing TOML or using `wineo set` | Left out of plan 0046 by the maintainer (2026-10-06). The GUI already runs hooks. |
| Not so fuzzy search | UX | **2** | 0054 | Find a game by how people say it: "gta" for Grand Theft Auto IV, "civ 6" for Sid Meier's Civilization VI | From the maintainer during 0061 acceptance (2026-11-02). Today search is a plain substring of the title, environment name, or tags (`order.rs`). Proposed rules, each a yes or no, since search filters and the grid keeps its chosen sort: initials of a run of title words ("gta"), every query word matching the start of some title word in any order ("civ", "auto gr"), and numbers matching Roman numerals ("6" and "VI"). True fuzzy matching is out, since without ranking it floods the grid. Open: whether the CLI shares the matcher through core, how far numerals go, and a minimum length for initials. A tag like `gta` works as a manual alias today. |
| Embedded GOG login window | UX | **2** | 0049 | Log in to GOG without copying the success address back | From 0049 acceptance (2026-10-12). GOG's login only redirects to `embed.gog.com/on_login_success`, so today you copy that address from the browser and paste it into wineo. Heroic and Minigalaxy open the login page in their own browser window and catch the redirect there. iced can't show web pages, so this needs a WebKitGTK webview (for example `wry`) in a small helper window: a large GTK dependency for a one-time login, and more to bring to a macOS port. Watching the clipboard for the address was turned down for privacy. Needs a plan. |
| Completion status and notes | UX | **2** | | Track where you are with each game, like Playnite's Playing, Beaten, Completed, Abandoned, and Plan to play | A status and free-form notes per environment, shown in the panel and filterable once library views can filter. From the Playnite comparison (2026-10-14). If plan 0053 question 8 goes as recommended, it lives in `library.toml` beside favorites, so it can land in any 1.x. |
| "Started" hook and launch variables | Core | **2** | 0046 | Run something once the game is up, and write paths in targets that work anywhere | A third hook beside `before` and `after` that runs once the target has started (the place a companion like GOG's comet would go), and variables like `{install_dir}` or `{prefix}` in a target's `exe`, `args`, and `cwd`. From the Playnite comparison (2026-10-14). Variables change what an existing value means, so under plan 0053's version rule they'd be the first `schema_version = 2`: only files that use them get stamped, and v2 needs an escape for literal braces (like `{{` and `}}`) from the start. |
| Clean up leftovers no environment owns | Core | **2** | 0028, 0030 | Find and remove prefixes and GOG folder records left behind after an environment is gone | `validate` warnings and fixes for prefixes and GOG folder records that nothing owns anymore. Details in the [sketch](sketches/leftover-cleanup.md). |
| Crisper small icons at fractional scales | UX | **2** | wineo-gui | Keep the filter check, the exclude x, and other small marks smooth on a 1.25 or 1.5 scaled display | From 0062 acceptance (2026-11-04): on a 1.25 scale the Filters popup's check and x look pixelly. Each icon goes through `library_menu::icon`, which iced 0.14 rasterizes once per physical size and draws with nearest sampling and linear-light blending, so a 12 px mark becomes a 15 px bitmap with a stroke under 2 px; a screenshot looked blockier still, not yet explained. Reproduce headless in Xvfb with `WINIT_X11_SCALE_FACTOR=1.25` before changing anything. Candidates: shapes tuned to whole pixels, drawing the marks on a `canvas`, or a fix upstream. The thinner icons (the chevron, the tab's x) may need it too; check them in the same repro (maintainer). |
| Fake Steam with the 64-bit client layout | Testing | **2** | | Test a cold Steam launch end to end the way Steam runs it now | From the 1.5.2 fix (2026-11-13): Steam's update moved the running client and game reapers from `ubuntu12_32/` to `steamrt64/`, and wineo stopped seeing both, so every launch waited a minute and then reported a failure. Core's tests now cover both folders, but the CLI's fake Steam (`crates/wineo-cli/tests/common/steam.rs`, a small C client) still builds only `ubuntu12_32`. A variant that copies the real layout, where bare `steam -silent` enters through `ubuntu12_32/steam` and starts `steamrt64/steam`, which spawns `steamrt64/reaper`, plus one cold-start launch-and-stop test, would cover the path that broke. Priority set by the primary session; the maintainer may move it. |
| Test helpers that time out on the real clock | Testing | **2** | | Keep the GUI and CLI suites from flaking on a busy machine (CI, makepkg, parallel worktrees) | Found during plan 0066 (2026-11-13) under heavy CPU load. Two pre-existing spots: `sort_control::tests::answers()` in wineo-gui, shared by many GUI tests, pulls each task's messages with a 500 ms `tokio::time::timeout`, so a long stall drops a message; and `the_fake_sweep_finds_fakes_through_a_symlinked_root` (`crates/wineo-cli/tests/lifecycle.rs:426`) failed once under load and passed alone, likely the fake `sleep` being swept before its environment shows `WINEPREFIX` (unconfirmed). 0066's pad tests replaced the same timeout pattern with a clock-free poller that runs until nothing is ready and nothing woke (`each_ready()` in `crates/wineo-gui/src/app_pad/tests.rs` on `0066-1.7`), which `answers()` could share. Priority set by the primary session; the maintainer may move it. |
| Steam launch clock after a slow cold start | Core | **3** | 0047 | Keep a cold Steam launch from failing while Steam is still starting itself | From the 1.5.2 fix (2026-11-13). On a cold launch the request helper starts Steam with `-silent`, waits up to 60 s for the client, then sends `-applaunch` (`CLIENT_START_WAIT`, `crates/wineo-core/src/supervisor/steam.rs`). The watcher's 60 s start limit runs from the same moment, not from when the launch goes out, so a client that takes most of a minute to update and log in can time out before `-applaunch` is sent. wineo then says Steam didn't start the game, and the launch still arrives and runs untracked. Options: start the watcher's window when the helper reports the launch sent, or give the client wait its own limit before the launch window opens. Priority set by the primary session; the maintainer may move it. |
| ProtonDB lookup in the CLI | UX | **3** | | Check a game's compatibility information before launching or changing runners | Resolve a wineo environment to the right ProtonDB game, handle missing matches, and make the result reusable by the future GUI. The command shape and online behavior need a plan. |
| Store library import: Epic | UX | **3** | wineo-gui | Show whole store libraries in wineo, installed or not, and launch through the store's launcher, like Playnite | Steam is done in plan 0047 and plan 0048, GOG in plan 0049, plan 0050, and plan 0052; Epic remains. Needs a plan. The Steam and GOG design background is in the [sketch](sketches/store-library-import.md). |
| Steam shortcuts | UX | **3** | | Add wineo targets to Steam as non-Steam games, for Big Picture and Steam Deck | Steam's `shortcuts.vdf` is binary and Steam must be closed while it's edited. |
| Import from Lutris/Bottles and Heroic store metadata | Compatibility | **3** | 0008 | Extends generic import and Heroic sideloaded-game import | 0008 covers manually added Heroic games only. Store integration and other launchers remain separate work. |
| Managed copying of portable application folders | Portability/Data | **3** | 0007, 0008 | Optional organization for applications used in place today | 0008 copies prefixes but leaves external game folders where they are. Storage layout and removal semantics need a later plan. |
| Copy a portable environment back home | Portability/Data | **3** | 0019 | Bring an environment made or changed in a portable folder back into the host library | Found in the review of plan 0054 (2026-10-17): `portable` copies only into a portable folder, and refuses a portable source (`portable.rs:102`, `610`), so there's no way back. A return copy needs conflict rules for a name that exists at home, and would carry the environment's `library.toml` entry (favorite, hidden, added) home with it. Priority set by the primary session; the maintainer may move it. |
| Interruptible GOG requests | UX | **3** | 0049 | Make a cancel stop a GOG or SteamGridDB request already in flight | Left out of plan 0049 ("Where canceling stops"). A cancel is honored at checkpoints, so a request already sent waits for its answer: up to 30 s in the worst case, the GOG client's request timeout in `crates/wineo-gog/src/client.rs`. Canceling the blocking request itself, or running it on a thread the cancel can abandon, would make Cancel and Ctrl+C feel instant on a slow network. |
| Move a prefix | UX | **3** | | Relocate an environment's prefix in one command | From removing Heroic (2026-10-12): Disco Elysium's prefix, adopted in place by GOG migration, still lived in Heroic's folder, and moving it took a manual `mv` plus `wineo set prefix=`. A command could check the environment is idle, rename on one filesystem or copy across, update the config, and refuse when snapshots or other environments depend on the old path. |
| Emulated games | UX | **3** | 0026 | Add and launch games from emulators like Dolphin, RetroArch, and Cemu, with playtime, art, and desktop entries | A native environment can launch one by hand today; this adds a guided flow, a RetroArch core picker, and ROM folder imports. Needs a plan. Details in the [sketch](sketches/emulated-games.md). |
| Playtime tracking by folder or process name | Core | **3** | | Keep counting playtime when a launcher hands off to a game process it doesn't wait for | Playnite tracks the launched process, any process running from the install folder, or a process by name. wineo follows the process group, plus its handoff check for native launchers. Matters most for future store launchers like Epic's. From the Playnite comparison (2026-10-14). |
| Game metadata from the stores | UX | **3** | | Show a description, genres, developer, publisher, release date, and links in the panel | GOG's and Steam's APIs, which wineo already calls, have most of it; IGDB would cover the rest. From the Playnite comparison (2026-10-14). |
| Publish to the AUR | Distribution | **3** | 0059 | Install on Arch with an AUR helper instead of building from the repo | Left out of 0059: AUR registration has been closed to new accounts since 2026-10-30, with no reopening date. The maintainer checks once a week. The recipes are ready, `wineo` and `wineo-bin` in `packaging/aur` and `wineo-git` as the root `PKGBUILD`, and the publish script already covers them. |
| Publish to the repositories in parallel | Distribution | **3** | 0059 | Finish a publish in the time of its slowest destination, not all of them added up | From the maintainer (2026-11-01), once a few publishes have gone cleanly. Each destination already has its own subshell, result, and markers. PPA signing stays first for the passphrase, the shared Fedora tools image gets built before splitting, and each destination needs its own log. |
| Collections | UX | **3** | 0062 | Gather games into named groups beyond tags, built from a multi-select in the Library | Asked for by the maintainer at the 0062 step 8 checkpoint (2026-11-06). Needs its own plan first: what a collection holds that a tag doesn't (an order, its own art, a place in the sidebar), and how it shows in the CLI. |
| Menu shadows in the software renderer | UX | **4** | wineo-gui | Keep popup menus white when the GUI falls back to software rendering | Found in plan 0054 step 4's review (2026-10-18). iced_tiny_skia 0.14.1 draws a quad's shadow without the clip mask its fill uses, so a partial redraw (a hover or focus change) greys a menu everywhere but the changed row, darker with each redraw. The Sort, Filters, and right-click menus all show it; wgpu doesn't. Fixed upstream in iced 055355982 (2026-02-11), not in any release yet, so recheck when wineo moves to the iced release that carries it, rather than carrying a patched crate. |
| Epic Games Launcher setup | Compatibility | **4** | | Install the official Epic Games Launcher into its own environment in a few steps, and let Epic games live there | From the maintainer (2026-10-09). The launcher manages its own games inside that environment, and wineo doesn't list them one by one. Epic integration is Heroic's original purpose, so wineo leaves that space to Heroic. Needs a plan. |
| Edit Steam launch options | UX | **4** | 0047 | Set environment variables and wrappers like MangoHud for a Steam game from wineo | Left out of plan 0047 by the maintainer (2026-10-07): only arguments reach a Steam game through `steam -applaunch`. Env vars and wrappers live in Steam's launch options (`VAR=1 %command%` in `localconfig.vdf`), which can only be edited while Steam is closed, and wineo stays read-only toward Steam's files. |
| Race-free process group waits | Core | **4** | | Stop installer runs, hooks, and Proton run settling from calling a process group empty while a member is still running | The `/proc` group scan can miss a member that forks and exits mid-scan; a real fix likely needs cgroups or pidfds. Parked by the maintainer. Details in the [sketch](sketches/race-free-group-waits.md). |
| macOS port | Compatibility | **4** | | Run wineo and wineo-gui on Apple Silicon Macs with Wine, CrossOver-style Wine builds, and Game Porting Toolkit | Parked by the maintainer (2026-10-17) until there is a route to an ARM64 Wine that is free to use and runs signed. Spike findings, engines, signing, and CrossOver notes are in the [sketch](sketches/macos-port.md). |
| Copy GOG game folders with snapshots, clones, and portable copies | Portability/Data | **4** | 0049 | Take a GOG game's files along when its environment is snapshotted, cloned, or copied to a portable folder | Left out of plan 0049 (question 13). Today those copy the environment and prefix, and the GOG game folder stays where it is, shared by the copies and protected by its owner's folder state. Copying it needs a second folder state, its own owner, and rules for updates of the copy. Needs a plan. |
| GOG games in portable roots | Portability/Data | **4** | 0049 | Migrate and update GOG games inside a `WINEO_PORTABLE_DIR` folder | Left out of plan 0049 (question 17): GOG game folders and their state live in the host root, and `--portable` refuses migrating and updating. Logging in and listing already work there. Needs folder states that travel with the portable folder. Needs a plan. |
| Use Heroic's native file list | Compatibility | **4** | 0049 | Know which files a migrated native GOG game shipped before its first update | Left out of plan 0049. heroic-gogdl keeps `.gogdl-linux-manifest` (each file's name, size, CRC32, and whether it's a symlink) in the game folder. Importing it, checked against the folder, would give a migrated game winning files at once, so its first update could tell your changes from GOG's. Until then the first rebuild overwrites and lists files that differ. |
| Achievements | UX | **4** | 0052, emulated games | Earn and see achievements for GOG and emulated games outside Steam | Parked by the maintainer (2026-10-14): GOG through comet and RetroAchievements. Details in the [sketch](sketches/achievements.md). |
| Controller fullscreen mode | UX | **4** | GUI library views | A whole controller-first mode for the couch, like Playnite's Fullscreen or Steam's Big Picture | Beyond the library views row's coverflow picker: every page reachable from a gamepad. From the Playnite comparison (2026-10-14). |

## 1.0

Wineo 1.0.0 shipped on 2026-10-29, once the nine criteria the maintainer
approved on 2026-10-06 were met: a written public contract
([Stability](stability.md)), exit codes and `--json` schemas documented for
every command, configs from earlier releases tested to keep loading, a
deprecation path for renames, reliable lifecycle tests, an audit for
data-loss bugs, the last big shape change settled, and a stretch of daily
use with no manual fixes on upgrade. The details are in git history.
