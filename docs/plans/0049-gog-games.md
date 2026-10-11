Status: approved

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
  a new `gog_id` key like `heroic_id`. wineo owns the files, so snapshots,
  winetricks, clone, and portable copies all work, and wineo can update it.
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

## Proposed design

Written for the answers above.

### The `wineo-gog` crate

- A new workspace crate, `crates/wineo-gog`, `publish = false`, inheriting
  the workspace lints and `unsafe_code = "forbid"`. It holds the GOG API:
  login URL, code exchange, refresh, the owned list, product details,
  installer links, the build list, and the file download (bearer token, MD5,
  resume, cancellation, progress). Plan 0050 adds manifests, chunks, and
  diffs to it.
- It knows tokens, URLs, and folders, never environments, prefixes,
  configs, or runners. `wineo-core` depends on it and decides where games
  go and what happens in the prefix: running installers, extracting `.sh`
  files, Heroic migration, the installed-state file, and keyring storage.
- Its tests use its own fake GOG server and fixtures, without wineo's
  environment test harness.
- `AGENTS.md`'s code boundaries gain a line for it when it lands.

### Account

- Login stores `{refresh_token, user_id}` as one secret under
  `service=wineo, account=gog`, through `key_store.rs`. The access token
  lives in memory only. A failed refresh means logged out, with
  `Log in to GOG again: wineo import gog --login.`
- The secret and the access token never appear in output, errors, or
  logs, and travel only in POST bodies or `Authorization` headers where
  GOG allows it. GOG's token endpoint takes them as query parameters, so
  those requests are never logged.

### Library

- The owned list and product details are cached like Steam's owned list,
  under the cache directory (not contract), fresh for an hour, with
  `--refresh`.
- Installed state per environment lives in `gog/<environment>.toml` under
  the data directory: GOG id, platform, version string, build id (Windows,
  when known), language, installed DLC ids. `rm`, `mv`, `clone`, and
  portable copies carry or drop it like `art/<environment>/`.

### Installing

- A generic file download in core: bearer token, MD5 or SHA verification,
  resume for multi-GB files, a free-space check before starting,
  cancellation, and progress. Installers download to a cache folder and are
  deleted after a successful install.
- **Windows:** a core create-and-install function used by the CLI and the
  desktop app (replacing today's two copies): build the prefix, run the Inno
  installer with `/SP- /SILENT /SUPPRESSMSGBOXES /NORESTART /DIR=<games
  folder> /LANG=<language>`, and fall back to the installer's wizard if the
  silent run fails, the way Minigalaxy does. The default target comes from
  `goggame-<id>.info`'s primary play task.
- **Native:** extract `data/noarch` into `~/Games/<title>`, run
  `support/postinst.sh` when present, and target `start.sh`.
- Owned DLC installs after the game, from its own installers.

### Updating

- `wineo update` compares installed state against GOG: the build list for
  Windows games, the installer version for Linux. An update reruns the
  newer installer over the game (or its patch installer when GOG offers one
  from the installed version), then records the new version.
- A running game is refused: `Stop disco-elysium before updating it.`
- If the game's own `goggame-<id>.info` shows a different build than
  wineo recorded (another tool updated it), wineo trusts the file.

### Migrating from Heroic

- Read `gog_store/installed.json` and `GamesConfig/<id>.json` (read-only),
  plus the game folder's `goggame-<id>.info` or `gameinfo`.
- Windows: register the Heroic prefix with `--adopt` semantics and the
  runner Heroic used (the same mapping Heroic import uses today). Native:
  a native environment targeting `start.sh`.
- An existing wineo environment whose target is inside the game folder
  gets the GOG id attached instead of a new environment.

### Desktop app

- **Add** gets **Import from GOG**: a poster grid of owned games (GOG's
  images), with installed, Heroic, and update states. Selecting games and
  pressing **Install** or **Import** runs the same flows as the CLI, with
  download and install progress.
- Settings gets a **GOG** section with **Log in** (opens the browser, then a
  field for the address) and **Log out**.
- Library cards show **Update available**; the panel and the context menu
  get **Update**.

## Implementation sequence

1. Create `wineo-gog` with the GOG client: login URL, code exchange,
   refresh, the owned list, product details, installer links, and the
   build list, plus its fake server for tests. In `wineo-core`, token
   storage and a debug-only `WINEO_TEST_GOG_API` override, like Steam's.
2. File download in `wineo-gog`: bearer, MD5, resume, cancellation, and
   progress. The free-space check before an install lives in `wineo-core`,
   which knows the destination.
3. Config `gog_id` key and validation, the installed-state file, and
   carrying it through `rm`, `mv`, `clone`, and portable copies.
4. Core create-and-install with installer arguments (Windows), extraction
   (native), targets from GOG metadata, and DLC. Move the CLI and desktop
   app onto it.
5. Heroic migration, including attaching to an existing environment.
6. Updates: checking, applying, and the running-game refusal.
7. CLI: `import gog` (`--login`, `--logout`, `--list`, `--game`, `--all`,
   `--refresh`, `--dry-run`, `--json`) and `update`.
8. Desktop app: Import from GOG, Settings, update badges and actions.
9. Playtime import, GOG art, validate checks (logged out, missing files),
   and docs (`gog.md`, config-files, json-output, exit-codes, stability,
   README).

## Testing and acceptance

Unit and CLI tests run against the fake GOG server and fixture installers
(a tiny Inno-style stub and a tiny `.sh` with a zip). Real GOG calls are
`--ignored` tests that need `WINEO_TEST_GOG_LOGIN`.

Acceptance checklist, drafted after implementation, will include: logging
in from the CLI and the desktop app; listing owned games; installing a
small Windows game and a small Linux game; migrating RimWorld (attaching to
its existing environment, keeping `mods-1.5`) and Disco Elysium (its
Heroic prefix and build); checking for and applying an update; and logging
out.

## Scope limits

- Not in this plan: cloud saves, bonus content, posting playtime to GOG,
  and Mac games. Galaxy differential downloads, beta branches, and
  rollback come in plan 0050.
- Epic stays a backlog row.
- wineo never edits Heroic's files.
