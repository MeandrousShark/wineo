# Changelog

A finished plan bumps the minor version and fixes between plans bump the
patch version. A plan released in parts bumps the minor version for each
part. See `AGENTS.md` for the rule.

## 1.0.1 - 2026-10-29

Fixes for loose ends found while getting CI running on Ubuntu.

- On ext4, removing an environment and creating one with the same name
  could look like nothing changed to the desktop app, which then kept the
  old one's art page open. wineo now tells a config file from a newer one
  by its birth time as well, where the filesystem records one. The same
  check guards the Heroic id that `import heroic` writes into a config.
- Behavior changes:
  - Installing or updating a Windows GOG game without a graphical session,
    such as over SSH, now gives the real answer first when nothing would
    run: no Galaxy build, no Windows installer, already up to date, or a
    branch or password problem. Before, it asked you to run it from your
    desktop, where you'd only then learn there was nothing to do. Anything
    that runs an installer or setup still needs a desktop.
  - `wineo update NAME` without a graphical session says "Updating a
    Windows game needs a graphical session", like `wineo update --all`,
    instead of "Installing".

## 1.0.0 - 2026-10-29

Wineo is now considered stable (plan 0058). See
[Stability](docs/stability.md) for what's promised to stay stable until
the next major release.

New in this release:

- Wineo's source is public at
  [github.com/MeandrousShark/wineo](https://github.com/MeandrousShark/wineo),
  and pull requests there run CI. See
  [Contributing](README.md#contributing).
- Each release from 1.0 has prebuilt `wineo` and `wineo-gui` binaries for
  x86_64 Linux, built for Ubuntu 22.04 and newer (glibc 2.35) and checked
  in a clean Ubuntu 22.04 before they're published. The README lists the
  packages the desktop app needs on Ubuntu or Debian.
- The `PKGBUILD` builds from the GitHub repository over HTTPS, so anyone
  can run `makepkg` without access to the maintainer's server.
- [Stability](docs/stability.md) now names the desktop app's own files,
  `library.toml` and `themes/`, and `wineo-gog`'s Rust API as not
  promised.
- The test suite passes on Ubuntu and ext4 as well as Arch and btrfs. Tests
  no longer lean on the host's display, `wine`, gamescope, MangoHud,
  winetricks, fonts, or reflink support.

## 0.39.1 - 2026-10-28

A fix for Steam's library window opening on a cold launch.

- Behavior changes:
  - When Steam isn't running, launching a Steam game starts it with
    `steam -silent` first and sends `-applaunch` once its client is up, up
    to a minute later. Steam ignores `-silent` when `-applaunch` comes in
    the same cold start, so 0.39.0's single command still opened the library
    window. `wineo start` returns once the request is handed off. A launch
    with Steam already running is sent right away, as before.

## 0.39.0 - 2026-10-28

Color themes for the desktop app, from
plan 0057.

- Behavior changes:
  - Global Settings has two tabs, General and Appearance. Everything that
    was there before is on General, unchanged.
  - Midnight's danger red is a touch lighter (`#ec625a`), so error text
    passes 4.5:1 on panels.
  - `wineo validate` warns about an `[appearance]` table that names a
    missing, unreadable, or wrong-kind theme, and about a theme file in
    `themes/` that doesn't parse (`theme_invalid`).
  - Launching a Steam game starts Steam with `-silent`, so a launch that
    has to start Steam no longer opens its library window. A Steam that's
    already open stays as it is.
- Eight built-in themes: Midnight and Daylight (the old dark and light),
  Cellar and Cellar Light, High contrast, Dracula, Catppuccin Macchiato, and
  Catppuccin Latte. Following the system's light or dark mode stays on by
  default, with a dark and a light theme picked separately; turning it off
  picks one theme for both.
- Picking a theme previews it right away. Save keeps it, Revert or Discard
  puts back what was saved, and one Save covers both tabs.
- Improve readability, off by default, nudges any color that misses its
  contrast target until it passes, keeping its hue. It works on custom
  themes too.
- Custom themes: copy any theme, then change its colors with a hex field
  per role, rename it, or delete it. Each is a file in the config root's
  `themes/` folder, and a theme that's missing a color takes it from the
  theme it was copied from.
- Appearance settings live in an `[appearance]` table in `library.toml`.
  Older wineo versions ignore it.
- `crates/wineo-gui/THIRD_PARTY.md` credits the Dracula, Catppuccin, and
  Ember colors.

## 0.38.1 - 2026-10-27

Fixes for a reboot that made wineo distrust its own trash, and a small touch
to hidden cards.

- Behavior changes:
  - A trash entry wineo can't verify no longer stops every environment from
    loading. It blocks only its own environment when its manifest still
    reads and its paths match its name, and every environment otherwise.
    `wineo validate`, Checkup, the blocked card's panel, and a banner above
    the grid say what it blocks and why.
  - `wineo trash` lists unreadable entries instead of failing, and
    `wineo prune ID` can delete one, after its confirmation, when everything
    it kept is inside its trash folder. The app's Trash page and Checkup
    offer the same **Delete this entry**. `validate --fix`, `--older-than`,
    and `--all` never delete them.
  - A hidden card's eye moved to the poster's bottom-left corner, so the
    star has the top-right to itself. In poster-only, the title leaves room
    for it.
- Trash entries, snapshot restore recovery, and GOG folder records survive a
  reboot that renumbers a btrfs subvolume's device. wineo now tells
  filesystems apart by their statfs ID, which also keeps a btrfs snapshot
  from passing for the original. Records written before this keep the old
  checks.
- Identity checks read every field through one file handle, so a path
  swapped partway through a check can't mix two objects.

## 0.38.0 - 2026-10-26

Small touches to the desktop app, from
plan 0056.

- Behavior changes:
  - A Wine or native environment can have a `title`, the name its card
    shows. `wineo set` and the Settings page take it, `wineo clone` copies
    it, and `wineo validate` warns about one that breaks the rules
    (`title_invalid`). wineo 0.37.0 and older refuse to load a Wine or
    native environment with a `title`.
  - The grid is centered, so leftover width splits evenly on both sides.
  - A hidden card's eye now always sits in the second mark slot, below
    the star's.
  - Every poster keeps room for the star in its corner, so a GOG update
    pill turns into the round badge a little sooner on narrow cards.
  - New environment, Import prefix, and Clone put Back at the top and
    their Next, Review, Create, Import, or Clone in a strip pinned at the
    bottom, like the Heroic, Steam, and GOG pages.
- Star a game from its poster: an outline star shows on hover, and a
  favorite's filled star is always shown. Clicking it toggles the favorite
  without opening the panel. The star lights up under the pointer and pops
  when it favorites a game.
- The hover star and the poster-only title fade in and out.
- A card's details fade and drift in and out when the zoom crosses 90%,
  instead of popping in.
- Card titles: the card, the panel, and the poster-only overlay show your
  title, then a Steam game's name, then the desktop entry's name, then the
  environment name. Search matches the title and the name, and Sort by
  name uses the title.
- The choice lists on Add, New's program list, Heroic's and GOG's lists,
  and the version picker take keyboard focus: Up and Down move, Enter
  chooses, and the focused row scrolls into view.
- The zoom slider's 100% snap is easier to hit.
- Fixed: the discard prompt on New and Import from Heroic no longer resets
  the form's scroll position and focus, and a card stays hovered when the
  pointer jumps straight onto it.

## 0.37.0 - 2026-10-22

Grid zoom and a poster-only view in the desktop app's library, part 2 of
the library views work, from
plan 0055.

- Behavior changes:
  - **Add** moved to the toolbar's far left, before search, and now takes
    focus first, so Tab walks the toolbar in the order it's drawn. Search
    shrinks to about half its width before it clips.
  - A re-sort, filter, search, or Show hidden change used to snap the
    cards to their new places. They now spring there, new cards fade in,
    and leaving ones fade out. A window resize and the background rescan
    still snap.
  - `library.toml` gains `zoom` and `poster_only` under `[view]`.
    `wineo validate` and Checkup know both. An older wineo reports them as
    unknown keys, a warning only. A library that never changes them writes
    no file.
- Zoom the grid from 60% to 200% with the new slider, `Ctrl+=`, `Ctrl+-`,
  or Ctrl with the mouse wheel; `Ctrl+0` goes back to 100%. Keys and
  wheel notches step by 10 and the slider snaps to 100% near it. The app
  never shows a percentage, and remembers the zoom.
- Zooming keeps your place: the selected card, or the first visible one,
  stays where it was on screen.
- The poster-only toggle beside the slider hides the titles under the
  posters. Hover or selection shows the title over the art. Games that are
  running, installing, or broken show a status dot in the poster's corner.
- A GOG update pill turns into a round badge when its words don't fit a
  small poster.
- Cards only load art while they are in view.

## 0.36.0 - 2026-10-19

Sorting, filters, and favorites in the desktop app's library, part 1 of the
library views work, from
plan 0054. What the app
remembers about each environment lives in a new, lenient
`~/.config/wineo/library.toml`.

- Behavior changes:
  - wineo keeps `library.toml` up to date from the CLI too. Creating or
    importing an environment records when it was added, `rm` drops its
    entry and keeps it with the environment in the trash, `restore` brings
    it back, a clone starts with a fresh entry, and a portable copy takes
    its entry along.
  - `wineo validate` checks `library.toml` and lists problems with the
    whole file under its own heading. `--fix` repairs a file that can't be
    read, keeping the original as `library.toml.broken-<time>`, and removes
    entries for environments that no longer exist. A file from a newer
    wineo is refused with a hint to update. Checkup in the app does the
    same.
  - Problems updating library entries while migrating GOG games show as
    warnings in the CLI and the app.
- Sort the library by name, last played, playtime, or date added. Each key
  starts in its natural direction, the arrow flips it, and **Favorites
  first** puts favorites ahead. The sort is saved.
- **Filters** narrows the library by store (Steam, GOG, Epic, Amazon),
  platform (Native, Windows), and tag. While any are on, a tab under the
  button shows each with an x, plus one x that clears them all. Search
  matches tags too.
- A Steam game counts as Windows once Steam shows it runs through Proton.
- Star favorites from the panel, the right-click menu, or `F`, and hide
  games you don't want to see; **Show hidden** brings them back dimmed.
  Posters mark both.
- Empty states for no matches and for everything hidden, and a banner with
  **Open Checkup** when `library.toml` has a problem.
- The Sort menu and Filters popup work from the keyboard.
- The library's refresh reads stats logs and `library.toml` only when they
  change, roughly halving the app's idle CPU.
- The right-click menu grows to 600 px before it scrolls, and Enter no
  longer acts on a card while the menu is open.
- Scripts to generate a benchmark library and measure the app's idle CPU
  and scrolling.

## 0.35.1 - 2026-10-17

Loose ends from plan 0051's possible bugs
and a flaky process scan.

- Behavior changes:
  - A Proton run stays running until everything in umu's process group has
    exited, not just umu. A launcher that hands off to the game and exits no
    longer ends the run while the game plays, and a stop keeps escalating
    until those processes are gone. Snapshots, clones, portable copies, and
    winetricks wait for them too.
  - After such a handoff, `wineo status` shows the program as exited with
    other processes keeping the prefix running, and `status --json` has
    `run.program_exited_at`, as Wine runs already did.
- Process-group checks no longer fail with "process-group members could not
  be inspected" when any process on the machine has a name that isn't valid
  UTF-8.
- The app's Art page keeps its previews when the library refreshes instead
  of decoding them again, and an image that can't be decoded says "Can't
  show this image" instead of loading forever.
- Settings shows default folders as `~/Games` and similar, the form a config
  accepts.
- Tests no longer leave fake Wine or winetricks processes running after a
  failure.
- A `release-fast` Cargo profile for quicker optimized local builds.

## 0.35.0 - 2026-10-16

The config contract, from plan 0053:
configs and state from earlier releases are tested to keep loading, renamed
keys and flags get a warning period, and wineo never quietly swaps a config
it can't use for defaults. This closes 1.0 criteria 4 and 5.

- A key wineo doesn't know is refused with a hint: `A newer version of wineo
  may have written it; update wineo or remove the key.` An unsupported
  `schema_version` adds `A newer version of wineo wrote it; update wineo to
  use this file.`
- Deprecated config keys: a key renamed in 1.x keeps working under its old
  name until 2.0. `wineo validate` reports it as `deprecated_key` (in the new
  JSON area `config`), `--fix` renames it in place, and commands print
  `warning: N configs use deprecated keys; see wineo validate`. Nothing is
  deprecated yet.
- Renames before 1.0, with the old spellings still working:
  `wineo rm --include-prefix` (was `--delete-prefix`, which now warns),
  `snapshot remove` (was `snapshot delete`, which now warns), and every
  removal command takes both `rm` and `remove`. `status NAME --json`'s
  `dxvk` and `vkd3d` objects add `configured`, still also written as
  `wanted`.
- Upgrade tests run against configs and state written by seven earlier
  releases, from v0.2.0 on.
- GOG folder states and Galaxy records accept fields from newer releases,
  and redistributable cleanup removes nothing when a record can't be read.
- Behavior changes:
  - An unknown key or unsupported version in `config.toml` makes commands
    that act on settings refuse (`set`, `new`, `start`, removals, portable
    copies, and more) instead of quietly using the defaults. Other damage
    still uses the defaults, now with `warning: using default settings:
    ...`. `status`, `stop`, and finishing an interrupted restore warn and
    carry on.
  - Removing a runner, DXVK, or vkd3d build refuses when an environment or
    template config can't be read, instead of warning and removing it.
  - While `art.toml` can't be read, desktop entries keep their icon and say
    why, instead of losing it.
  - `wineo status` warns about any problem with the global config, not only
    an unsupported version.
  - JSON: the validate code `restore_interrupted` is now
    `snapshot_restore_interrupted`.
- Fixed: one unreadable trash manifest was reported as an art problem for
  every environment. It's now reported once, as `trash_invalid`.

## 0.34.1 - 2026-10-16

Import logs no longer share a name with environments.

- Heroic import logs (written with `-v`) moved from
  `~/.local/state/wineo/imports/` to `~/.local/state/wineo/.imports/`.
  Before, an environment named `imports` used the same folder for its
  state, so `wineo rm imports` deleted every game's import log.
- Logs written before this release stay in the old folder, and
  `wineo validate` doesn't offer to remove it.

## 0.34.0 - 2026-10-16

GOG Galaxy builds, part B, which finishes
plan 0052: Galaxy games can follow a beta
or legacy branch, roll back to an older build and stay there, and use
private branches.

- `wineo update NAME --builds` lists the builds GOG offers by branch, with
  `--json`. `--branch NAME` follows a branch, `--default-branch` returns to
  the default one, `--build ID` (an id or a version name) installs a build
  and pins the game to it, and `--latest` unpins. `import gog --game` takes
  `--branch` and `--build` too.
- A pinned game stays on its build: `update --all` leaves it alone, plain
  `wineo update NAME` says how to unpin and exits 0, and `update --check`
  shows `pinned`. Newly owned DLC still installs from the pinned build.
- Private branches ask for their password at a terminal, or read
  `--password-stdin`, and save it in the keyring for the account that
  entered it. Logging out or switching accounts removes them.
- The desktop app has a Version row and picker in a GOG game's panel:
  branches, builds, pinning, Repair, and moving between Galaxy and offline
  installers. A branch or pin change that moves no files reviews with
  **Switch**. The update page asks for a missing private branch password
  instead of pointing at the CLI, and its button names what it does.
- JSON: `update --check --json` and `import gog --list --json` add `branch`
  and `pinned`, and `reason` can be `pinned`.
- Behavior changes: the app's Update row checks the game again after an
  update, so it shows **Up to date** instead of disappearing; `update --all
  --track galaxy` is now a usage error instead of ignoring `--track`; and a
  native game's branch flags say only `Only Galaxy games have branches.`
- Fixed: switching to an older Galaxy build whose manifest has no build id
  (like The Witcher 1.5) failed with "GOG's answer isn't shaped as
  expected".
- A new app logo: a window with a glass of wine.
- Packaging: release builds no longer warn about unused imports, and the
  PKGBUILD skips makepkg's debug package, which the stripped binaries left
  empty.

## 0.33.0 - 2026-10-15

GOG Galaxy builds, part A (plan 0052):
Windows GOG games install and update from GOG Galaxy's builds, file by
file, so updates download only what changed.

- `wineo import gog --game` installs Windows games on the Galaxy track
  when GOG has a Galaxy build; `--track installer` keeps the offline
  installers. Setup (GOG's script interpreter, redistributables) runs in
  the prefix, and redistributables are shared between games.
- Galaxy games update with `wineo update`, including ones Heroic or the
  GOG Galaxy client installed. Disco Elysium no longer says "Update on GOG".
- `wineo update NAME --repair` checks every file of a Galaxy game and
  fixes damaged ones, saving the damaged copy aside.
- `--reinstall --track galaxy|installer` moves a game between tracks,
  keeping files that already match.
- The desktop app installs and updates on either track, with a Track
  picker in the install review.
- `update --check` and `import gog --list` show GOG's version names for
  Galaxy builds; JSON keeps build ids, and Galaxy updates are `applicable`.
- Every failure that leaves an unfinished operation ends with the command
  that finishes it.

## 0.32.1 - 2026-10-14

Safer GOG installer updates.

- Installing or updating a GOG game from its installer never overwrites or
  deletes a file in the game folder that wineo hasn't checked first. Before,
  a file another program created or changed while wineo was publishing
  could be replaced or removed. wineo now places files with a rename that
  refuses to replace, and moves its own old files aside before checking and
  deleting them. Anything that isn't wineo's is kept or saved as
  `<name>.wineo-saved`.
- Files the game's previous version shipped are still updated in place, as
  before.
- On filesystems that can't rename without replacing (like NFS), installs
  keep working as before.
- A folder that appears where a new file goes is saved aside instead of
  failing the update.

## 0.32.0 - 2026-10-14

Lean and simple (plan 0051): wineo does
less while nothing is happening, uses less memory, and its code is easier
to read. Everything you see works as before, except where noted.

- The desktop app does nothing in the background: no rescans and no
  redraws while it isn't focused, and when focused it only redraws when
  something changed. On a real GPU, background CPU went from 1.6% to 0%.
  Refocusing refreshes at once.
- The app holds less art in memory (a 96 MiB budget for posters, one copy
  per card), about 210 MB less on a 300-game library. A closing panel keeps
  its art through the fade.
- Import pages decode posters at most 8 at a time (half your cores),
  shrunk to the tile and cached on disk: peak memory on a 300-game page
  went from about 1 GB to 150 MB, and reopening a page is near instant. A
  progress bar shows while art is still being prepared.
- `wineo list` and `status` are about 2.5x faster and use about 25% less
  memory (one `/proc` pass for Steam, the time zone read only when shown,
  the trash read once), and about 10x faster with a full trash. Bulk
  Heroic imports parse Heroic's library once instead of once per game, and bulk imports share one HTTP client and one GOG login.
- Copies and snapshots read your prefix one fewer time (snapshot create on
  a 376 MB prefix: 0.87 s to 0.63 s), keeping every safety check.
- Stopping a game or tool goes through one helper. It fixes two places
  that could signal a recycled process group, and a stop it can't confirm
  now says so instead of reporting success. A force-stop kills at once; a
  cancelled tool waits until its whole process group is gone.
- Toasts, scrollbars, the hook output reader, and the art picker no longer
  poll; the supervisor still wakes about once a second during a game.
- The keyring uses the `secret-service` crate on zbus, the D-Bus library
  the app already uses, so building wineo no longer needs libdbus.
  Removing a key from a locked keyring asks you to unlock it first.
- Release builds use full LTO: the app's binary is 44.2 to 34.8 MB, and
  `wineo` 17.4 to 16.9 MB including the new keyring library.
- Steam launches are checked 4 times a second for the first 10 seconds,
  then once a second.
- The app's prefix import review now refuses what the CLI refuses (a
  destination another environment names, nested prefixes when adopting).
  Portable copies say "destination appeared during review/publication"
  instead of a raw rename error, and clone's final check can be cancelled.
- CLI tables line up accented and CJK titles.
- JSON: `windows_installer`, which 0.31.0 stopped reporting as an update
  `reason`, is gone from the vocabulary and the docs.
- `wineo trash` with an empty trash and `wineo list` with no environments
  say so on stderr (`Trash is empty`, `No environments. Create one with:
  wineo new`), like `status`'s `Nothing is running`. Piped output stays
  empty.
- Building with tests from a terminal, as `makepkg` does, no longer hangs
  on two `art fetch` tests waiting for input.

## 0.31.1 - 2026-10-13

First fixes from plan 0051, which trims
what wineo does while nothing is happening.

- A running game costs almost nothing to supervise. The background
  supervisor woke about 100 times a second for the whole session; it now
  wakes about once a second. An attached `wineo run` went from 50 wakeups a
  second to none.
- A native game whose launcher hands off to another process and exits is
  checked once a second instead of 10 times, so the end of such a session
  is noticed up to a second later. Steam launches and after hooks waiting
  on another launch also stop polling.
- A run that is lost in the middle of a long stop keeps an accurate end
  time in its play log.
- The desktop app does about half the work while the mouse moves over it,
  and spinners only animate while they're on screen.

## 0.31.0 - 2026-10-13

- GOG installs (plan 0050). `wineo import gog
  --game ID` now installs a game you own that isn't in wineo yet, natively
  or from its Windows installer, with the DLC you own, into
  `defaults.games_root` (default `~/Games`). `--platform`, `--language`,
  `--runner`, `--arch`, and `--dir` choose how, and `--dry-run` shows the
  review. A new `gog.language` setting takes a locale tag like `fr-FR`.
  An interrupted install resumes from where it stopped when you install
  again or run `wineo update NAME`, even offline. See [GOG
  games](docs/gog.md).
- `wineo update` now updates Windows installer games and `--reinstall`
  repairs them, installs DLC bought since the game was installed, and
  retries DLC that failed or wasn't offered before.
- The desktop app's **Import from GOG** shows every game you own with its
  poster, installs any of them through a review like the CLI's, and
  finishes an install that didn't. Settings has the games folder and GOG
  language.
- Settings shows only the key button that applies for SteamGridDB and the
  Steam Web API: **Set key...** with no key, **Remove key** with one.
- Behavior change: `new --install-exe`, guided `new`, and the desktop app's
  installer step now behave the same. Each sets up DXVK and vkd3d-proton
  before the installer runs, and each asks which program to use afterwards
  whatever the installer's exit code. A canceled installer keeps the
  environment and points to `wineo setup NAME`.
- JSON: `import gog --list --json` says `installed` for games wineo
  installed (`imported` stays for migrated ones), a DLC's `state` can be
  `not_installed`, and `update --check --json` no longer reports
  `windows_installer` as a reason.

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
  (plan 0049), without Heroic. `wineo import gog
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

- Owned Steam games (plan 0048). With a
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

- Steam games (plan 0047). `wineo import
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

- Launch hooks (plan 0046). `[hooks]` with
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

- Playtime by tag (plan 0045).
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
  (plan 0044). It prints the same review a real
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
  (plan 0043). With
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

- GUI visual polish (plan 0042), following
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
  (plan 0041).
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
  (plan 0040).
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
  (plan 0039). A sidebar holds
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
  (plan 0038). See
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
  (plan 0037).
- Save and Revert, unsaved-change prompts, config reloads, and keyboard
  shortcuts. Settings writes share the CLI's core validation and preserve
  TOML comments.
- Independent Icon, Grid, Hero, and Logo switches for automatic art fetching.
  All off disables fetching; existing all/icon configurations still work.
- Consistent scrolling, default storage paths shown when unset, and long
  runner labels shortened with full names on hover.

## 0.17.0 - 2026-09-28

- `wineo-gui`, wineo's first desktop app, built with iced
  (plan 0036). See
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
  a plan (plan 0035). See
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
  (plan 0034). See
  [Core API for frontends](docs/core-api.md).
- Winetricks and wineboot run in their own process group. Ctrl+C goes to
  wineo, which stops them, and a second Ctrl+C forces it.
- Canceling prefix creation prints "Canceled. Nothing was created." and exits
  with 130.

## 0.14.0 - 2026-09-28

- `tags` groups environments, like `game` or `work`, on Wine and native
  environments and in templates (plan 0033). See
  [Tags](docs/config-files.md#tags).
- `wineo list --tag` filters by tag. Several `--tag` flags list only
  environments with all of them.
- `status NAME` and its JSON show tags, and `--tag` completes the tags in use.

## 0.13.0 - 2026-09-28

- `winetricks` lists the verbs an environment needs, and `start`, `run`, and
  `restart` install the missing ones first in one unattended call
  (plan 0032). See
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
  (plan 0031). See
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
  (plan 0030). See [Trash and removal](docs/trash.md).
- `wineo trash` lists entries, `wineo restore ID` brings one back under its
  original name and paths, and `wineo prune` permanently deletes by ID,
  `--older-than 30d`, or `--all`.
- Interrupted removal, restore, or prune shows up in `wineo validate`, and
  `validate --fix` recovers it.
- Portable folders keep their trash when moved on the same filesystem.

## 0.10.0 - 2026-09-26

- `wineo stats [NAME]` shows playtime, session counts, and per-target totals,
  including running sessions, with JSON output and optional session history
  (plan 0029). See [Playtime statistics](docs/stats.md).
- `status` includes playtime, and lost runs count through their last heartbeat.
- Heroic imports carry over playtime; `stats --import-heroic` updates existing
  environments from Heroic's totals.
- Portable copies carry playtime, clones start fresh, and `validate` finds
  stray stats logs.

## 0.9.0 - 2026-09-26

- `wineo validate [NAME]` finds config mistakes and leftover state before a
  launch trips over them, and offers the fixes that are safe
  (plan 0028). See [Validate](docs/validate.md).
- At a terminal, one review lists every fix. Enter applies them all, and
  numbers like `1 3 4` skip some. `--fix` applies them without asking.
- `--json` reports each problem with a stable `code`, plus `fixed` and
  `errors` for what `--fix` did.
- `wineo desktop` points stray entries at `wineo validate`.

## 0.8.0 - 2026-09-26

- `wineo new --native` is now a plain flag that picks the kind, and
  `--exe PATH` adds a default target named after the environment, for Wine
  and native environments alike
  (plan 0027).
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
  (plan 0026). See
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
  (plan 0025).
- `[art] fetch = "icon"` in the global config limits those flows to the icon.
- Prompts spell out their keys, like `Press Enter to create it, Esc to go
  back, Ctrl+C to cancel.`, and a review with no rows asks `Create it?`
  instead of `Choice:`.
- Guided `new` no longer lists "Download a runner or graphics tool" twice.

## 0.5.0 - 2026-09-25

- Desktop entries use icon art, the target's first, then the environment's,
  then the exe's (plan 0024).
- Art icons are converted to one PNG of up to 256x256, since some launchers
  never look in `512x512`. SVG is installed as is.
- `art set`, `art rm`, and `art fetch` update the affected desktop entries
  right away.

## 0.4.0 - 2026-09-24

- `start`, `run`, `exec`, `winetricks`, and `restart` build a missing prefix
  from a hand-written config, and `setup` offers to
  (plan 0023).
- Launch checks that need no prefix run before the build, so a bad config
  fails fast.
- An example environment config, installed to
  `/usr/share/doc/wineo/environment.toml.example` by the Arch package.
- A relative `prefix` is refused outside portable mode instead of resolving
  against the current directory.

## 0.3.0 - 2026-09-24

- Review menus say what Enter does in the keys line, above a plain
  `Choice: ` prompt, instead of listing the accept action as `1)`
  (plan 0022).
- Adding a target returns to the menu that opened the program list.
- `setup`'s Finish and the installer path save without a second confirm.
- A README, and docs for creating an environment from a template.

## 0.2.0 - 2026-09-24

- Artwork store and `wineo art key`, `set`, `list`, `rm`, and `fetch`, with
  SteamGridDB fetching and the key kept in the system keyring
  (plan 0021, [docs](docs/art.md)).
- `wineo art fetch --asset` skips the fetch filters for an image you pick.
- The Arch package now depends on `dbus`.
- `stop` no longer gives up on a lost Proton run when the killed group's
  leader exits before the rest.
- Unit tests pass when run from a terminal, so `makepkg` builds without
  workarounds.

## 0.1.0

Everything from plans 0001 through 0020, before versions were tracked.
