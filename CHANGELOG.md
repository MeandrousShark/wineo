# Changelog

A finished plan bumps the minor version and fixes between plans bump the
patch version. See `AGENTS.md` for the rule.

## 0.30.1 - 2026-10-12

- The library loads faster with Steam games in it: wineo checks running
  processes once for all of them instead of once per game, which took
  `wineo list` from about 120 ms to 20 ms on a library of 22.
- Data-loss fixes from an audit of trash, snapshots, clone, imports, and
  portable copies:
  - Restoring a snapshot, deleting one, and pruning trash never reach into
    a mount: a prefix with something mounted inside it, or a folder that is
    itself a mount point, is refused instead of having the mounted files
    deleted.
  - Recovering an interrupted removal whose files sat on another
    filesystem no longer throws away the complete copy in the trash when
    the original was half deleted. If the original was changed or replaced
    instead, it stays as it is and the earlier copy becomes a normal trash
    entry you can restore or prune.
  - Removing an environment whose files sit on another filesystem checks
    the copy in the trash file by file before deleting the original, not
    only by a digest that two different folders could share.
  - Copies, imports, clones, and runner copies are flushed to disk before
    they're registered, and first-time configs are written atomically.
  - A failed copy no longer leaves its staging folder behind when the
    source has read-only folders.
- Behavior change: a prefix inside another environment's prefix, or one
  that contains another's, is refused by `new`, clone, import (including
  `--adopt`), `set prefix=`, portable copy, snapshot restore, trash restore,
  and `rm --delete-prefix`. Sharing the exact same prefix still works.
- Behavior change: clone and import refuse a `--dir` another environment
  already names as its prefix, even when that folder is missing (an
  unmounted drive, say), and portable copy refuses to replace an existing
  play log in the target folder.
- Ctrl+C or `wineo stop` during a Steam game's start no longer fails with
  "Interrupted system call" or records the run as `failed`, and a failed
  lock acquire releases what it took right away.
- JSON: `update --check --json` and `import gog --list --json` have
  `unfinished`, true when an interrupted update has to be finished.
  Behavior change: `stats --json` no longer has `this_week_seconds` and
  `this_month_seconds` for Steam games; those totals are text-only for every
  environment. [JSON output](docs/json-output.md) now documents every field
  and which changes count as compatible.
- `snapshot recover` names the config it can't read when that blocks it.

## 0.30.0 - 2026-10-12

- GOG account, migration, and updates
  ([0049](docs/plans/0049-gog-games.md)), without Heroic. `wineo import gog
  --login` logs in through GOG's login page in your browser and keeps the
  login in the keyring; `--list` shows the games you own, `--game ID` and
  `--all` move games Heroic installed into wineo in place, and `--logout`
  removes the login. See [GOG games](docs/gog.md).
- New `wineo update`: `--check` compares GOG games and their DLC with GOG's
  current versions (`--json` too), `wineo update NAME` updates a native
  game from GOG's packages, `--reinstall` rebuilds it to repair changed or
  missing files, and `--all` updates every game with an update. An update
  writes down what it's doing first, so an interrupted one never leaves a
  half-updated game that runs: `wineo start` refuses until `wineo update
  NAME` finishes it, from the packages already downloaded.
- Migrated games keep their Heroic prefix and settings, and pick up GOG
  playtime and art wineo doesn't have yet.
- Behavior change: an environment migrated from GOG gets a `gog_id` key,
  which only `import gog` sets. `rm`, trash, restore, snapshot restore,
  clone, and `validate` keep the game folder's record consistent, and
  `wineo start` refuses an environment whose game folder is mid-update.
- The desktop app has a **GOG** section in Settings to log in and out,
  **Import from GOG** for migrating, an **Update** status row on GOG games,
  and **Finish update** for an interrupted one.
- DLC are shown as `NAME (ID)`, like every other title and ID, and the
  logged-in account shows its GOG username.

## 0.29.1 - 2026-10-10

- In the desktop app, the art page and the art picker work in narrow and
  short windows. Change, Find, Browse, Remove, Filters, and Cancel stay
  whole instead of shrinking to nothing; their rows move underneath or wrap
  when there isn't room.
- The art picker's title and filters scroll with the images, and only the
  progress line and **Use** stay pinned at the bottom, so images still show
  in a short window. Arrow keys scroll to the selected image's real row, even
  when badges make rows taller.
- A new [Stability](docs/stability.md) page lists what wineo will promise
  not to break from 1.0 on, and what can change in any release.

## 0.29.0 - 2026-10-10

- Owned Steam games ([0048](docs/plans/0048-steam-owned-games.md)). With a
  Steam Web API key (`wineo import steam --key`, kept in the keyring like the
  SteamGridDB key), `import steam --list` also shows games you own but
  haven't installed, and they import like installed ones. `--all --owned`
  imports every one. The owned list is cached for an hour; `--refresh`
  fetches it again. See [Owned games](docs/steam.md#owned-games).
- `wineo install` and `wineo uninstall` hand a Steam game to Steam's own
  dialog. `-f` stays and shows progress until Steam is done. The game is
  playable once Steam finishes, with no re-import.
- Behavior change: `install` in `status --json` and `list --json` has new
  values (`installing`, `install_queued`, `uninstalling`) and a new
  `install_progress` percent. `state` stays the run status, so a game can be
  `inactive` and `installing` at once.
- Behavior change: `wineo validate` no longer reports a Steam game that isn't
  installed, since owned games are allowed to be. The
  `steam_game_not_installed` code is gone.
- The desktop app lists owned games in **Import from Steam**, shows install
  progress on cards, and has **Install**, **Uninstall**, and **Remove key**.
- Desktop app polish: every button uses the shared styles, every scrollbar is
  the thin rail, choice lists are quiet grouped rows, import tiles have
  balanced two-line titles and a keyboard-only focus ring, and art loads
  lazily by real row height.
- Short windows: import pages keep Back at the top and pin each step's
  action row at the bottom while the content scrolls, so a game grid shows
  even in a 240 px tall dialog.
- The desktop app bundles Noto Sans, so titles no longer fall back to a
  monospace font on systems without a semibold Noto Sans.
- Concurrent saves of the same file, including the SteamGridDB and Steam Web
  API key files, can no longer clobber each other. Removing a key also
  removes any temporary copy a crash left behind.

## 0.28.0 - 2026-10-09

- Steam games ([0047](docs/plans/0047-steam-library.md)). `wineo import
  steam` brings installed Steam games into the library as a new proxied
  environment kind: tagged, counted in `wineo stats` with Steam's own
  playtime, started and stopped from the CLI or the desktop app, with hooks
  around wineo's launches. Steam keeps owning the game and its prefix, and
  wineo never writes to Steam's files. Games started from the Steam client
  show as running, and Stop works on them. See [Steam games](docs/steam.md).
- Steam games take arguments, tags, a title, hooks, and a desktop entry.
  Snapshots, winetricks, `exec`, clone, portable copies, env vars, and
  runner settings are refused with one short line.
- The global `steam` setting (`wineo set --global steam=flatpak`) picks
  Flatpak Steam over a native install.
- Import copies Steam's cached art, and `art fetch` and `--art` look Steam
  games up on SteamGridDB by app id instead of by title.
- `status --json` and `list --json` have `kind` for every environment, and
  Steam games add `launcher`, `app_id`, `install`, and `started_by`. A Steam
  game's `played.sessions` is `null`, and `played.seconds` is `null` when
  Steam's playtime is unknown.
- `wineo validate` checks Steam games: Steam missing, the game not
  installed, or a library that can't be read.
- The desktop app gets **Add**, **Import from Steam**, a Steam mark on
  cards, grayscaled cards for games that aren't installed, and
  **Waiting for Steam...** while a game starts.

## 0.27.1 - 2026-10-07

- In the desktop app, Stop turns into Force stop when a game is still
  running 3 seconds after you pressed it. Force stop kills it right away,
  like `wineo stop --force`. The wait keeps a double-click on Stop from
  killing a game that was about to close.

## 0.27.0 - 2026-10-07

- Launch hooks ([0046](docs/plans/0046-launch-hooks.md)). `[hooks]` with
  `before` and `after` shell commands goes on an environment, a target, or
  a template, and runs around `start`, `run`, `restart`, desktop entries,
  and Play in the desktop app. `before` runs ahead of the exe check, so it
  can mount the drive a game lives on, and a failing `before` cancels the
  launch. `after` runs however the run ended, with `WINEO_END`,
  `WINEO_EXIT_CODE`, and `WINEO_PREFIX` for Wine games. See
  [Hooks](docs/hooks.md).
- `wineo stop` waits for a running `after` hook, and `--force` kills it.
  `stop --force` on a running game still runs `after` and says how to kill
  it. In `wineo run`, Ctrl+C during `after` offers to kill it, right away
  if Ctrl+C already stopped the game.
- `wineo status` shows an environment's hooks and when it's running one,
  and `status --json` and `list --json` have `hooks` with `before` and
  `after` booleans. The desktop app shows a hook's state, shows a failed
  `before` hook's output with the error, and asks before killing an
  `after` hook.
- Heroic import carries a game's before and after launch scripts over as
  hooks, and template summaries in `wineo new` and the Add flow list them.

## 0.26.0 - 2026-10-07

- Playtime by tag ([0045](docs/plans/0045-stats-by-tag.md)).
  `wineo stats --tag game` limits the overview, its total, and the "never
  played" count to environments with that tag; give `--tag` more than once
  to require every tag. `--by-tag` shows one row per tag, most played
  first, with `(untagged)` last. An environment with several tags counts
  under each, and the output says so. Both work with `--json`. See
  [By tag](docs/stats.md#by-tag).
- Playtime under a minute shows as `< 1 m` instead of `0 m`, in `wineo
  stats` and the desktop app. `--json` still has exact seconds.

## 0.25.0 - 2026-10-07

- `--dry-run` for `wineo new` and `wineo import`
  ([0044](docs/plans/0044-dry-run.md)). It prints the same review a real
  run would, then stops without writing, downloading, starting Wine, or
  asking anything, so it works without a terminal. Errors match the real
  run's. A Windows path `--exe` shows `(checked once the prefix exists)`.
  See [Dry runs](docs/dry-run.md).
- `import heroic --all --dry-run` previews what the same flags would do.
  Without `--yes` it assumes every download offer is accepted, and with
  `--yes` it mirrors the real run, including `skip:` lines.
- Reviews in `import prefix`, `clone`, `portable copy`, and `snapshot
  create` count links that point outside the prefix instead of listing
  them: `external links: 26, kept in place (-v to list them)`. `-v` lists
  each one. Clone used to list the first 12.
- Behavior change: exit codes are consistent across commands. See
  [Exit codes](docs/exit-codes.md).
  - 130 for anything canceled, including answering no at a final
    confirmation, canceling a Heroic import, and Esc at the first question
    of a guided flow, which now cancels. The key hint says `Esc or Ctrl+C
    to cancel.`
  - 2 for usage errors, including `prune` without a selector.
  - 141, quietly, when whatever reads wineo's output goes away, like
    `wineo list | head -1`.
- Heroic import checks DXVK and vkd3d-proton builds again under their lock
  before registering them.

## 0.24.0 - 2026-10-06

- Runners from Steam, Heroic, and Lutris
  ([0043](docs/plans/0043-external-runners.md)). With
  `external_runners = true` in global config, environments can use Proton
  and Wine builds those launchers already installed, read in place. When a
  name exists in several places, wineo's own runners root wins, then Steam,
  then Heroic, then Lutris. Portable folders never use external builds.
- The setting is offered once: in guided `wineo new` and the GUI Add flow
  when the picked template is tagged `game`, and in Heroic import. GUI
  Settings has a switch for it under New environments, and
  `wineo set --global external_runners=` changes it from the CLI.
- `wineo runners` has a new source column (`wineo`, `Steam`, `Heroic`, or
  `Lutris`), with a note when a hidden copy of a name is a different build.
  Guided `new` shows the source in the runner row, like
  `GE-Proton11-3 (Proton, Steam)`.
- With the setting on, Heroic import uses Heroic's build in place instead of
  copying it, and the review shows `GE-Proton11-1 (Heroic)`.
- `wineo runners remove` and GUI Downloads refuse builds wineo didn't
  install: "GE-Proton11-3 belongs to Steam." This also covers a
  `runner_root` that points inside a launcher's folder, even with the
  setting off.
- GUI Downloads lists other launchers' builds in a read-only "From Steam,
  Heroic, and Lutris" section, and runner pickers label them by launcher.

## 0.23.1 - 2026-10-05

- `wineo manpages DIR` writes a man page for `wineo` and each subcommand.
  The Arch package installs them, so `man wineo-new` works.
- In the desktop app, the mouse Back button does what Esc does on every
  page. Forward does nothing.
- Canceling winetricks, an installer, prefix creation, or Run program no
  longer leaves a core dump and a crash notification behind. Those child
  processes get a soft core size limit of 1, which makes the kernel skip
  systemd-coredump. Game launches keep normal limits.
- CLI `wineo exec` gets the same treatment.
- Fixed three lifecycle tests that could read `status` or the play log
  before the supervisor finished, which failed under load.

## 0.23.0 - 2026-10-04

- GUI visual polish ([0042](docs/plans/0042-gui-visual-polish.md)), following
  the new [GUI style guide](docs/gui-style.md).
- Library posters are smaller, at 160 x 240 px. Opening, closing, and
  switching the detail panel moves cards between slots with a short spring.
- Cards show the default target's enabled desktop entry name, with the
  environment name as muted context, and search matches both.
- Environments tagged `game` show Play; others show Launch.
- Detail actions, dropdowns, and menus share compact rounded controls. The
  context menu stays inside small windows, and long busy reasons scroll.
  Clicking unused Library space closes the panel.
- New-environment reviews group their fields and warnings with a fixed
  Back/Create footer. Settings, Snapshots, Downloads, Checkup, and Trash use
  the same grouped layout.
- Trash entries stay valid when the Linux mount ID of their filesystem
  changes, such as after a reboot. Previously `wineo trash`, restore, prune,
  and portable or copy recovery could report an invalid or unsupported trash
  manifest. Replacement and mount boundary checks are unchanged.

## 0.22.1 - 2026-10-02

- Process cleanup handles processes disappearing during a `/proc` scan,
  including Linux's `ESRCH` and dead tasks with cleared group fields,
  instead of treating them as inspection failures and leaving an installer
  unresolved. Permission errors still refuse cleanup.
- The crashed Proton installer test checks for no live group members,
  allowing an orphaned zombie leader to wait for the system to reap it.

## 0.22.0 - 2026-10-02

- Snapshots, portable copies, exec, and winetricks in `wineo-gui`
  ([0041](docs/plans/0041-gui-snapshots-portable-exec-winetricks.md)).
- The [Snapshots](docs/gui.md#snapshots) page creates, restores, deletes,
  and recovers snapshots with frozen reviews and cancellable preparation.
  Reports scroll inside selectable text panels with Copy controls.
- [Portable copies](docs/gui.md#copy-to-a-portable-folder) support Wine,
  Proton, and native environments, with destination reviews, progress, and
  cancellation.
- [Exec](docs/gui.md#run-program-and-winetricks) runs programs with exact
  arguments and temporary environment variables, live output, cancellation,
  and exit results.
- [Winetricks](docs/gui.md#run-program-and-winetricks) installs missing
  declared verbs or runs one-off commands, with live logs and Cancel or Force
  stop. Install missing can build a missing prefix without opening the
  external picker.
- Snapshot rollback is a separately confirmed fix in Checkup. CLI
  `wineo validate --fix` and GUI batch fixes leave it for explicit recovery.
- Filesystem and tool operations survive navigation; closing waits for
  active workers and lets publishing or restore swaps finish safely.

## 0.21.0 - 2026-10-02

- Art and downloads in `wineo-gui`
  ([0040](docs/plans/0040-gui-art-and-downloads.md)).
- An environment's [Art](docs/gui.md#art) page sets each kind from a file,
  removes it, or picks from SteamGridDB's ranked results in a thumbnail
  picker, sized per art kind. Global settings manages the SteamGridDB key.
- The [Downloads](docs/gui.md#downloads) page lists installed runners, DXVK,
  and vkd3d-proton builds with the environments using each, downloads new
  releases with progress, and deletes unused builds.
- `wineo runners remove`, `wineo dxvk remove`, and `wineo vkd3d remove`
  delete unused builds, and builds are locked while in use. See
  [Remove a runner or graphics component](docs/downloads-and-imports.md#remove-a-runner-or-graphics-component).
- The sidebar pins Trash, Checkup, and Downloads to the bottom, above
  Settings.
- Opening a menu or dialog no longer resets the library's scroll position.

## 0.20.2 - 2026-10-01

- Global settings in `wineo-gui` drops its **Back** button and no longer
  closes on Esc, like the other sidebar sections.

## 0.20.1 - 2026-10-01

- `wineo-gui`'s sidebar groups Trash and Checkup below Library and pins
  **Settings** at the bottom, replacing the top bar's gear. Ctrl+, opens it.
  See [Settings](docs/gui.md#settings).

## 0.20.0 - 2026-10-01

- Remove, trash, and validate from `wineo-gui`
  ([0039](docs/plans/0039-gui-trash-and-validate.md)). A sidebar holds
  Library, Trash, and Checkup.
- Remove an environment from its panel, with or without its prefix, and
  Undo from the toast. See [Removing](docs/gui.md#removing).
- The [Trash](docs/gui.md#trash) page restores, deletes forever, empties,
  and prunes old entries, and recovers interrupted removals.
- Check one environment for problems from its panel, or everything from the
  [Checkup](docs/gui.md#checkup) page, and apply fixes with live progress.
- Trash removal's wine wait can be canceled, and validate fixes log
  winetricks output.

## 0.19.1 - 2026-09-30

- `wineo-gui` shrinks art bigger than it's shown once and keeps the copy in
  `$XDG_CACHE_HOME/wineo/art`, so later starts skip decoding huge files and
  use far less memory. See [The library](docs/gui.md#the-library).
- The library decodes at most two images at a time, so a first pass over
  large art no longer spikes memory.

## 0.19.0 - 2026-09-30

- Add, import, and clone from `wineo-gui`: new environments from an
  installer, an existing install, or an empty registration, plus prefix
  imports, Heroic imports, and Wine and native clones
  ([0038](docs/plans/0038-gui-new-import-clone.md)). See
  [Add, import, and clone](docs/gui.md#add-import-and-clone).
- Heroic import picks one or many games from a poster grid built from
  Heroic's cached posters.
- Clickable art tiles for local files and SteamGridDB matches, and a new
  environment previews its library page as you fill it in.
- Missing runners and components can be downloaded inline, with progress,
  cancel, and live installer and winetricks logs.
- `wineo art fetch` prefers logos about 1200 px wide and heroes about
  1920 px over the biggest image, and uses images over 4096 px only when
  nothing smaller exists.
- Import sources are checked by metadata instead of content, with progress
  and cancel. Import, snapshot, clone, and portable refuse a folder that
  holds other prefixes.

## 0.18.0 - 2026-09-29

- Environment and global settings pages in `wineo-gui`, including targets,
  launch options, components, winetricks, tags, and environment variables
  ([0037](docs/plans/0037-gui-settings.md)).
- Save and Revert, unsaved-change prompts, config reloads, and keyboard
  shortcuts. Settings writes share the CLI's core validation and preserve
  TOML comments.
- Independent Icon, Grid, Hero, and Logo switches for automatic art fetching.
  All off disables fetching; existing all/icon configurations still work.
- Consistent scrolling, default storage paths shown when unset, and long
  runner labels shortened with full names on hover.

## 0.17.0 - 2026-09-28

- `wineo-gui`, wineo's first desktop app, built with iced
  ([0036](docs/plans/0036-gui-library-launcher.md)). See
  [Desktop app](docs/gui.md).
- A library of every environment with its art, state, and playtime, with
  search and a live refresh that picks up changes made from the CLI.
- A detail panel with Play, Stop, a target picker, start progress with Cancel
  and Force stop, a live log, and Clean up for lost runs and unfinished
  installers. Double-click a card to play it.
- Keyboard navigation, and light and dark themes that follow the desktop.
- The Arch package installs `wineo-gui` with a desktop entry and icon.
- Time and duration formatting moved into `wineo-core`, and
  `launch::default_target` tells frontends which target a launch uses.

## 0.16.0 - 2026-09-28

- Heroic import's decisions now live in `wineo-core`: planning a game,
  resolving its settings, planning a whole library for `--all`, and applying
  a plan ([0035](docs/plans/0035-heroic-import-in-core.md)). See
  [Heroic import](docs/core-api.md#heroic-import). CLI output is unchanged.
- Fixed: choosing "copy" in a guided Heroic import's final review and then
  going back left the new config's exe and working directory pointing into
  Heroic's original prefix instead of the copy.
- All GUI entry criteria are met, so wineo-gui (iced) is next.

## 0.15.1 - 2026-09-28

- Canceling `new` into an existing empty `--dir` now leaves the directory
  empty instead of keeping wineboot's partial files.
- A canceled prefix build stops the new prefix's wineserver before cleaning
  up, so Wine can no longer refill or recreate the directory afterwards.
- Ctrl+C at guided `wineo download`'s release prompts cancels like any other
  prompt.

## 0.15.0 - 2026-09-28

- Launch, stop, environment creation, winetricks, settings checks, status
  views, stats, and log tailing now live in `wineo-core` as operations a GUI
  can call, with progress events and a cancel token
  ([0034](docs/plans/0034-core-api-review.md)). See
  [Core API for frontends](docs/core-api.md).
- Winetricks and wineboot run in their own process group. Ctrl+C goes to
  wineo, which stops them, and a second Ctrl+C forces it.
- Canceling prefix creation prints "Canceled. Nothing was created." and exits
  with 130.

## 0.14.0 - 2026-09-28

- `tags` groups environments, like `game` or `work`, on Wine and native
  environments and in templates ([0033](docs/plans/0033-tags.md)). See
  [Tags](docs/config-files.md#tags).
- `wineo list --tag` filters by tag. Several `--tag` flags list only
  environments with all of them.
- `status NAME` and its JSON show tags, and `--tag` completes the tags in use.

## 0.13.0 - 2026-09-28

- `winetricks` lists the verbs an environment needs, and `start`, `run`, and
  `restart` install the missing ones first in one unattended call
  ([0032](docs/plans/0032-declarative-winetricks.md)). See
  [Winetricks dependencies](docs/config-files.md#winetricks-dependencies).
- What counts as installed is the prefix's own `winetricks.log`, so verbs
  installed by hand count too. A failed install fails the launch and the
  next one retries only the missing verbs.
- `status NAME` lists the verbs and marks missing ones, and
  `wineo validate --fix` installs them ahead of time.
- The winetricks runner moved into core.

## 0.12.0 - 2026-09-27

- `wayland`, `mangohud`, and a `[gamescope]` table are launch settings on the
  global config, environments, targets, and templates
  ([0031](docs/plans/0031-wayland-gamescope-mangohud.md)). See
  [Launch options](docs/launch-options.md).
- Launches run under `gamescope [flags] -- mangohud gamemoderun`, in Heroic's
  order. Under gamescope, MangoHud becomes `--mangoapp`, and Wayland adds
  `--expose-wayland`.
- On Wine, gamescope stays open until the prefix is idle, so a launcher that
  hands off to the game and exits still works.
- `status`, its JSON, and `validate` report the new settings and missing
  binaries. Runners without Wayland support are an error at launch.
- Heroic imports carry over gamescope, MangoHud, Wine Wayland, and HDR (as
  an `[env]` entry when Wayland is on).
- Docs note the `--backend sdl` workaround for gamescope's window vanishing
  on Hyprland.

## 0.11.1 - 2026-09-26

- `wineo trash` no longer mentions a prefix for entries that had none at
  removal, like native environments.

## 0.11.0 - 2026-09-26

- `wineo rm` moves an environment to recoverable trash instead of deleting
  it, and `--delete-prefix` trashes the prefix and snapshots too
  ([0030](docs/plans/0030-trash-and-prune.md)). See [Trash and removal](docs/trash.md).
- `wineo trash` lists entries, `wineo restore ID` brings one back under its
  original name and paths, and `wineo prune` permanently deletes by ID,
  `--older-than 30d`, or `--all`.
- Interrupted removal, restore, or prune shows up in `wineo validate`, and
  `validate --fix` recovers it.
- Portable folders keep their trash when moved on the same filesystem.

## 0.10.0 - 2026-09-26

- `wineo stats [NAME]` shows playtime, session counts, and per-target totals,
  including running sessions, with JSON output and optional session history
  ([0029](docs/plans/0029-playtime-stats.md)). See [Playtime statistics](docs/stats.md).
- `status` includes playtime, and lost runs count through their last heartbeat.
- Heroic imports carry over playtime; `stats --import-heroic` updates existing
  environments from Heroic's totals.
- Portable copies carry playtime, clones start fresh, and `validate` finds
  stray stats logs.

## 0.9.0 - 2026-09-26

- `wineo validate [NAME]` finds config mistakes and leftover state before a
  launch trips over them, and offers the fixes that are safe
  ([0028](docs/plans/0028-validate.md)). See [Validate](docs/validate.md).
- At a terminal, one review lists every fix. Enter applies them all, and
  numbers like `1 3 4` skip some. `--fix` applies them without asking.
- `--json` reports each problem with a stable `code`, plus `fixed` and
  `errors` for what `--fix` did.
- `wineo desktop` points stray entries at `wineo validate`.

## 0.8.0 - 2026-09-26

- `wineo new --native` is now a plain flag that picks the kind, and
  `--exe PATH` adds a default target named after the environment, for Wine
  and native environments alike
  ([0027](docs/plans/0027-new-exe-flag.md)).
- The old `--native <EXE>` form is gone. Use `--native --exe <EXE>` instead.
- `--native` without `--exe` makes an empty native environment and points at
  `wineo setup`.
- `-d`/`--desktop` on `new` gives the `--exe` target a desktop entry, and
  `import heroic` gains `-d` as the short form of `--desktop`.
- `wineo setup` picks the only target by default when editing, and the edit
  card says `Press Enter to finish editing`.

## 0.7.1 - 2026-09-25

- SteamGridDB pick lists in `art fetch`, guided `new`, and `import heroic`
  have a `Search for something else` entry, and a search with no matches
  asks for another term instead of failing when there's a terminal.
- Shell completion offers paths for path arguments like `new --native`,
  `--install-exe`, and `--dir`, which completed nothing before.

## 0.7.0 - 2026-09-25

- Native Linux games: `kind = "native"` environments launch a Linux binary
  or script directly, with no prefix or runner
  ([0026](docs/plans/0026-native-linux-games.md)). See
  [Native Linux games](docs/native.md).
- `wineo new --native <EXE>`, and guided `new` asks what kind of environment
  first.
- Native runs are tracked through their process group, so launcher scripts
  that hand off to the game stay tracked, and `stop` ends the whole group.
- `import heroic` imports native Heroic games, and skips one whose executable
  is missing.
- `status` shows the kind, and `status --json` has a `kind` field, with
  `runner` and `prefix` `null` for native environments.
- `docs/native-environment.toml.example` is a commented native config, and
  the Arch package installs it.

## 0.6.1 - 2026-09-25

- `snapshot create`, `snapshot restore`, `clone`, `portable copy`, and
  `rm --delete-prefix` wait for wine to exit in the prefix first, since
  wineserver lingers a few seconds after the last program and keeps writing
  the registry. A snapshot right after `exec` no longer fails with "reviewed
  environment changed".

## 0.6.0 - 2026-09-25

- `wineo new` and `wineo import heroic` offer to fetch SteamGridDB art for
  the environment they create, with `--art` for the unguided flows
  ([0025](docs/plans/0025-art-in-new-and-import.md)).
- `[art] fetch = "icon"` in the global config limits those flows to the icon.
- Prompts spell out their keys, like `Press Enter to create it, Esc to go
  back, Ctrl+C to cancel.`, and a review with no rows asks `Create it?`
  instead of `Choice:`.
- Guided `new` no longer lists "Download a runner or graphics tool" twice.

## 0.5.0 - 2026-09-25

- Desktop entries use icon art, the target's first, then the environment's,
  then the exe's ([0024](docs/plans/0024-desktop-entry-icons.md)).
- Art icons are converted to one PNG of up to 256x256, since some launchers
  never look in `512x512`. SVG is installed as is.
- `art set`, `art rm`, and `art fetch` update the affected desktop entries
  right away.

## 0.4.0 - 2026-09-24

- `start`, `run`, `exec`, `winetricks`, and `restart` build a missing prefix
  from a hand-written config, and `setup` offers to
  ([0023](docs/plans/0023-config-first-creation.md)).
- Launch checks that need no prefix run before the build, so a bad config
  fails fast.
- An example environment config, installed to
  `/usr/share/doc/wineo/environment.toml.example` by the Arch package.
- A relative `prefix` is refused outside portable mode instead of resolving
  against the current directory.

## 0.3.0 - 2026-09-24

- Review menus say what Enter does in the keys line, above a plain
  `Choice: ` prompt, instead of listing the accept action as `1)`
  ([0022](docs/plans/0022-review-menu-polish.md)).
- Adding a target returns to the menu that opened the program list.
- `setup`'s Finish and the installer path save without a second confirm.
- A README, and docs for creating an environment from a template.

## 0.2.0 - 2026-09-24

- Artwork store and `wineo art key`, `set`, `list`, `rm`, and `fetch`, with
  SteamGridDB fetching and the key kept in the system keyring
  ([0021](docs/plans/0021-artwork-framework.md), [docs](docs/art.md)).
- `wineo art fetch --asset` skips the fetch filters for an image you pick.
- The Arch package now depends on `dbus`.
- `stop` no longer gives up on a lost Proton run when the killed group's
  leader exits before the rest.
- Unit tests pass when run from a terminal, so `makepkg` builds without
  workarounds.

## 0.1.0

Everything from plans 0001 through 0020, before versions were tracked.
