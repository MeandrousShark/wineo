Status: approved

# 0048: Owned Steam games

## Goal

Let wineo see the Steam games you own but haven't installed, import them
like installed ones, and hand the install to Steam. Once Steam is
installing, wineo shows the progress, and the game becomes playable without
a re-import. First half of queue item 5 in [features.md](../features.md);
GOG direct is the second half and its own plan.

## Settled by the maintainer

From the "Store library import" backlog row (2026-10-02) and
[0047](0047-steam-library.md):

- The owned list is cached and refreshed. Installed games are scanned live.
- Uninstalled games show grayscaled and dimmed in the desktop app.
- Install goes through Steam's own dialog (`steam://install/<app_id>`), and
  wineo watches the manifest's state and downloaded bytes for progress.
- Steam games are proxied environments with one config each, from an
  explicit `wineo import steam` (0047 question 1).
- `rm` removes only wineo's config and never uninstalls (0047). Uninstalling
  is its own command (question 4).

On 2026-10-09 the maintainer split queue item 5 into this plan and a direct
GOG plan, and parked Epic as a backlog row.

The same day the maintainer answered the questions below:

1. One listing: `--list` shows owned games with a state column, `--game`
   imports either kind, `--all` stays installed-only, and `--all --owned`
   takes the whole library (A).
2. The key is set with `wineo import steam --key` and removed with
   `--remove-key` (A).
3. A new `wineo install NAME` and an **Install** button (A).
4. `wineo uninstall NAME` and a desktop button open Steam's own uninstall
   dialog (B).
5. A not-installed Steam game doesn't fail `validate` (A).

A review on 2026-10-09 found gaps, and the maintainer took every fix and
these answers the same day:

- When the owned list can't be fetched, listings fall back to installed
  games, and an empty `response` from Steam never replaces the cache.
- Install and uninstall have defined outcomes for unreadable libraries,
  partial installs, and pending launches, and run under the game's lock.
- `validate` has no note level, so a not-installed Steam game is simply not
  reported, rather than adding one.

Also on 2026-10-09 the maintainer added `-f`/`--follow` to `install` and
`uninstall`, which hands off to Steam and then shows progress until it's
done. The name matches `logs -f`.

## Existing foundation

- `steam.rs` scans every library's `appmanifest_*.acf` and parses
  `StateFlags`, `BytesToDownload`, `BytesDownloaded`, `BytesToStage`, and
  `BytesStaged` into `Manifest` (`steam.rs:188`). `state_from_flags`
  (`steam.rs:329`) already maps flags to installed, updating, and not
  installed, and the launch watcher reads staging progress
  (`supervisor/steam.rs:275`).
- A proxied environment whose game has no manifest is already a working
  state: `report.rs:112` reports `not_installed`, launch refuses it
  (`launch.rs:962`), and the desktop app dims its card with Play disabled
  (`grid.rs:32`, `library.rs:114`).
- `steam/account.rs:26` finds the active account from `loginusers.vdf`, so
  wineo knows the SteamID64 without asking. It returns only the 32-bit
  account id today.
- Steam caches art for the whole library, not just installed games. The
  maintainer's machine has 452 folders in `appcache/librarycache` against 13
  installed games, so most uninstalled imports get art with no SteamGridDB
  key.
- The keyring wrapper is generic (`secret_service.rs:23`, `for_account`),
  but the env var, file fallback, and save/remove helpers around it are
  SteamGridDB-specific (`steamgriddb.rs:88-189`).
- HTTP goes through reqwest's blocking client with the `wineo/<version>`
  User-Agent (`download.rs:253`). `Transport::request` only knows bearer
  tokens (`download.rs:190`), and nothing caches API responses on disk.

### What Steam offers

Checked on 2026-10-09:

- `IPlayerService/GetOwnedGames/v1` with `key`, `steamid`,
  `include_appinfo=1` (names), `include_played_free_games=1`, and
  `include_free_sub=1` returns the owned games. A key linked to the account
  sees its own library even when the profile is private.
- There is no keyless route anymore. The public `games?xml=1` profile page
  now redirects to a login screen, and `GetOwnedGames` without a key
  returns 401. Playnite's other route, logging in through an embedded
  browser, doesn't fit a CLI.
- Keys come from `https://steamcommunity.com/dev/apikey`. Steam may refuse
  a key to accounts without a purchase or without Steam Guard.
- The API sometimes misses games, mostly free ones. Those can still be
  imported once Steam installs them, since installed games come from the
  local scan.
- `steam steam://install/<app_id>` opens Steam's install dialog, starting
  the client if needed. Flatpak Steam takes the same URL through `flatpak
  run com.valvesoftware.Steam`.
- `StateFlags` bits (from OpenSteamworks, matching `steam.rs:35`): 1
  uninstalled, 2 update required, 4 fully installed, 256 update running,
  512 update paused, 1024 update started, 2048 uninstalling, 131072
  validating, 262144 adding files, 524288 preallocating, 1048576
  downloading, 2097152 staging, 4194304 committing. Lutris's list numbers
  them sequentially and is off past 4096. Only a few of these reach the
  manifest on disk; see step 1 under "Decisions during implementation".

## Proposed design

### The Steam Web API key

A second secret beside the SteamGridDB key, with the same read order:
`WINEO_STEAM_KEY`, then the keyring (account `steam`), then a 0600
`steam.key` file under wineo's data dir when no keyring is available.

- Generalize the SteamGridDB key helpers (`find_key`, `save_key`,
  `remove_key`) over a small key descriptor (env var, keyring account, file
  name, label) instead of copying them.
- Saving a key checks it online with one `GetOwnedGames` call for the
  active account. Unlike SteamGridDB's status-only check
  (`steamgriddb.rs:219`), a 200 isn't enough: the response must carry
  `game_count`, since Steam answers an empty `response` when the key can't
  see the library.
- Steam takes the key as a query parameter, so it ends up in the URL, and
  several layers put the URL in errors: reqwest's own errors,
  `Transport::request`'s `request failed: {url}` context
  (`download.rs:198`), and the default transport's refusal
  (`download.rs:158`). Steam API calls get their own request path whose
  errors are built without the URL at every layer, including body reads
  and JSON parsing, rather than scrubbed afterward. Tests format each
  failure through the production transport with `{error:#}` and check that
  the key appears nowhere.
- `wineo import steam --key` prompts for it and checks it, and
  `--remove-key` removes it (question 2).

### The owned list

- `steam/owned.rs` fetches `GetOwnedGames` for the active account's
  SteamID64 (`account.rs` returns it alongside the account id) and caches
  `app_id` and `name` for each game in
  `$XDG_CACHE_HOME/wineo/steam-owned-<steamid64>.json`, with the fetch time.
- Each command that needs it loads it once and passes it to `plan` and
  `apply`: a listing, a direct `--game` import of a game that isn't
  installed, and `--all --owned`, which makes one fetch for the whole run.
  The cache is used when it's under an hour old; otherwise wineo fetches,
  or on `--refresh`.
- Installed games never need it. Importing one works with no key and no
  network, as in 0047, and `apply`'s recheck under the app lock
  (`steam_import.rs:188`) only consults the owned list when the game still
  isn't installed.
- When a fetch fails:
  - With a cache, the old list is used, with `Couldn't refresh owned
    games: REASON`.
  - Without one, listings show installed games only, with `Couldn't get
    owned games: REASON`. A direct import of an uninstalled game fails with
    the same message.
  - An empty `response` (no `game_count`) means the key can't see the
    library. It's a failure: the cache is kept, never replaced.
  - `game_count: 0` is a real, empty library and replaces the cache.
- Only import commands read it. The desktop app's 2-second library reload
  never touches the owned list or the network. An imported game already
  carries its `title` in its config, so the library doesn't need the list.
- The tools filter from 0047 still applies.
- With no key, listings show installed games only, plus one line: `Add a
  Steam Web API key to see uninstalled games.`
- With several accounts and none marked most recent, listing owned games
  fails with the same error as playtime does today.

### Importing an uninstalled game

From question 1: `--list` shows every owned game with a state column,
`--game` imports either kind, `--all` stays installed-only, and `--all
--owned` takes the whole library. Also:

- `plan` and `apply` stop refusing not-installed games when the owned list
  has the app id. An id that is neither installed nor owned is refused:
  `App 12345 isn't installed or owned.` The written config
  is the same as for an installed game: `kind`, `launcher`, `app_id`,
  `title` (from the owned list), `tags = ["game"]`.
- Art is copied from Steam's library cache when it's there, and `--art`
  fills the rest from SteamGridDB by app id, as in 0047.
- An imported game that isn't installed is `not installed` everywhere it
  already is today. Once Steam installs it, it's playable with no re-import.

### Installing

From question 3: `wineo install NAME`, and **Install** in the desktop app.

- Wineo opens `steam://install/<app_id>` detached, through the same
  `__steam_request` handoff `start` uses (`supervisor/steam.rs:39`), so
  Steam is never in wineo's process group.
- It returns once Steam accepts the handoff. It doesn't wait for the
  download unless `-f` is given.
- Installing an installed game is a no-op: `Already installed.`
- A queued install opens Steam's dialog anyway. An installing game is a
  no-op: `Steam is already installing NAME.` A paused install looks the
  same on disk, so it gets the same answer, and it's resumed in Steam.
- An unreadable library or unknown state is refused with the reason, as
  `start` does today (`steam.rs:442`), since the game may be there.
- Installing an environment that isn't a Steam game is refused: `Only Steam
  games can be installed.`
- The check and the handoff run under the game's `steam-<app_id>.lock`, the
  one import takes, so install, uninstall, and import can't race each other.
  `start` doesn't take it; see step 3 under "Decisions during
  implementation".

### Following

`wineo install NAME -f` and `wineo uninstall NAME -f` hand off to Steam the
same way, then stay attached and show progress from the manifest:

```text
$ wineo install witcher3 -f
Waiting for Steam's install dialog...
Installing The Witcher 3... 42% (14.2 GB of 33.8 GB)
Installed.

$ wineo uninstall witcher3 -f
Uninstalling The Witcher 3...
Uninstalled.
```

- The progress line redraws in place on a terminal and prints one line per
  state change otherwise, so logs stay readable.
- Before Steam writes a manifest, it waits as long as the dialog is open,
  with no timeout, since the install dialog can sit for a while. Wineo
  can't tell a canceled dialog from an open one, so Ctrl+C is the way out.
- Ctrl+C stops following, never the install, and exits 130 like other waits
  (`docs/exit-codes.md`).
- It exits 0 once the game is installed (or the manifest is gone for
  uninstall), and 1 if Steam stops partway: the manifest disappears during
  an install (`Steam canceled the install.`) or comes back fully installed
  during an uninstall (`Steam canceled the uninstall.`). A paused install
  keeps following, with no percent, until Steam resumes it.
- A no-op (`Already installed.`, `Not installed.`) exits 0 right away.
- Following uses the same manifest reader as `status`, polled once a
  second, and takes no lock while it waits.

### Uninstalling

From question 4.

- `wineo uninstall NAME` opens `steam://uninstall/<app_id>` through the same
  handoff and returns. Steam's dialog asks for confirmation, so wineo
  doesn't ask again. Step 1 confirmed it asks.
- The config stays, and the game goes back to `not installed`.
- A game that is `starting` or running is refused, including one started
  from the Steam client (found by the reaper scan): `Stop NAME before
  uninstalling it.`
- A queued or installing partial install (paused ones included) is
  allowed. Steam's dialog
  cancels it.
- An unreadable library or unknown state is refused with the reason.
- A game that isn't installed is a no-op: `Not installed.` A game that
  isn't a Steam game is refused: `Only Steam games can be uninstalled.`
- Like install, the check and handoff run under `steam-<app_id>.lock`.

### Install progress

New install states beside `installed`, `updating`, and `not installed`,
from the manifest:

| Manifest | `install` | Display |
| --- | --- | --- |
| no manifest | `not_installed` | `not installed` |
| not fully installed, update started (1024) or another updating bit | `installing` | `installing 42%` |
| not fully installed, only update required (2) | `install_queued` | `install queued` |
| fully installed plus any updating bit | `updating` | `updating` (unchanged) |
| uninstalling (2048), if Steam ever writes it | `uninstalling` | `uninstalling` |

- Percent is `BytesDownloaded / BytesToDownload`, then staging bytes once
  downloading is done, like the launch watcher. No percent when the totals
  are zero.
- Step 1 found no paused state on disk: a paused install keeps `1026` and
  its byte counts drop to zero, the same as an install that hasn't started
  downloading. So a paused install shows as `installing` with no percent.
- Steam rewrites the byte counts only now and then, so the percent moves
  in jumps.
- These are new values of the existing `install` field, not of `state`.
  `state` stays the run status (`view.rs:78`, 0047), so a script checking
  `state` sees no change, and a game can be idle and installing at once.
- `status`, `list`, `status --json`, `list --json`, and the desktop app all
  show it. The JSON gains `install_progress`, an integer percent from 0 to
  100, `null` when there's no percent or the game isn't installing.
- `start` on an installing or uninstalling game is refused: `Steam is
  still installing NAME.` or `Steam is uninstalling NAME.`

### Validate

`steam_game_not_installed` is an error today (`validate.rs:258`), which
made sense when every Steam import was installed. Imported but uninstalled
games are normal now, and `validate` has only errors and warnings
(`validate.rs:46`), so the check is dropped: not installed, installing,
queued, and uninstalling are not reported. `status` and the desktop
app already show them. Unreadable libraries and unknown state keep their
existing checks. The `steam_game_not_installed` code goes away, which
changes `validate`'s exit code and JSON for those environments and goes in
the changelog.

### Desktop app

- **Import from Steam** lists uninstalled owned games after installed ones,
  dimmed, with a **Refresh** button. With no key, it shows the same hint as
  the CLI with a link to Settings.
- A not-installed card's panel shows **Install** where Play is. Installing
  cards show a thin progress bar on the poster and the state word in the
  panel, from the existing 2-second reload.
- Installed Steam games get **Uninstall** in the context menu, which opens
  Steam's dialog.
- The panel's hero and logo art are dimmed for not-installed games too,
  matching the card.
- Settings gets a **Steam Web API key** row beside the SteamGridDB one, with
  the same save, check, and remove behavior.

## Decisions during implementation

- Step 1 (2026-10-09, maintainer): a real install and uninstall of Cookie
  Clicker (1454400) and Fields of Mistria (2142790), installed from
  Steam's UI and watched once a second:
  - The manifest appears as soon as Install is pressed, with `StateFlags`
    `2`. It changes to `1026` (update required, update started) and keeps
    it for the whole download, then turns `4`. Downloading, staging, and
    update running never showed up on disk.
  - Byte totals appear a few seconds into `1026`. `BytesDownloaded` was
    rewritten about once per download (`187MB/306MB`), and staged bytes
    moved along with it.
  - A pause and resume (as the maintainer remembers it) kept `1026` and
    dropped both byte counts to `0/0` for about two seconds. `512` never
    appeared, so `install_paused` was dropped from the table.
  - Uninstalling removed the manifest within a second, with no `2048`.
  - `steam steam://install/1454400` opens Steam's install dialog, and
    `steam steam://uninstall/1454400` opens its uninstall confirmation.
- Step 3 (2026-10-09):
  - The plan said launch takes `steam-<app_id>.lock`. It doesn't: only
    import did. Install and uninstall take it (renamed `Locks::steam_app`),
    so they serialize with each other and with import. A `wineo start`
    racing an uninstall is guarded only by uninstall's status check
    (starting, running, or a reaper). The maintainer accepted that window
    rather than changing 0047's launch path: wineo only opens Steam's
    dialog, Steam asks for confirmation, and Steam arbitrates.
  - Install while Steam is uninstalling is refused: `Steam is uninstalling
    NAME.` Uninstall while uninstalling opens the dialog again.
  - Unreadable libraries and unknown state refuse both actions with
    `Steam's library at PATH isn't available.` or the reason.
  - Import counts a manifest as installed whenever 0047's
    `state_from_flags` does, so a game mid-install (`1026`) imports as
    before. Flags `2`, `0`, `1`, and `2048` fall through to the owned list.
  - Without owned data (`OwnedGames::Unloaded`), import keeps 0047's
    `Steam lists N as not installed.` Owned games that aren't installed
    are listed after installed ones, by title.
  - `start` on a queued game keeps `Steam lists NAME as not installed.`
  - Following: an install is waiting until a queued or installing state
    appears, and a missing manifest after that is `Steam canceled the
    install.` An uninstall keeps waiting while the game is fully installed
    (the dialog is open), and a return to fully installed after any other
    state is `Steam canceled the uninstall.` Unreadable reads keep
    following. There's no debounce on a missing manifest; review found no
    evidence Steam unlinks before replacing one.
  - The `-f` byte pair comes from the same phase as the percent: download
    bytes while downloading, staged bytes once staging.
  - `install_progress` is left out of the JSON for Wine and native
    environments, and `null` for a Steam game with no percent.
- Step 4 (2026-10-09):
  - `install` and `uninstall` without `-f` print `Asked Steam to install
    NAME.` or `Asked Steam to uninstall NAME.`, like `start`.
  - `install -f` on a game Steam is already installing prints `Steam is
    already installing NAME.` and then follows it.
  - Sizes use wineo's usual formatter, so the progress line says `GiB`.
  - `Steam canceled the install.` and `Steam canceled the uninstall.` go to
    stderr without an `error:` prefix and exit 1.
  - `--list` gained a header row (`APP ID  TITLE  SIZE  STATE  IMPORTED`).
  - `--owned` requires `--all`. `--refresh` with plain `--all` or an
    installed `--game` is ignored, since nothing is fetched.
  - `--all --owned` with no key fails with `Add a Steam Web API key to
    import uninstalled games.` `--game` of an uninstalled game with no key
    keeps `Steam lists N as not installed.`
  - With a key, several accounts and none most recent fail with `Steam
    doesn't say which account was used last.` before any request.
  - A dry run checks ownership like the real run (fresh cache, otherwise a
    fetch) but never writes the cache, so `GetOwnedGames` joins the
    SteamGridDB search as a dry run's network use.
  - Debug builds read `WINEO_TEST_STEAM_API` and
    `WINEO_TEST_STEAM_FOLLOW_INTERVAL_MS` so CLI tests can stand in for
    Steam. Release builds compile both out.
- Step 5 (2026-10-09):
  - The CLI's owned-list loading moved into core as
    `steam_import::OwnedData`, so the CLI and the desktop app decide when to
    fetch the same way. CLI output didn't change.
  - Import from Steam shows installed games first and loads the owned list
    after them, so a slow fetch never blocks them.
  - Uninstall from the context menu opens the game's panel, so Steam's
    answer or a refusal shows there. It's offered for installed and updating
    games only; the CLI also allows it mid-install.
  - Installing cards aren't dimmed; not-installed and queued ones are.
  - The app owns one Steam key operation at a time, like SteamGridDB's, so
    its result reaches Settings even after leaving and coming back. Both key
    rows' spinners now animate, which fixed the same miss in SteamGridDB's.
- Acceptance polish (2026-10-09, maintainer). Merged before its last review
  round at the maintainer's request, pending the final qa pass after
  acceptance:
  - Import pages (Heroic and Steam share the code) sit inside the dialog
    border with 24 px padding, and their action rows wrap instead of
    squeezing a button to nothing. The artwork editor and the New flow's
    Environment row wrap the same way in narrow dialogs.
  - Poster titles on import tiles and library cards wrap to two balanced
    lines, measured with the rendered font, so a short word isn't left
    alone on line 2. A title that still doesn't fit ends with `...`; on
    120 px import tiles that includes long ones like Grand Theft Auto IV,
    which the maintainer accepted.
  - The import poster focus ring shows only after keyboard navigation, as
    a primary ring around the poster, not after clicks.
  - Lazy art loading uses real card heights instead of a fixed row pitch.
  - At a dialog height of 280 px or less, the import grid had no room left
    under the header and action row, so no games showed. Window managers
    like Hyprland ignore minimum sizes, so a window can get that short.
    The maintainer settled it on 2026-10-10: the import page's title and
    notes scroll with the grid, and Back and the action row stay pinned
    outside the scroll, like New's review footer. The layout tests' short
    dialog exemption (`heroic::grid_too_short`) goes away. The maintainer
    put the action row at the bottom, under the grid, with Back staying at
    the top. This applies to both the Heroic and Steam grids.
  - Review of that change found the Heroic discard prompt could follow you
    onto the games grid: clicking a source while it showed opened the grid
    with the prompt still up. The maintainer chose to treat a source pick as
    Keep editing, so opening a source now dismisses the prompt.
- Acceptance (2026-10-10, maintainer): items 1 to 16, 18, and 19 pass.
  The maintainer accepted item 17 (`--remove-key`) without running it: item
  15's **Remove key** calls the same core `steam::key::remove_key` against
  the real keyring, and `steam_owned.rs` covers the CLI's flags and output.
  Item 18 passed
  as written, but the maintainer asked for every page to match
  [gui-style.md](../gui-style.md), with the whole app in scope:
  - Text action buttons all use the guide's controls: 32 px high with 8 px
    corners, a filled accent for the main action of a step, and neutral
    for the rest. Before this, the New flow, the import pages, Tools, and
    others used iced's stock buttons or shorter chip-style ones. A disabled
    main action keeps its accent and dims rather than turning neutral. The
    Remove dialog's **Remove** is danger, matching the context menu row
    that opens it.
  - Every `< Back` in the app is now `Back`, like New's.
  - Choice lists (the numbered **Add** choices, New's program list, and
    Heroic's source list) are quiet full-width rows in one grouped surface,
    like the context menu, instead of stock buttons.
  - Every scrollbar uses the library's thin rail (6 px, 4 px thumb, no
    track). The thumb stays visible outside the library, so a short dialog
    shows that there's more below. The library keeps hiding its thumb when
    idle.

## Questions

All five were answered by the maintainer on 2026-10-09; see the settled
list above. The options are kept as a record of what was weighed.

1. How uninstalled games show up and get imported:
   - **A (recommended).** In the same listing, marked. `--list` shows
     every owned game with a state column. `--game` imports either kind.
     `--all` still means installed games only, and `--all --owned` takes
     the whole library.
   - **B.** A separate switch. `--list` stays installed-only, and
     `--owned` switches `--list` and `--all` to the uninstalled games.

   ```text
   A: wineo import steam --list
   APP ID   TITLE                 STATE          IMPORTED
   1086940  Baldur's Gate 3       installed      bg3
   526870   Satisfactory          installed
   292030   The Witcher 3         not installed
   wineo import steam --all            # installed only, as today
   wineo import steam --all --owned    # installed and uninstalled

   B: wineo import steam --list --owned
   APP ID   TITLE                 IMPORTED
   292030   The Witcher 3
   wineo import steam --all            # installed only
   wineo import steam --all --owned    # uninstalled only
   ```

   A keeps one list to scan and makes `--all` safe by default, since a
   whole library can be hundreds of games.
2. Where the Steam Web API key is set:
   - **A (recommended).** On the import command: `wineo import steam
     --key` prompts and checks it, `--remove-key` removes it. It sits where
     it's used, like Steam's own settings.
   - **B.** A new `wineo steam key [--remove]` command, which would later
     hold other Steam-only commands.
   - **C.** Generalize `wineo art key` into `wineo key steamgriddb` and
     `wineo key steam`, keeping `art key` as an alias.

   ```text
   A: wineo import steam --key
   B: wineo steam key
   C: wineo key steam
   ```
3. How install is triggered:
   - **A (recommended).** A new `wineo install NAME` for Steam games, and an
     **Install** button in the desktop app. Explicit, and free for GOG to
     reuse later.
   - **B.** `wineo start NAME` on an uninstalled game opens Steam's install
     dialog instead of refusing.
   - **C.** A, plus `wineo import steam --game ID --install` to import and
     install in one go.
4. Uninstalling:
   - **A (recommended).** Not in this plan. Steam's client does it in two
     clicks, and wineo stays a tool that never removes game files. Backlog
     row.
   - **B.** `wineo uninstall NAME` and a desktop button that open
     `steam://uninstall/<app_id>`, Steam's own confirmation dialog.
5. `validate` on not-installed Steam games:
   - **A (recommended).** A note, so `validate` passes. Validate turned out
     to have no note level, so the review settled on not reporting it.
   - **B.** A warning: `validate` passes, but warnings show in its summary.
   - **C.** Keep it an error.

## Implementation sequence

1. Observe a real install and uninstall (maintainer, with a primary-written
   watcher script): record `StateFlags` and byte counts every second while
   Steam installs, pauses, resumes, finishes, and uninstalls a small free
   game, and check that `steam://uninstall` shows a confirmation. Amend the
   install state table.
2. Core: key descriptor and generalized key helpers, SteamID64 from the
   account, the Steam API request path with redacted errors, `GetOwnedGames`
   and the owned cache. (`coder`)
3. Core: import of uninstalled games, install states and progress in
   reports and JSON, the install and uninstall handoffs with their
   admission rules, and dropping the not-installed validate check.
   (`coder`)
4. CLI: `--list` state column and `--all --owned`, `--key` and
   `--remove-key`, `install` and `uninstall` with `-f`, and status and list
   output.
   (`coder`)
5. Desktop app: import list, Install button, Uninstall menu item, progress
   bar, panel dimming, and the Settings key row. (`coder-opus`)
6. Docs: `steam.md`, `json-output.md`, `validate.md`, `gui.md`, `art.md`
   (keys), README, and the acceptance checklist. (primary)

Each step is reviewed by `qa-gpt` before it lands.

## Testing and acceptance

- Unit tests: key read order and file fallback for the Steam key, error
  redaction (no key in any error string), owned cache freshness and stale
  fallback, install state mapping for each flag combination from step 1,
  progress math with zero totals.
- Owned-list failures: fetch fails with and without a cache, empty
  `response` keeps the cache, `game_count: 0` replaces it, a key save
  refused on an empty `response`, and `--all --owned` fetching once.
- Install and uninstall admission for each state: installed, not installed,
  queued, installing, uninstalling, unavailable, unknown, starting,
  running from wineo, and running from the Steam client.
- `-f` against a fake manifest that steps through states: install to
  finished, byte counts dropping to zero mid-install (a pause), the
  manifest vanishing mid-install, an
  uninstall finishing and one coming back, non-terminal output, and Ctrl+C
  exiting 130 while the fake install carries on.
- `validate` passes on a not-installed Steam game and still fails on an
  unrelated error in the same run, in text and JSON.
- `status --json` and `list --json` keep `state` as run status while
  `install` and `install_progress` change.
- A local HTTP fixture for `GetOwnedGames`: success, 401, 403, empty
  `response`, `game_count: 0`, and malformed JSON.
- CLI integration tests with a fake Steam root: listing with and without a
  key, importing an uninstalled game, installing and uninstalling (the
  handoff records the URL instead of running Steam), refusing to uninstall
  a running game, and status during a fake install.
- The acceptance checklist uses throwaway `XDG_*` dirs, a real Steam
  account and key, and a small free game to install.

## Acceptance checklist

Like 0047, wineo gets throwaway XDG dirs while Steam is found through your
real home. In fish:

```fish
set -x XDG_CONFIG_HOME /tmp/acc-0048/config
set -x XDG_DATA_HOME /tmp/acc-0048/data
set -x XDG_STATE_HOME /tmp/acc-0048/state
set -x XDG_CACHE_HOME /tmp/acc-0048/cache
```

Run `wineo-gui` from the same shell when a step uses the app. The XDG dirs
don't isolate the keyring, so steps 2, 15, and 17 save and remove your real
Steam key; save it again afterward if you want to keep it. You need a
Steam Web API key from <https://steamcommunity.com/dev/apikey>, and two
small games you own but haven't installed, called GAME1 and GAME2 below
(Cookie Clicker, 1454400, is free and about 100 MB).

1. `wineo import steam --list` shows installed games only, under a header
   row, and ends with `Add a Steam Web API key to see uninstalled games.`
2. `wineo import steam --key` with a made-up key: `Couldn't check the key:
   ...`, and nothing is saved. Then with your real key: `Saved the Steam Web
   API key to the keyring`. The key is never echoed.
3. `wineo import steam --list` now lists uninstalled games after installed
   ones, with `-` for size and `not installed`, and GAME1 is there.
   `ls /tmp/acc-0048/cache/wineo/` has `steam-owned-<id>.json`.
4. `stat -c %Y` that file, then `wineo import steam --game GAME1 --dry-run`:
   the review shows `state     not installed` and `Dry run, nothing was
   imported.`, and the file's time hasn't changed.
5. `wineo import steam --game GAME1 -n acc1`: `Imported acc1.`.
   `wineo list` shows `acc1  not installed`, `wineo start acc1` refuses with
   `Steam lists acc1 as not installed.`, and `wineo validate` reports
   nothing for acc1.
6. `wineo install acc1 -f`: Steam's install dialog opens, and the terminal
   shows `Waiting for Steam's install dialog...`, then `Installing ...` with
   a percent once Steam knows the size, then `Installed.`; `echo $status` is
   0. While it installs, `wineo status acc1 --json` in another terminal (same
   variables) has `"state":"inactive"`, `"install":"installing"`, and
   `install_progress` as a number or null.
7. `wineo start acc1`, then `wineo uninstall acc1`: refused with `Stop acc1
   before uninstalling it.` Stop the game.
8. `wineo uninstall acc1 -f`: Steam asks to confirm, then `Uninstalled.`
   and exit 0. `wineo list` still has `acc1`, now `not installed`.
9. `wineo install acc1 -f`, and press Ctrl+C while it's installing: exit
   130, and Steam keeps installing. Run `wineo install acc1` again once it's
   done: `Already installed.`
10. `wineo import steam --all --dry-run` reviews only installed games, and
    `wineo import steam --all --owned --dry-run` adds uninstalled ones.
11. `WINEO_STEAM_KEY=bad wineo import steam --list --refresh`: warns
    `Couldn't refresh owned games: ...` and still lists uninstalled games
    from the saved list.
12. In the app, **Add**, **Import from Steam**: installed games appear, then
    **Getting owned games...**, then uninstalled games dimmed after them.
    **Refresh** works. Import GAME2 as `acc2`.
13. `acc2`'s card is grayed out and its panel has **Install** instead of
    Play, with dimmed hero and logo art. **Install** opens Steam's dialog
    and the panel says `Asked Steam to install acc2.`. While Steam installs,
    the card has a progress bar and the panel's State says `installing N%`.
    Once it's done, Play works.
14. Right-click `acc2`, **Uninstall**: its panel opens and Steam asks to
    confirm. Afterwards the card is grayed out again.
15. **Settings**, **Steam Web API**: it says the key is saved in the
    keyring. **Remove key**, then **Add**, **Import from Steam** shows the
    hint and **Open Settings**, which goes to the key row. **Set key...**
    with your key, leave Settings while it checks, and come back: the result
    is shown.
16. Shrink the window: the import page's Back and review buttons stay
    reachable, and Settings scrolls to the Steam row. Check light and dark
    themes if you use both.
17. `wineo import steam --remove-key`: `Removed the Steam Web API key`.
18. Import page polish, on **Import from Steam** and **Import from Heroic**:
    Back sits inside the dialog's corner with even margins; long titles wrap
    to two balanced lines (`Sid Meier's` / `Civilization VI`); clicking a
    poster shows no white box, and arrow keys bring back a ring around the
    focused poster. In a narrow window the action row wraps, and the New
    flow's details page puts Edit on its own line.
19. Library: long titles wrap to two lines with the state and playtime still
    under them, and art keeps loading while you scroll far down a big
    library.

Afterward, uninstall the test games in Steam if you don't want them, and
`rm -r /tmp/acc-0048`.

## Scope limits

- Family Sharing games and other accounts' libraries.
- DLC lists and per-DLC install.
- Choosing the install library from wineo. Steam's dialog asks.
- Web login instead of a key.
- GOG: the next plan.
