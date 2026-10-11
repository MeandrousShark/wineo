Status: approved

# 0049: GOG account, migration, and updates

## Goal

Manage GOG games from wineo directly, without Heroic. Log in to a GOG
account, list the games it owns, move the games Heroic installed into
wineo, and keep native games updated. First of three GOG plans, and the
second half of queue item 5 in [features.md](../features.md), after
[0048](0048-steam-owned-games.md):

- **0049 (this plan):** the GOG client, account, owned list, Heroic
  migration, and updates for native games and their DLC.
- **[0050](0050-gog-installs.md):** fresh installs (native and Windows),
  Windows offline installers and their updates, and newly bought DLC.
- **0051:** Galaxy builds for Windows: differential updates, beta branches,
  and rollback.

The plan was one plan until its third review (see below). The questions
and answers here cover all of GOG; 0050 lists the ones it carries.

## Settled by the maintainer

- GOG goes direct, so store libraries don't depend on Heroic (2026-10-09,
  when queue item 5 was split).
- Games already installed through Heroic migrate into wineo, so wineo can
  update them (2026-10-10).
- Updating installed games is a requirement, not a later extra (2026-10-10).

On 2026-10-10 the maintainer answered the open questions below: 1A, 2A,
4A, 5A, 6A, 7A, 8A, 9A, 10A, and 11 with every recommended extra in.
After more detail on B, the maintainer chose a phased answer to question
3: offline installers in this plan (A), and Galaxy builds for Windows in
a later plan, 0051, with differential updates, beta branches, and
rollback, since only Galaxy builds offer those.

The same day the maintainer chose a new workspace crate, `wineo-gog`, for
everything that talks to GOG: this plan's client and file download, and
0051's Galaxy downloader. It is `publish = false` until going public at 1.0.

After a plan review the same day, the maintainer answered questions 12 to
16 with A each: Galaxy-installed games are compared by build and not
updated until 0051, snapshots, clones, and portable copies keep their
existing limits, an incomplete game refuses to launch, native updates
overwrite changed shipped files and list them, and a dry run may renew and
save the GOG login.

After a second review, the maintainer answered questions 17 to 19 with A
each: GOG installs and migrations happen in the host root only, GOG's
`postinst.sh` isn't run, and `wineo update` installs newly owned DLC and
retries failed or unavailable DLC. If a third review again finds High
issues, the plan splits into account, migration, and updates first, and
fresh installs and DLC second.

The third review found two High issues, so on 2026-10-10 the maintainer
split the plan. This plan keeps the account, migration, native updates,
and every rule about folders, receipts, and recovery. Fresh installs,
Windows offline installers, and newly bought DLC moved to
[0050](0050-gog-installs.md), and Galaxy builds became 0051. Windows games
in this plan get update checks only: Galaxy-tracked ones wait for 0051 and
installer-tracked ones for 0050.

After a fourth review, the maintainer answered question 20 with A: every
native update rebuilds all installed products from current packages.

After a fifth review, the maintainer answered question 21 with A: each
game folder's state lives in one file with a write-ahead journal, which
replaces the per-environment receipt (question 2's file) and the status
matrix.

A sixth and last review kept that design and tightened it: game files are
published atomically and durably before the commit, state files survive
unmounted drives, a lost state file is re-adopted instead of guessed, a
launch claims ownership only under an exclusive lock, the journal carries
everything needed to finish offline, and 0050's Windows installers get
their own journal contract. On 2026-10-10 the maintainer accepted those
changes without a further review.

## What GOG offers

From reading the source of heroic-gogdl 1.3.1, Heroic, Minigalaxy 1.4.3,
lgogdownloader, and Lutris, plus a few live requests (2026-10-10). None of
this is official GOG documentation.

- **Login.** Every Linux GOG tool uses the GOG Galaxy desktop client's
  OAuth client ID and secret. GOG has no public client registration. The
  login page redirects to `embed.gog.com/on_login_success?code=...`, so
  without an embedded browser the user pastes that final URL back
  (lgogdownloader's browser mode). The code is traded for an access token
  (1 hour) and a refresh token. wineo keeps the refresh token.
- **Owned games.** `embed.gog.com/account/getFilteredProducts` (paginated)
  lists owned games with titles, images, and platforms. Product details
  (`api.gog.com/products/<id>`) add installers, patches, DLC, languages, and
  art. Platform flags are sometimes wrong, so tools cross-check them against
  the installer list.
- **Offline installers.** Per OS and language: Windows is an Inno Setup
  `setup_*.exe` plus `.bin` parts up to 4 GiB each, Linux is a `.sh` that is
  a shell script, a MojoSetup binary, and a zip in one file. Download links
  need the access token and expire. Each file has an MD5 checksum. Some
  games also offer patch installers between versions.
- **Galaxy content system.** Windows and Mac only (Linux returns 404). Builds
  have a `build_id` and version name. Each build's manifest lists every file
  and chunk with MD5s, so updates download only what changed. Galaxy
  downloads are bare files: the installer's own setup (registry keys,
  DirectX and VC++ redistributables) has to be run separately, which is what
  Heroic's `setup.ts` and GOG's script interpreter do. heroic-gogdl spends
  about 3,700 lines on this.
- **Installed games.** Windows games have `goggame-<id>.info` in the install
  folder with `playTasks` (the primary exe, working dir, arguments) and,
  for Galaxy installs, `buildId`. Linux games have `gameinfo` (title,
  version) and `start.sh`. Heroic records its GOG installs in
  `~/.config/heroic/gog_store/installed.json` (path, platform, version,
  build, language, installed DLC) and each game's prefix and runner in
  `GamesConfig/<id>.json`.
- **Playtime.** `gameplay.gog.com` reports a game's total playtime.
- **Cloud saves.** Reachable without Galaxy, Windows games only, with a
  per-game token and path rules. Out of scope here.

On the maintainer's machine Heroic has two GOG games installed: RimWorld
(Linux, version `1.6.4871 rev588`, three DLCs, a `mods-1.5` folder inside
the game folder, and already a wineo native environment from 0026's tests)
and Disco Elysium (Windows, Galaxy build `59781086834418085`, its own Proton
prefix under Heroic's prefixes).

## Existing foundation

- **Environments.** Wine and Proton environments with prefixes, and native
  environments (0026). An installed GOG game fits either as is.
- **Installer runs.** `wineo new --install-exe` and the desktop app's
  installer step build a prefix and supervise an installer until its whole
  process group exits (`installer.rs`). It passes no arguments today, so
  silent switches need adding.
- **Keys.** `key_store.rs` stores a secret in the keyring with a `0600`
  file fallback, used by the SteamGridDB and Steam Web API keys.
- **Owned-list cache.** `steam/owned.rs` caches the owned list for an hour.
  The pattern carries over; the code is Steam-specific.
- **Downloads.** `download.rs` streams runners and DXVK with progress,
  SHA verification, and cancellation, but is shaped around GitHub releases:
  no bearer token on file downloads, no MD5, no resume, no disk-space check.
- **Heroic import.** Reads Heroic's sideloaded games only. Its GOG library
  is never read today.
- **Playtime.** `stats.rs` stores an imported total per environment
  (0029), today only from Heroic.
- **Gaps.** No OAuth, no POST anywhere in the HTTP code, no generic large
  file download, no installer arguments, no core function that both
  creates an environment and runs its installer (the CLI and the desktop
  app each do it), and "proxied means Steam" is assumed in about eight
  places.

## Open questions

These are the answers as given. Where a question's wording describes scope
or behavior that changed later (installing games in this plan, updating
every GOG game, failed DLC staying launchable), the proposed design below
is what holds.

### 1. What an installed GOG game is

The backlog row says both "a normal wineo environment" and "a new proxied
kind". They can't both hold.

- **A (recommended).** A normal Wine, Proton, or native environment, with
  a new `gog_id` key like `heroic_id`. wineo owns the files, so it can
  update them, and winetricks works. (Corrected after the plan review:
  snapshots, clone, and portable copies keep their existing limits for a
  game folder outside the prefix; see question 13.)
- **B.** A proxied environment with `launcher = "gog"`. Matches Steam's
  shape, but proxied environments refuse most of wineo's tools, and
  nothing else would own the game to update it.

```toml
# A: environments/disco-elysium.toml
schema_version = 1
runner = "proton"
prefix = "~/Games/Heroic/Prefixes/default/Disco Elysium - The Final Cut"
gog_id = "1771589310"
tags = ["game"]

[targets.disco-elysium]
exe = "~/Games/Disco Elysium - The Final Cut/Disco Elysium.exe"
default = true
```

### 2. Where the installed version is kept

Updates need the installed version, build, language, and DLC.

- **A (recommended).** A file wineo manages for itself, under the data
  directory (`gog/<environment>.toml`). Updates don't rewrite your config,
  and per [Stability](../stability.md) it isn't part of the contract.
- **B.** Keys in the environment's config. Visible and portable, but every
  update rewrites a file you edit by hand, and the keys become contract.

### 3. How games download and update

- **A (recommended).** Offline installers in this plan, for Windows and
  Linux. Updating downloads the new installer (or GOG's patch installer
  when there is one) and runs it over the old install. Updates are whole
  downloads, and work for every game installed from an offline installer
  (Galaxy-installed games wait for 0051, question 12). A later plan adds Galaxy builds
  for Windows, so Windows updates download only what changed.
- **B.** Galaxy builds for Windows in this plan, offline installers for
  Linux. Windows updates are small from day one, but the plan roughly
  doubles: manifests, chunks, two manifest generations, and re-creating
  the installer's setup (redistributables, GOG's script interpreter).

Under A, an update check still uses Galaxy's public build list for
Windows games, so a game Heroic installed through Galaxy (like Disco
Elysium) is compared by build, not by an installer version string it never
had.

### 4. Logging in

The login page can only redirect to GOG's own success page, so wineo can't
catch the code itself. The flow is the same in every case: wineo opens the
login page in the browser, you log in, and paste the address of the page
you land on. The question is the command:

```text
A: wineo import gog --login      wineo import gog --logout
B: wineo gog login               wineo gog logout
```

**A (recommended)** matches `wineo import steam --key` and keeps GOG under
one command. The desktop app gets a **GOG** section in Settings with **Log
in** and **Log out**, like the Steam Web API key.

### 5. The client ID

- **A (recommended).** Use the Galaxy client ID built in, like every other
  tool. If GOG ever blocks it, a wineo release changes it. Say so in the
  docs.
- **B.** The same, plus an environment variable to override it. More
  surface, little gain: nobody has another client ID to use.

### 6. Migrating Heroic's games

- **A (recommended).** Adopt in place: wineo registers the existing game
  folder and Heroic's prefix where they are, and records the GOG id and
  version. Nothing moves. Heroic's metadata is never edited, as today,
  though wineo's later updates change the adopted game folder and prefix. After
  migrating, `wineo import gog` says the game should be removed from
  Heroic's library (without deleting files) so both don't update it.
- **B.** Copy the prefix into wineo's prefix root, like `import prefix`
  without `--adopt`. Leaves Heroic's copy working, but doubles the disk
  use, and the game folder is still shared.

A game that already has a wineo environment, like RimWorld, gets its GOG id
and version attached to that environment instead of a new one.

### 7. Where new games install

- **A (recommended).** A new `defaults.games_root` setting, default
  `~/Games`, for both Windows and native games: `~/Games/<title>`. The same
  place Heroic used, so migrated and new games sit together, and a Windows
  game's files survive replacing its prefix (its registry entries and
  redistributables don't; see `--reinstall`).
- **B.** Windows games inside their prefix (`drive_c/GOG Games/<title>`),
  native games under `games_root`. Self-contained prefixes, but snapshots
  and clones copy the whole game, and replacing a prefix loses it.

### 8. Native installers

- **A (recommended).** Extract the game from the `.sh` (its zip's
  `data/noarch`) without running it, like Heroic. Deterministic, no
  MojoSetup UI, and files it doesn't ship, like RimWorld's `mods-1.5`, are
  left alone on update.
- **B.** Run the `.sh` with its unattended flags. Closer to what GOG
  intends, but runs a GUI installer that may still prompt.

### 9. Commands

```text
wineo import gog --list               owned games, with state (installed,
                                      not installed, imported, update)
wineo import gog --game 1771589310    install a game, or migrate it from
                                      Heroic when Heroic has it
wineo import gog --all                migrate every Heroic GOG game
wineo update rimworld                 update one game
wineo update --all                    update every GOG game with an update
wineo update --check                  list available updates, change nothing
```

- **A (recommended).** As above. Uninstalled games stay in the GOG listing
  (and the desktop app's Import from GOG grid), not in the library, since
  a normal environment needs its files. Updates never happen on their own.
- **B.** Also show owned, uninstalled GOG games in the library, grayed out
  like Steam's, as placeholder environments. Needs an environment with no
  prefix or files, which only proxied environments have today.

### 10. When to check for updates

- **A (recommended).** `wineo update --check`, `import gog --list`, and
  the desktop app check on demand, using results cached for an hour. The
  desktop app also checks once in the background after it starts when
  you're logged in, and marks cards with **Update available**.
- **B.** Only on demand. No network use unless you ask.

### 11. Extras in this plan

Recommended in: owned DLC (installed with the game and kept updated, since
RimWorld has three), a language setting (default English, the installer's
choice when English isn't offered), GOG's playtime imported once at
migration like Heroic's (read-only, never posted back), and GOG's own
artwork as the default poster, hero, logo, and icon. Recommended out:
cloud saves, bonus content, Galaxy builds (question 3), private beta
branches, and rolling back to an older version.

## Plan review (2026-10-10)

A review found the first draft described the successful paths but not
ownership, recovery, versions, or several contracts. The design below now
specifies them. Five points needed the maintainer and are questions 12 to
16, all answered A.

### 12. Games Heroic installed through Galaxy

Disco Elysium came from Galaxy, so wineo knows its build but has no offline
installer version to compare. An offline installer can't be matched to a
Galaxy build, so it can't safely update it.

- **A (recommended).** Until 0051, wineo compares such a game only by
  build: **Up to date**, or **Update on GOG** with the note that wineo will
  install Galaxy updates in a future release. `wineo update` doesn't touch
  it. Everything else (launch, playtime, art) works.
- **B.** Let `wineo update --reinstall NAME` move it onto the offline
  installer track: install the current offline installer over it, a whole
  download, and track installer versions from then on. 0051 could move it
  back to builds.

### 13. Snapshots, clones, and portable copies

The game folder lives outside the prefix (question 7), and today snapshots
cover only the prefix, while clones and portable copies leave targets
outside it pointing at the original files.

- **A (recommended).** Keep those limits, stated plainly: a snapshot doesn't
  undo a game update, and a clone or portable copy shares the original game
  folder, so it drops `gog_id` and the installed state and isn't
  update-eligible. Copying game folders becomes a backlog row.
- **B.** Add managed game-folder copying to snapshots, clones, and portable
  copies in this plan. A real scope increase, and snapshots of games are
  large unless the filesystem has reflinks.

### 14. Launching after an interrupted install or update

An interrupted update can leave files half replaced, even though the old
version is still recorded.

- **A (recommended).** Refuse to launch an incomplete game:
  `Finish installing disco-elysium first: wineo update disco-elysium.` A
  game whose base install finished but some DLC failed still launches.
- **B.** Launch with a warning, since the files might still work.

### 15. Shipped files you changed, on native updates

wineo records every file a native game ships. On update it deletes files
the new version dropped, unless you changed them, which it keeps and lists.
For files the new version still ships but you changed:

- **A (recommended).** Overwrite them, like GOG's own installers, and list
  them in the update summary. Mods in their own folders, like RimWorld's
  `mods-1.5`, are never shipped files, so they are untouched.
- **B.** Keep your version and skip the update for that file, listing it.
  Safer for edited configs, but can leave a mixed install.

### 16. Dry runs and GOG logins

GOG may hand back a new refresh token whenever wineo renews the login, and
the old one may stop working. A dry run that renews without saving could
log you out.

- **A (recommended).** A dry run may renew and save your GOG login, and the
  docs say so. It changes nothing else.
- **B.** A dry run never renews: with an expired access token it uses
  cached GOG data only and says it couldn't check GOG.

## Second plan review (2026-10-10)

A second review found gaps where the pieces meet: clones and locks, base
games and DLC, snapshots and receipts, and recovery. The design now
specifies those. Three points needed the maintainer and are questions
17 to 19, all answered A.

### 17. GOG games in portable folders

Ownership of a game folder has to be checked somewhere. Across portable
roots wineo can't see idle environments in other roots, and a game folder
inside a portable root moves when the root does.

- **A (recommended).** GOG installs and migrations happen in the host
  (XDG) root only. Portable copies of GOG games are allowed and, per
  question 13, don't own the game. Installing into portable roots becomes
  a backlog row.
- **B.** Support portable roots now: install folders inside a root are
  stored relative to it like prefixes, ownership is checked per root, and
  a host-wide registry tracks folders in use. More code and more cleanup
  rules.

### 18. GOG's `postinst.sh`

Some native games ship `support/postinst.sh`, which MojoSetup runs after
installing. Its side effects aren't known, so rerunning it on retry or
update isn't safe to assume.

- **A (recommended).** Don't run it, like heroic-gogdl, which only extracts
  the game files. wineo already does what these scripts usually do (exec
  bits, menu entries).
- **B.** Run it after a fresh install only, never on update or retry, with
  a failure leaving `incomplete`.

### 19. DLC bought after installing

- **A (recommended).** `wineo update` also installs owned DLC that isn't
  installed yet, retries `failed` DLC, and rechecks `unavailable` DLC, and
  lists each one in its review. That's what Galaxy does.
- **B.** `update` only updates what's installed. New DLC needs
  `wineo update NAME --dlc`.

## Third plan review (2026-10-10)

The third review found two High issues, which split the plan (see above),
and several Medium ones. All are resolved in the design below without new
questions:

- **Removing an owner** no longer clears a folder's health: folder records
  outlive their environment, and `rm` of a game mid-update is refused
  without `--force`.
- **Native setup scripts:** GOG's `postinst.sh` can assemble split files
  (RimWorld's does, when `support/split_files` exists). wineo does that
  assembly itself and refuses packages whose script does anything else.
- **Crash consistency** between the receipt and the folder record has a
  write order, an authority, and a `validate` repair.
- **Nested folders** can't both be GOG game folders.
- **Launch protection** always checks the owner by its receipt; for other
  environments it checks the paths it can see and says what it can't.
- **DLC precedence** stays stable across updates, and every package an
  update needs is fetched before anything changes.
- **Native space checks** use the packages' real unpacked sizes.
- Heroic does keep a native file list; using it is deferred.

## Fourth plan review (2026-10-10)

Two High issues: the write order made a normal crash look like corrupt
records, which blocked the recovery meant to fix it; and a migrated game's
first update had no file lists to protect its DLC. Several Medium ones
covered removal, reclaiming an incomplete folder, indirect DLC overlaps,
and split-file containment. All are resolved below. Three of the High
findings so far came from re-extracting only some products, so that rule
became a question.

### 20. Rebuilding native games on update

- **A (recommended).** Every native update re-extracts every installed
  product, the base game and then each DLC in order, from current
  packages, and applies the result as a whole. The folder ends up exactly
  as a fresh install would leave it, and file lists come out of every
  update. Every update downloads all installed DLC packages again.
- **B.** Re-extract only what changed plus everything its files could
  shift, directly, indirectly, or by exposing an earlier product's file.
  Smaller downloads, but hard to get right, and still a full rebuild when
  file lists are missing.

## Fifth plan review (2026-10-10)

Every finding, two High, was about crash consistency between two files
with statuses and a write order: a recovery that crashes again, a
same-version reinstall whose start and end look alike, files left by a
failed attempt that no committed version ships, removal and restore
crashing between writes, and whether the guarantee covers power loss.

### 21. Crash consistency

- **A (recommended).** One file per game folder holds the committed state
  and, while work is in flight, a write-ahead journal of what it will do.
  Healthy means no journal. Starting writes the journal durably before
  touching the folder; committing replaces the state and drops the journal
  in one atomic write. Recovery finishes or cleans up exactly what the
  journal names.
- **B.** Keep the receipt and the folder record, and add rows, a prior
  health field, an operation id, attempted paths, and ordered removal and
  restore steps to the matrix.

## Sixth plan review (2026-10-10)

The last review kept the journal and found gaps in what surrounds it, all
resolved below: game-file writes need their own atomic, durable protocol
(High), "folder is gone" can mean an unmounted drive (High), a lost state
file can't be rebuilt from `gog_id` alone, claiming an ownerless folder
needs an exclusive lock, offline recovery needs the resolved target
description, Windows installers can't name every path they write, and
symlinks, file types, and collisions with your own files need rules.

## Proposed design

Written for the answers above.

### The `wineo-gog` crate

- A new workspace crate, `crates/wineo-gog`, `publish = false`, inheriting
  the workspace lints and `unsafe_code = "forbid"`. It holds the GOG API:
  login URL, code exchange, refresh, the owned list, product details,
  installer links, the build list, and the file download. Plan 0051 adds
  manifests, chunks, and diffs to it.
- It knows tokens, URLs, and folders, never environments, prefixes,
  configs, or runners. `wineo-core` depends on it and decides what happens
  to games: migration, extracting packages, folder state files and their
  journals, locks, and keyring storage.
- Its tests use its own fake GOG server and fixtures, without wineo's
  environment test harness.
- `AGENTS.md`'s code boundaries gain a line for it when it lands.

### Account

- Login stores `{refresh_token, user_id}` as one secret under
  `service=wineo, account=gog`, through `key_store.rs`. The access token and
  its expiry (`expires_in`) live in memory, and wineo renews a minute
  before expiry.
- Every successful renewal saves the refresh token GOG returned, replacing
  the old one.
- Only an `invalid_grant` or 401 from the token endpoint means logged out:
  `Log in to GOG again: wineo import gog --login.` A timeout, a 429, or a
  5xx keeps the saved login and reports a temporary failure.
- Renewal, login, and logout take one host-wide GOG credentials lock. A
  renewal saves only if the stored secret is still the one it started from,
  so a stale renewal can't overwrite a newer login or bring back a login
  after logout. The CLI and the desktop app share this.
- The refresh token, access token, authorization code, and pasted redirect
  URL never appear in output, errors, logs, or `Debug` output. Errors from
  GOG requests carry the endpoint name, never the URL with its query
  string. Tests assert this for each error path, following `steam/api.rs`.

### Library cache

- The owned list and product details are cached under the cache directory,
  keyed by GOG user id (`gog-owned-<user_id>.json`), fresh for an hour,
  with `--refresh`. Logging out deletes that user's cache, and switching
  accounts never shows another account's games.
- A fetch replaces the cache only when every page arrived. A failed fetch
  or page falls back to the last complete list with a warning, like Steam's.
- Product details are public, so they're cached per product id
  (`gog-product-<id>.json`), not per account, and kept across logout.
- A request already in flight when you log out or switch accounts may
  finish, but its result is never shown or saved: every request is checked
  before it starts and validated after it returns.
- Logged out, `import gog --list` and update checks say so. Installed games
  stay playable, so being logged out isn't a `validate` issue.

### Game folders and ownership

- GOG games live in the host root only (question 17). With `--portable` or
  `WINEO_PORTABLE_DIR`, logging in and listing work, but migrating and
  updating refuse: `GOG games live in the host library. Run this without
  --portable.`
- **No overlap.** A GOG game folder can't be the same as, inside, or around
  another GOG game's folder. Migration refuses an overlap, naming the other
  game.
- **Folder state.** Each GOG game folder has one state file in the host
  data directory, `gog/folders/<hash>.toml`, keyed by the folder's
  canonical path. It is the only record of the game: internal per
  [Stability](../stability.md), and it replaces question 2's
  per-environment file. An environment's game is the folder whose state
  names it as owner and has its `gog_id`.
- **Locks.** Each folder has two host-wide locks (see Decisions during
  implementation). The **state lock** is held briefly: reads take it
  shared, and every change to the state file is made holding it
  exclusively (migration, update, `rm` of the owner, `--unset gog_id`,
  trash restore of the owner, claiming an ownerless folder, and `validate
  --fix`). The **in-use lock** is held shared by a launch for the whole
  run. Update and migration try it exclusively without waiting and refuse
  while anything runs from the folder. `rm`, `--unset gog_id`, and restore
  never take it, since changing the owner doesn't touch a running game's
  files. When both are needed, the in-use lock comes first.

### State, journal, and crashes

- **The state file** holds:
  - `owner` (an environment name, or none), the canonical `folder`, and
    the folder's filesystem identity (its device), recorded at each
    commit.
  - `[committed]`: the last finished state, absent before a first install
    finishes (0050). GOG id, platform, language as installed, the track
    (`installer` or `galaxy`), the products' order, and per product its
    installed installer version or build id and status (`installed`,
    `failed`, `unavailable`). Native games also record the winning files:
    each entry a product shipped, with its type (file, symlink, or
    directory), size and MD5 for files, target for symlinks, mode, and
    product, split files recorded as their assembled result. A migrated
    native game has no winning files until its first update.
  - `[journal]`, present only while work is in flight: an operation id;
    the kind (`install`, `update`, `reinstall`, or `repair`); the resolved
    target, meaning each product's id, target version, platform, language,
    order, and intended status, enough to write the next `[committed]`
    without asking GOG; the exact packages (GOG download and file ids, sizes,
    MD5s, and which product each belongs to); and `attempted`: every entry any
    attempt of this operation is about to write, with its type and hash,
    kept across failed attempts. 0050's Windows installs keep a different
    `attempted` (see 0050).
- **Healthy means no journal.** Any journal blocks launch, for the owner
  and every other environment launching from the folder.
- **Publishing order.** Every operation goes through the same four steps:
  1. The journal is written durably.
  2. Game entries are replaced atomically: a file is written to a
     temporary file in the same directory, named
     `.wineo-<operation id>-<n>`, synced, and renamed over its path. A
     symlink or a file replacing a symlink replaces the directory entry
     itself, never writing through it, even when the link points inside
     the folder.
  3. Every file written is synced, and every directory whose entries
     changed (new, renamed, or deleted names) is synced.
  4. The state is committed durably, replacing `[committed]` and removing
     `[journal]` in one write.
  So a journal-free state on disk always means the game files it
  describes are on disk too, even after a power loss.
- **Temporary files** are named by their operation id, so recovery deletes
  every `.wineo-<id>-*` left in the folder by its own journal's operation,
  whatever it contains. A half-written file therefore never reaches its
  real path, and never looks like one of your files.
- **Durability of the state file.** It is written to a temporary file,
  synced, renamed into place, and its directory synced, like trash markers
  (`trash.rs`), never through the plain `write_atomic`.
- **Removing the owner.** `rm` sets `owner` to none in one write and keeps
  the file, so clones still take the lock and still see a journal. `rm` of
  a game with a journal is refused: `rimworld is mid-update. Finish it
  with wineo update rimworld, or remove it anyway with --force.` With
  `--force` the journal stays.
- **Ownerless folders.** Launching from an ownerless folder without a
  journal works; with one it's refused until the game is migrated again
  and finished.
- **Missing folders.** A state file is never removed just because its
  folder can't be found: the drive may be unmounted. `validate` reports
  the folder as missing and says which. `validate --fix` removes a state
  file only when it has no journal and no owner, the folder is gone, and
  its nearest existing parent is on the same filesystem the state file
  recorded, which shows the drive is mounted and the folder really was
  deleted. Anything less certain stays, and `wineo import gog --forget
  PATH` removes it on purpose, refusing while an environment owns it.
- **Reclaiming.** Migrating or trash-restoring an owner into a folder that
  already has a state file takes the folder lock exclusively, sets `owner`
  in one write, and keeps `[committed]` and any journal as they are, so a
  half-updated folder stays blocked until `wineo update` finishes it;
  Heroic's metadata can't vouch for it. A folder owned by another
  environment refuses either one.
- **Config and state out of step.** A trash restore brings the
  environment's config back before its `owner` write, so a crash between
  them leaves an environment with `gog_id` whose folder has no owner. Its
  next migration, update, or launch claims it (see Launch protection).
  One owned by another environment is reported by `validate`.
- **A lost state file** (deleted by hand, or a damaged disk) can't be
  rebuilt from `gog_id` alone: it was the only record of the folder, the
  DLC, the language, and the order. An environment with `gog_id` and no
  state file refuses to launch and update: `rimworld's GOG state is
  missing. Migrate it again with wineo import gog --game 1094900565, or
  stop tracking it with wineo set rimworld --unset gog_id.` Migrating again works
  while Heroic still has the game; wineo never invents an installation
  from the product id.
- **Snapshot restore** brings back the environment's config but not the
  game folder, so the state file follows the folder: if the restored
  config has no `gog_id` (a snapshot from before migration), wineo puts
  the state file's `gog_id` back into it and says so. If it has a
  different `gog_id`, the restore is refused before changing anything.

### Launch protection

- **Owners** are always checked through their folder's state file,
  whatever target they launch.
- **Other environments** (clones, portable copies, hand-made ones) are
  checked when the launched program, its working directory, or an absolute
  path in its arguments is inside a GOG game folder. Windows paths are
  translated through the prefix's drive letters first. Bare program names
  that Wine resolves, and launchers that find the game some other way,
  can't be seen, and the docs say so.
- **Claiming at launch.** A launch first takes the folder's in-use lock
  shared and keeps it for the run. A launch that can't, because an update
  holds it, refuses like a folder with a journal. If it needs to claim an
  ownerless folder for its environment, it takes the state lock
  exclusively, reads the state again, writes `owner` if it's still
  unowned, and releases it. It then reads the state once more under the
  state lock shared, releases that, and starts the program. A launch never
  changes the state without the state lock held exclusively.
- A folder with a journal refuses the launch, naming the owner:
  `rimworld's game folder is mid-update. Finish it with wineo update
  rimworld.` An ownerless one says to migrate it again and finish it.
- Updating refuses while anything is running from the folder:
  `rimworld-copy is running from this game folder. Stop it before
  updating rimworld.`

### Downloads

- In `wineo-gog`: bearer token, progress, cancellation, and resume. Each
  partial file is stored with its GOG file id, expected size, and checksum.
  Resuming resolves a fresh download link (links expire), sends a Range
  request, and accepts only a 206 with a matching `Content-Range`; a 200
  restarts the file. Every finished file is verified whole: MD5 and size
  when GOG publishes a checksum, size only with a note when it doesn't.
- Packages download to the cache under their product and download id
  (`<product id>/<download id>/<GOG file name>`), with a small record per
  GOG file id, so a Windows installer's `.exe` and `.bin` parts stay
  together under the names the `.exe` looks for. They're deleted after a
  commit and kept while a journal names them, so a retry can work without
  the network (see Recovery).

### Native packages

- A native package is a `.sh` holding a zip. wineo reads the game files
  (`data/noarch`) from the zip without running MojoSetup. Each product is
  unpacked into its own staging folder beside the game, so same-named
  pieces from different products never mix.
- Entries with absolute paths or `..` are refused, executable bits are
  kept, and symlinks must point inside the folder. Writes and deletes in
  the game folder never follow an existing symlink out of it.
- **Setup scripts.** GOG's `support/postinst.sh` isn't run (question 18).
  The known GOG script only assembles split files: for each path in
  `support/split_files`, it joins `<path>.split*` in order into `<path>`
  and deletes the pieces. wineo does that itself, inside the product's
  staging folder. Each listed path must be relative and normalized, and
  the output and every piece must stay inside the staging folder. The
  assembled file is what gets recorded and counted; the pieces aren't. A
  package whose script doesn't match a known GOG script is refused before
  anything changes: `This GOG package has a setup script wineo doesn't
  support.`
- **Space.** After downloading and before changing anything, wineo sums the
  zip's unpacked sizes for every package, plus the staging copies and the
  assembled split files, and refuses with the sizes when the filesystem is
  short. Before downloading it shows the download size and an unpacked
  estimate labeled as one.

### Updating

- **Eligibility.** A native game is compared with GOG's current installer
  version for its language; a string that differs means an update. Each
  installed DLC is compared the same way, so a DLC-only update is found.
  Whenever wineo can't tell, for example a missing version or a changed
  language, it reports **Can't compare** instead of offering an update.
- **Windows games, status only in this plan.** A Galaxy-tracked game is
  compared by build: **Up to date**, **Update on GOG** (applied by 0051),
  or **Can't compare** for a build not on the default branch, like a beta.
  An installer-tracked Windows game is compared by installer version and
  shows **Update available**, applied by 0050. `wineo update` on either
  says which plan's release will do it, and changes nothing.
- **Rebuild (question 20).** Any native update, of the base game or of one
  DLC, downloads the current package of every installed product before
  changing anything. If one isn't offered any more, for example a DLC
  dropped in that language, the update is refused before any change,
  naming it.
- **Order.** The state file keeps the products' order: the base game, then
  each DLC in the order it was added. A file shipped by more than one
  product comes from the latest one in that order.
- **Applying.** Every product is unpacked and split files assembled, then
  the staging folders are layered in order into the new winning entries.
  wineo writes the journal with those entries in `attempted`, publishes
  them in the order above, cleans up, syncs, and commits.
- **Type changes.** A winning entry can replace a committed entry of another
  type (a file becoming a symlink, or a directory becoming a file) when
  the old one is tracked and unchanged; a changed or untracked one is a
  collision.
- **Collisions with your files.** When a new winning entry's path is taken
  by something wineo doesn't track, like a mod you put there, or by a
  tracked file you changed into another type, wineo moves yours aside to
  `<name>.wineo-saved` (with a number if that's taken) before publishing,
  and lists it. Nothing of yours is overwritten or deleted.
- **Old files.** A path that was a committed winning file, or is in the
  journal's `attempted`, and isn't a new winning file is deleted when its
  content matches one of the hashes recorded for it, and kept and listed
  when it doesn't. So a file from a failed attempt that no version ships
  any more is still removed, and a file you made there is not. A winning file whose current content doesn't
  match the old winning hash was changed by you, so it's overwritten and
  listed (question 15). Files that were never winning files, like
  RimWorld's `mods-1.5`, are never touched.
- **Another tool's update:** the build in `goggame-<id>.info` is trusted
  only for Galaxy-tracked Windows games, and only when its `gameId` is the
  environment's `gog_id`; then wineo records that build.

### Recovery

- `wineo update NAME`, run on the owner with the folder lock held
  exclusively, finishes any journal. It keeps the journal's kind, so an
  interrupted `--reinstall` or `repair` still rebuilds even when the
  versions match.
- **Packages.** When every package the journal names is in the download
  cache and verifies against its recorded size and MD5, recovery finishes
  with that exact set and the journal's resolved target, even offline,
  and the next check finds anything newer. Otherwise it resolves the
  current packages and target online and replaces both in the journal,
  keeping `attempted`, before writing.
- **Temporary files** from the journal's operation are deleted first.
- A crash during recovery leaves the journal, with every path the attempt
  wrote already in `attempted`, so recovery can be restarted any number of
  times.
- A blocked clone's message names the owner to run it on.
- `wineo update NAME --reinstall` rebuilds a native game from current
  packages even when nothing changed, repairing changed or missing files.
  Historical packages aren't guaranteed through GOG's download API, so it
  uses the current ones. Windows games refuse it in this plan.

### Migrating from Heroic

- Reads Heroic's metadata (`gog_store/installed.json`,
  `GamesConfig/<id>.json`) and the game folder's `goggame-<id>.info` or
  `gameinfo`, whose id must match Heroic's. Heroic's metadata is never
  edited; wineo's later updates do change the adopted game folder and
  prefix.
- Works without a GOG login: it's all local. Art and playtime from GOG are
  skipped with a note when logged out.
- **Track:** a Windows game with a `buildId` is Galaxy-tracked; one without
  is installer-tracked with Heroic's version. Native games are
  installer-tracked.
- **No winning files yet.** heroic-gogdl keeps a native file list
  (`.gogdl-linux-manifest`), but importing and checking it against the
  folder is deferred, and scanning the folder would count mods as shipped
  files. So a migrated native game has no winning files until its first
  update. That update is a normal rebuild, which writes every product's
  files and records them; with no old winning files it deletes nothing.
- **Starting order.** A migrated native game's products start as the base
  game, then its DLC in the order Heroic's `installedDLCs` lists them.
- **Mid-update folders** keep their journal when migrated (see
  Reclaiming).
- A first adoption, of a folder with no state file yet, writes one in one
  durable write, with `owner` and `[committed]` from Heroic's metadata and
  no journal. A folder that already has a state file is reclaimed instead
  (see Reclaiming), keeping its `[committed]` and journal.
- **Matching an existing environment,** by canonical paths: first an
  environment with the same `gog_id`; then one whose default target's exe
  is the game's primary play task (Windows) or `start.sh` (native) and,
  for Windows, whose prefix is Heroic's prefix for the game. More than one
  match, or a match that already has a different `gog_id`, is refused with
  the names. A matched environment keeps every setting; only `gog_id` and
  the state file are added.
- **New environments** carry over what Heroic import maps today (runner,
  arch, gamemode, environment variables, and the rest), plus Heroic's
  launch arguments, a custom executable when Heroic has one, and the
  working directory from the play task.
- **Playtime:** the imported baseline becomes the larger of the existing
  baseline and GOG's total (GOG's includes what Heroic posted), so
  migrating twice changes nothing.
- Migrating is idempotent: a game already migrated is reported as such.

### Art

- GOG's images become the poster, hero, logo, and icon when the
  environment has none, recorded in `art.toml` with source `gog`. A kind GOG
  doesn't have falls back to SteamGridDB when a key is set, else stays
  empty.

### Commands

- `wineo import gog --login`, `--logout`, `--list`, `--refresh`, and
  `--forget PATH` (removes a game folder's state file on purpose, refused
  while an environment owns it).
- `--game ID` migrates a game Heroic installed; `--all` migrates every one.
  A game that isn't installed says installing it comes in a later release
  (0050), and changes nothing.
- Every migration and update shows a review first, frozen like other
  imports, unless `--yes`. Sizes in the review are labeled when estimated.
- `wineo update [NAME|--all|--check] [--reinstall]`. `--all` updates every
  native GOG game with an update, and lists Windows games as skipped with
  the reason. `--check` with `--reinstall` is a usage error (exit 2).
  `--all --dry-run` previews every game. Without a terminal, a migration or
  update that would ask for confirmation needs `--yes`, like other
  commands, and exits 2 without it.
- **Outcomes:** `Already migrated.` and `Up to date.` are no-ops that exit
  0. A batch continues past a failed game and exits 1 with a summary.
  Cancel exits 130 and leaves the journal when files had changed.
- **JSON**, using the collection envelope from
  [JSON output](../json-output.md): `import gog --list --json` returns
  `{"games": [...], "errors": [...]}`, each game with `id`, `title`,
  `platforms`, `state` (`not_installed`, `heroic`, `imported`),
  `environment` (or `null`), `track`, `version`, `update` (`available`,
  `none`, `galaxy`, `cant_compare`, or `null`), `applicable` (whether this
  wineo can apply the update), `reason` (why not, like
  `windows_installer` or `galaxy`, else `null`), and `dlc` (`id`, `title`,
  `state`). `update --check --json` returns `{"environments": [...],
  "errors": [...]}`, each with `name`, `id`, `track`, `installed`,
  `available`, `update`, `applicable`, `reason`, and `dlc`. A game whose details can't be fetched
  goes in `errors` with its `id` and `message`; the partial result still
  prints and the command exits 1, like `list --json`.
- `--dry-run` follows [Dry runs](../dry-run.md) and question 16.

### Desktop app

- Settings gets a **GOG** section with **Log in** (opens the browser, then a
  field for the address) and **Log out**.
- **Add** gets **Import from GOG**, listing the games Heroic installed, to
  migrate with a review and progress. It follows the existing import pages:
  Back, the leave prompt for a draft, cancel during work, and closing the
  window during work asks first and cancels it like Heroic import does.
  Owned games that aren't installed join the grid with 0050.
- Library cards show **Update available** for native GOG games; the panel
  and the context menu get **Update**. Windows games show their status as
  text with no Update action. The owner of a game with a journal shows
  **Finish update** instead of Play; a blocked clone says which game to
  finish. The background check runs once after start when logged in.
- In a portable session, the GOG section shows the login but Import from
  GOG and Update say GOG games live in the host library.

## Decisions during implementation

Settled by the maintainer on 2026-10-11, during step 3:

- **Setting `gog_id` by hand.** `wineo set NAME gog_id=VALUE` is refused:
  `gog_id is set by wineo import gog.` Only migration sets it, since a
  `gog_id` without a state file is a lost state that blocks launch.
- **Unsetting `gog_id`.** `wineo set NAME --unset gog_id` also releases
  the folder: it takes the folder lock exclusively and sets `owner` to none
  in one write, like `rm`. A folder with a journal refuses it: `rimworld is
  mid-update. Finish it with wineo update rimworld.` With no state file (a
  lost state), it only edits the config.
- **An owner whose config is gone.** That only happens when a config is
  deleted by hand, since `rm` releases the folder before trashing the
  config. `validate` reports it, and `validate --fix` sets `owner` to none,
  keeping any journal, so the folder then follows the ownerless rules.

After step 3's review, also on 2026-10-11:

- **Two folder locks.** One lock held shared for a whole game run made
  `rm`, `--unset gog_id`, and restore of an owner wait, uncancelably,
  until a clone's game exited. The folder lock is split into a short-held
  state lock (step 3's lock) and a run-long in-use lock that only launches
  hold and only update and migration check (step 4). See Locks.
- **`gog_mid_update` stays an error.** `validate` exits 1 while an
  update is interrupted, since healthy means no journal and the game
  can't launch until `wineo update` finishes it.

Settled by the maintainer on 2026-10-11, before step 4:

- **What counts as a launch.** Launch protection and the in-use lock cover
  `wineo start` and the desktop app's Play (every target), and `wineo
  exec` and the Tools page's Run a program (any program), with the same
  rules. Winetricks isn't covered, since it writes to the prefix, not the
  game folder.

## Implementation sequence

1. Create `wineo-gog` with the GOG client: login URL, code exchange,
   refresh, the owned list, product details, installer links, and the
   build list, plus its fake server for tests. In `wineo-core`, token
   storage, the credentials lock, renewal rules, and a debug-only
   `WINEO_TEST_GOG_API` override, like Steam's.
2. File download in `wineo-gog`: bearer, progress, cancellation, resume,
   and verification.
3. Config `gog_id` key and validation, the folder state file with its
   journal and durable writes, locks, `validate` checks and `--fix`, and
   keeping state consistent through `rm`, trash, restore, snapshot
   restore, clone, and portable copies.
4. Launch protection for owners and other environments.
5. Native packages: zip reading, staging, safety rules, split-file
   assembly, script recognition, and space checks.
6. Heroic migration, including matching existing environments.
7. Updates: eligibility for every game, applying native updates with DLC
   precedence, old-file cleanup, recovery, and `--reinstall`.
8. CLI: `import gog` and `update`, with reviews, batches, JSON, and dry
   runs.
9. Desktop app: GOG settings, Import from GOG for migration, update
   badges and actions.
10. Playtime import, GOG art, and docs (`gog.md`, config-files,
    json-output, exit-codes, stability, README), plus backlog rows for
    copying game folders with snapshots, clones, and portable copies, for
    GOG games in portable roots, and for using Heroic's native file list.

## Testing and acceptance

Unit and CLI tests run against the fake GOG server and fixture packages: a
`.sh` built like GOG's (a shell header, a MojoSetup stand-in, and a zip),
including one with split files, one whose `split_files` names a path
outside the folder, and one with an unknown setup script. They cover the
failure paths: an interrupted download resumed (including a 200 instead of
a 206, and an expired link), a crash before and after each state-file write
and between every game-file write, a rebuild that started from a journal
crashing and being recovered again, a same-version `--reinstall` crashing
before and after its commit, update B writing a file only B ships and
failing before recovery runs with a newer C that doesn't ship it,
`rm --force` and trash restore crashing between their config and state
writes, offline recovery from cached packages and from missing ones, a
crash partway through writing a game file (the half-written temporary
file is removed and never kept as yours), a power loss before game files
are synced (simulated by checking every sync happens before the commit),
validation with the game's drive unmounted and then remounted, a lost
state file, two launches racing to claim an ownerless folder, a symlink
replaced by a file and by a directory, and a new package path already
taken by your own file, a DLC-only update, a base and a DLC sharing a file that the
DLC then drops, indirect overlaps across three products, a package no
longer offered, obsolete and changed winning files, a migrated game's first
update with DLC, a clone running while its owner updates, removing the owner
mid-update with and without `--force`, nested game folders, a snapshot
restore from before migration, symlinks in an existing game folder, launch
protection through a Windows path and through an argument, removing an
owner and launching a clone before migrating it again, reclaiming a
folder with a journal, account
switching and logout, renewal under contention, and secrets never reaching
errors.

`--ignored` tests update a real small native GOG game with
`WINEO_TEST_GOG_LOGIN`.

Acceptance checklist, drafted after implementation, will include: logging
in from the CLI and the desktop app; listing owned games; migrating
RimWorld (attaching to its existing environment, keeping `mods-1.5`) and
Disco Elysium (its Heroic prefix and build, shown as Galaxy-tracked);
checking for updates; updating RimWorld and its DLC when GOG has an update,
checking that `gameinfo`'s version and the game files actually changed;
interrupting an update and finishing it; and logging out.

## Scope limits

- In [0050](0050-gog-installs.md): fresh installs, Windows offline
  installers and their updates, `--reinstall` for Windows games, newly
  bought DLC, and the full owned-games grid.
- In 0051: Galaxy builds, differential updates, beta branches, and
  rollback.
- Not planned: cloud saves, bonus content, posting playtime to GOG, Mac
  games, copying game folders with snapshots, clones, and portable copies,
  GOG games in portable roots, and using Heroic's native file list.
- Epic stays a backlog row.
- wineo never edits Heroic's metadata.
