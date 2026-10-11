Status: draft

# 0049: GOG games

## Goal

Install, launch, and update GOG games from wineo directly, without Heroic.
Log in to a GOG account, list the games it owns, download and install them
into ordinary wineo environments, and keep them updated. Games Heroic
already installed move into wineo so wineo can update them from then on.
Second half of queue item 5 in [features.md](../features.md), after
[0048](0048-steam-owned-games.md).

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
the next plan, 0050, with differential updates, beta branches, and
rollback, since only Galaxy builds offer those.

The same day the maintainer chose a new workspace crate, `wineo-gog`, for
everything that talks to GOG: this plan's client and file download, and
0050's Galaxy downloader. It is `publish = false` until going public at 1.0.

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
  downloads, but they work for every game. A later plan adds Galaxy builds
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
  version. Nothing moves. Heroic's files are never edited, as today. After
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
  game survives replacing its prefix.
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
16.

### 12. Games Heroic installed through Galaxy

Disco Elysium came from Galaxy, so wineo knows its build but has no offline
installer version to compare. An offline installer can't be matched to a
Galaxy build, so it can't safely update it.

- **A (recommended).** Until 0050, wineo compares such a game only by
  build: **Up to date**, or **Update on GOG** with the note that wineo will
  install Galaxy updates in a future release. `wineo update` doesn't touch
  it. Everything else (launch, playtime, art) works.
- **B.** Let `wineo update --reinstall NAME` move it onto the offline
  installer track: install the current offline installer over it, a whole
  download, and track installer versions from then on. 0050 could move it
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

## Proposed design

Written for the answers above and the recommended answers to 12 to 16.

### The `wineo-gog` crate

- A new workspace crate, `crates/wineo-gog`, `publish = false`, inheriting
  the workspace lints and `unsafe_code = "forbid"`. It holds the GOG API:
  login URL, code exchange, refresh, the owned list, product details,
  installer links, the build list, and the file download. Plan 0050 adds
  manifests, chunks, and diffs to it.
- It knows tokens, URLs, and folders, never environments, prefixes,
  configs, or runners. `wineo-core` depends on it and decides where games
  go and what happens in the prefix: running installers, extracting `.sh`
  files, Heroic migration, the installed-state file, locks, and keyring
  storage.
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
- Logged out, `import gog --list` and update checks say so. Installed games
  stay playable, so being logged out isn't a `validate` issue.

### Game folders and ownership

- Each GOG game has one install folder, recorded by canonical path in its
  installed state. Windows games install to `<games_root>/<title>`, native
  games the same; `--dir` overrides.
- A folder belongs to at most one environment. Installing or migrating into
  a folder another environment already owns is refused, naming the owner.
  Per question 13, clones and portable copies drop `gog_id` and the state.
- A host-wide game-folder lock, keyed by canonical path like prefix locks,
  is held by launch, install, update, and migration, across portable roots.
  Updating a running game is refused: `Stop disco-elysium before updating
  it.`

### Installed state

- `gog/<environment>.toml` under the data directory, internal per
  [Stability](../stability.md). It records: GOG id, platform, install folder,
  language, the track (`installer` or `galaxy`), the installed installer
  version or build id, a status (`installed`, `installing`, `updating`,
  `incomplete`), the target version during an install or update, and per
  DLC: id, installer version, and status (`installed`, `failed`,
  `unavailable`). Native games also record the shipped file list with
  sizes and MD5s.
- `rm`, trash and restore, and snapshot restore keep it consistent with
  the environment: removed with it and restored with it. A snapshot restore that doesn't cover the game folder
  leaves the state alone.

### Downloads

- In `wineo-gog`: bearer token, progress, cancellation, and resume. Each
  partial file is stored with its GOG file id, expected size, and checksum.
  Resuming resolves a fresh download link (links expire), sends a Range
  request, and accepts only a 206 with a matching `Content-Range`; a 200
  restarts the file. Every finished file is verified whole: MD5 and size
  when GOG publishes a checksum, size only with a note when it doesn't.
- Multi-part Windows installers keep their original file names together in
  one folder, since the `.exe` finds its `.bin` parts by name.
- In `wineo-core`, before anything downloads: the download cache must have
  room for every installer part and DLC, and the install folder's
  filesystem must have room for the installers' total size with a margin
  (GOG doesn't publish installed sizes). Too little room refuses with the
  sizes. Installers are deleted after a successful install and kept after a
  failure for the retry.

### Installing

- One core create-and-install function, used by the CLI and the desktop
  app, replacing today's two copies.
- **Order:** download everything, then create the environment and write the
  state as `installing`, then install, then mark it `installed`. A crash,
  cancel, or failure leaves `incomplete`, which refuses launch (question
  14) and is finished or retried by `wineo update NAME`.
- **Windows:** build the prefix, then run the Inno installer with
  `/SP- /SILENT /SUPPRESSMSGBOXES /NORESTART /NOCANCEL` and
  `/DIR="Z:\..."`, the install folder as a fully qualified Wine path.
  `/LANG=` takes Inno's internal language name, from a table mapping GOG's
  language codes to Inno's names, and is left out for English. A Windows
  install needs a graphical session; without one it refuses before
  downloading. If the silent run exits with Inno's codes 1, 3, or 7 (setup
  never started changing files), wineo offers the installer's own wizard
  once. Codes 2 and 5 are cancellations, 4 is a failure partway, and
  neither retries.
- The default target comes from `goggame-<id>.info`'s primary file play
  task (path, working dir, arguments). A game with only a URL task, or no
  info file, gets no default target and the program list, like `new`.
- **Native:** extract the game files from the `.sh`'s zip (`data/noarch`)
  without running the MojoSetup installer. Entries with absolute paths or
  `..` are refused, executable bits are kept, symlinks must point inside
  the folder, and a fresh install into a non-empty folder is refused. Then
  run `support/postinst.sh` when present, from the game folder, on install
  and on update, cancellable, with a failure leaving `incomplete`. The
  target is `start.sh`.
- **DLC:** each owned DLC installs after the game from its own installer,
  in the chosen language, falling back to English, else marked
  `unavailable` with a note (for example, no Linux installer). A DLC failure
  after a finished base game marks that DLC `failed` and the game stays
  launchable.
- Replacing a prefix keeps the game folder, but the installer's registry
  entries and redistributables don't come back on their own.
  `wineo update NAME --reinstall` reruns the installed version's installer.

### Updating

- **Eligibility.** A game on the installer track is compared with GOG's
  current installer version for its platform and language. A string that
  differs means an update. A game on the Galaxy track is compared by build
  only, and isn't updated until 0050 (question 12). When wineo can't tell,
  for example a missing version or a changed language, it reports
  **Can't compare** instead of offering an update.
- DLC is compared per DLC the same way, so a DLC-only update is found and
  applied alone.
- **Applying:** the state goes to `updating` with the target version before
  any file changes. The newer installer (or GOG's patch installer when it
  starts from the installed version exactly) runs over the game. On
  success the new version is recorded and the state returns to
  `installed`; otherwise it's `incomplete`.
- **Native file ownership:** after extracting the new version, files in the
  old shipped list but not the new one are deleted when unchanged and kept
  and listed when changed; changed files the new version still ships
  follow question 15.
- If the game's own `goggame-<id>.info` shows a different build than
  recorded (another tool updated it), wineo records the file's build.

### Migrating from Heroic

- Reads Heroic's metadata (`gog_store/installed.json`,
  `GamesConfig/<id>.json`) and the game folder's `goggame-<id>.info` or
  `gameinfo`. Heroic's metadata is never edited; wineo's later updates do
  change the adopted game folder and prefix.
- Works without a GOG login: it's all local. Art and playtime from GOG are
  skipped with a note when logged out.
- **Matching an existing environment,** by canonical paths: first an
  environment with the same `gog_id`; then one whose default target's exe
  is the game's primary play task (Windows) or `start.sh` (native) and,
  for Windows, whose prefix is Heroic's prefix for the game. More than one
  match, or a match that already has a different `gog_id`, is refused with
  the names. A matched environment keeps every setting; only `gog_id` and
  the installed state are added.
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

- `wineo import gog --list` shows owned games with state; `--game ID`
  installs or migrates one; `--all` migrates every Heroic GOG game.
- Install options: `--platform linux|windows` (default: Linux when a native
  installer exists), `--language CODE` (default from the global
  `gog.language`, English), `--runner` and `--arch` (defaults from the
  global config, like `new`), `--dir` (install folder).
- Every install and migration shows a review first, frozen like other
  imports, unless `--yes`.
- `wineo update [NAME|--all|--check] [--reinstall]`.
- **Outcomes:** `Already installed.`, `Already migrated.`, `Up to date.`
  are no-ops that exit 0. A batch continues past a failed game and exits 1
  with a summary. Cancel exits 130. `--json` follows
  [JSON output](../json-output.md), and `--dry-run` follows
  [Dry runs](../dry-run.md) and question 16.

### Desktop app

- **Add** gets **Import from GOG**: a poster grid of owned games (GOG's
  images), with installed, Heroic, and update states, platform and
  language choices in the review, and download and install progress.
- Settings gets a **GOG** section with **Log in** (opens the browser, then a
  field for the address) and **Log out**.
- Library cards show **Update available**; the panel and the context menu
  get **Update**. The background check runs once after start when logged
  in.

## Implementation sequence

1. Create `wineo-gog` with the GOG client: login URL, code exchange,
   refresh, the owned list, product details, installer links, and the
   build list, plus its fake server for tests. In `wineo-core`, token
   storage, the credentials lock, renewal rules, and a debug-only
   `WINEO_TEST_GOG_API` override, like Steam's.
2. File download in `wineo-gog`: bearer, progress, cancellation, resume,
   and verification. Space checks in `wineo-core`.
3. Config `gog_id` key and validation, the installed-state file, the
   game-folder lock, and keeping state consistent through `rm`, trash,
   restore, clone, and portable copies.
4. Core create-and-install: Windows installer arguments and exit codes,
   native extraction and `postinst.sh`, targets from GOG metadata, DLC,
   and the incomplete state. Move the CLI and desktop app onto it.
5. Heroic migration, including matching existing environments.
6. Updates: eligibility, applying, native file ownership, DLC-only
   updates, and `--reinstall`.
7. CLI: `import gog` and `update`, with review, batches, JSON, and dry
   runs.
8. Desktop app: Import from GOG, Settings, update badges and actions.
9. Playtime import, GOG art, and docs (`gog.md`, config-files, json-output,
   exit-codes, stability, README), plus a backlog row for copying game
   folders with snapshots, clones, and portable copies.

## Testing and acceptance

Unit and CLI tests run against the fake GOG server and fixture installers:
a `.sh` built like GOG's (a shell header, a MojoSetup stand-in, and a zip),
and a stub Windows installer that reports Inno exit codes. They cover the
failure paths, not just the good ones: an interrupted download resumed
(including a 200 instead of a 206, and an expired link), an interrupted
install and update leaving `incomplete`, a DLC-only update, a failed DLC
after a finished game, obsolete and changed shipped files on a native
update, two environments claiming one folder, account switching and
logout, renewal under contention, and secrets never reaching errors.

The stub can't check real Inno path and language handling, so
`--ignored` tests install a real small GOG game for Windows and for Linux,
with `WINEO_TEST_GOG_LOGIN`.

Acceptance checklist, drafted after implementation, will include: logging
in from the CLI and the desktop app; listing owned games; installing a
small Windows game and a small Linux game; migrating RimWorld (attaching to
its existing environment, keeping `mods-1.5`) and Disco Elysium (its
Heroic prefix and build, shown as Galaxy-tracked); checking for and
applying an update; interrupting an install and finishing it; and logging
out.

## Scope limits

- Not in this plan: cloud saves, bonus content, posting playtime to GOG,
  Mac games, and copying game folders with snapshots, clones, and portable
  copies. Galaxy differential downloads, beta branches, and rollback come
  in plan 0050.
- Epic stays a backlog row.
- wineo never edits Heroic's metadata.
