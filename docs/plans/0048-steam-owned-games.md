Status: draft

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
- `rm` removes only wineo's config and never uninstalls (0047).

On 2026-10-09 the maintainer split queue item 5 into this plan and a direct
GOG plan, and parked Epic as a backlog row.

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
- The API sometimes misses games, mostly free ones, so manual import by app
  id stays possible (`--game` with an id the list doesn't show).
- `steam steam://install/<app_id>` opens Steam's install dialog, starting
  the client if needed. Flatpak Steam takes the same URL through `flatpak
  run com.valvesoftware.Steam`.
- `StateFlags` bits (from OpenSteamworks, matching `steam.rs:35`): 1
  uninstalled, 2 update required, 4 fully installed, 256 update running,
  512 update paused, 1024 update started, 2048 uninstalling, 131072
  validating, 262144 adding files, 524288 preallocating, 1048576
  downloading, 2097152 staging, 4194304 committing. Lutris's list numbers
  them sequentially and is off past 4096. The real sequence during a fresh
  install hasn't been observed yet; step 1 records it.

## Proposed design

### The Steam Web API key

A second secret beside the SteamGridDB key, with the same read order:
`WINEO_STEAM_KEY`, then the keyring (account `steam`), then a 0600
`steam.key` file under wineo's data dir when no keyring is available.

- Generalize the SteamGridDB key helpers (`find_key`, `save_key`,
  `remove_key`) over a small key descriptor (env var, keyring account, file
  name, label) instead of copying them.
- Saving a key checks it online with one `GetOwnedGames` call for the
  active account, like `wineo art key` does.
- Steam takes the key as a query parameter, so it ends up in the URL.
  reqwest errors print their URL, so every Steam API error is stripped with
  `without_url()` and reported as a short reason. A test checks that no
  error text contains the key.
- How you set it is question 2.

### The owned list

- `steam/owned.rs` fetches `GetOwnedGames` for the active account's
  SteamID64 (`account.rs` returns it alongside the account id) and caches
  `app_id` and `name` for each game in
  `$XDG_CACHE_HOME/wineo/steam-owned-<steamid64>.json`, with the fetch time.
- The cache is refreshed when an import listing opens and it's older than
  an hour, or on `--refresh`. A failed refresh keeps the old list with a
  warning.
- Only import listings read it. The desktop app's 2-second library reload
  never touches the owned list or the network. An imported game already
  carries its `title` in its config, so the library doesn't need the list.
- The tools filter from 0047 still applies.
- With no key, listings show installed games only, plus one line: `Add a
  Steam Web API key to see uninstalled games.`
- With several accounts and none marked most recent, listing owned games
  fails with the same error as playtime does today.

### Importing an uninstalled game

How uninstalled games show up and get imported is question 1. In every
option:

- `plan` and `apply` stop refusing not-installed games when the owned list
  has the app id, or when `--game` names an id directly. The written config
  is the same as for an installed game: `kind`, `launcher`, `app_id`,
  `title` (from the owned list), `tags = ["game"]`.
- Art is copied from Steam's library cache when it's there, and `--art`
  fills the rest from SteamGridDB by app id, as in 0047.
- An imported game that isn't installed is `not installed` everywhere it
  already is today. Once Steam installs it, it's playable with no re-import.

### Installing

How install is triggered is question 3.

- Wineo opens `steam://install/<app_id>` detached, through the same
  `__steam_request` handoff `start` uses (`supervisor/steam.rs:39`), so
  Steam is never in wineo's process group.
- It returns once Steam accepts the handoff. It doesn't wait for the
  download.
- Installing an installed game is a no-op: `Already installed.`
- Installing an environment that isn't a Steam game is refused: `Only Steam
  games can be installed.`

### Install progress

New install states beside `installed`, `updating`, and `not installed`,
from the manifest:

| Manifest | State | Display |
| --- | --- | --- |
| no manifest | `not_installed` | `not installed` |
| not fully installed, update running or downloading | `installing` | `installing 42%` |
| not fully installed, update paused | `install_paused` | `install paused 42%` |
| not fully installed, only update required | `install_queued` | `install queued` |
| fully installed plus any updating bit | `updating` | `updating` (unchanged) |

- Percent is `BytesDownloaded / BytesToDownload`, then staging bytes once
  downloading is done, like the launch watcher. No percent when the totals
  are zero.
- The exact flag combinations come from step 1's observation, and the
  table is amended if Steam does something else.
- `status`, `list`, `status --json` (`state` plus a new `progress` number,
  null when unknown), and the desktop app all show it.
- `start` on an installing game is refused: `Steam is still installing
  NAME.`

### Validate

`steam_game_not_installed` is an error today (`validate.rs:258`), which
made sense when every Steam import was installed. Imported but uninstalled
games are normal now, so it becomes a note and `validate` passes. Installing
and paused installs are notes too. This changes `validate`'s exit code for
those environments and goes in the changelog.

### Desktop app

- **Import from Steam** lists uninstalled owned games after installed ones,
  dimmed, with a **Refresh** button. With no key, it shows the same hint as
  the CLI with a link to Settings.
- A not-installed card's panel shows **Install** where Play is. Installing
  cards show a thin progress bar on the poster and the state word in the
  panel, from the existing 2-second reload.
- The panel's hero and logo art are dimmed for not-installed games too,
  matching the card.
- Settings gets a **Steam Web API key** row beside the SteamGridDB one, with
  the same save, check, and remove behavior.

## Questions

🧐 1. How uninstalled games show up and get imported:
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
🧐 2. Where the Steam Web API key is set:
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
🧐 3. How install is triggered:
   - **A (recommended).** A new `wineo install NAME` for Steam games, and an
     **Install** button in the desktop app. Explicit, and free for GOG to
     reuse later.
   - **B.** `wineo start NAME` on an uninstalled game opens Steam's install
     dialog instead of refusing.
   - **C.** A, plus `wineo import steam --game ID --install` to import and
     install in one go.
🧐 4. Uninstalling:
   - **A (recommended).** Not in this plan. Steam's client does it in two
     clicks, and wineo stays a tool that never removes game files. Backlog
     row.
   - **B.** `wineo uninstall NAME` and a desktop button that open
     `steam://uninstall/<app_id>`, Steam's own confirmation dialog.
🧐 5. `validate` on not-installed Steam games:
   - **A (recommended).** A note, so `validate` passes.
   - **B.** A warning: `validate` passes, but warnings show in its summary.
   - **C.** Keep it an error.

## Implementation sequence

1. Observe a real install (maintainer, with a primary-written watcher
   script): record `StateFlags` and byte counts every second while Steam
   installs, pauses, resumes, and finishes a small free game. Amend the
   install state table.
2. Core: key descriptor and generalized key helpers, SteamID64 from the
   account, the Steam API request path with redacted errors, `GetOwnedGames`
   and the owned cache. (`coder`)
3. Core: import of uninstalled games, install states and progress in
   reports and JSON, the install handoff, and the validate level. (`coder`)
4. CLI: listing and flags from question 1, key commands from question 2,
   install from question 3, status and list output. (`coder`)
5. Desktop app: import list, Install button, progress bar, panel dimming,
   and the Settings key row. (`coder-opus`)
6. Docs: `steam.md`, `json-output.md`, `validate.md`, `gui.md`, `art.md`
   (keys), README, and the acceptance checklist. (primary)

Each step is reviewed by `qa-gpt` before it lands.

## Testing and acceptance

- Unit tests: key read order and file fallback for the Steam key, error
  redaction (no key in any error string), owned cache freshness and stale
  fallback, install state mapping for each flag combination from step 1,
  progress math with zero totals.
- A local HTTP fixture for `GetOwnedGames`: success, 401, 403, empty
  `response`, and malformed JSON.
- CLI integration tests with a fake Steam root: listing with and without a
  key, importing an uninstalled game, installing (the handoff records the
  URL instead of running Steam), and status during a fake install.
- The acceptance checklist uses throwaway `XDG_*` dirs, a real Steam
  account and key, and a small free game to install.

## Scope limits

- Uninstalling, unless question 4 picks B.
- Family Sharing games and other accounts' libraries.
- DLC lists and per-DLC install.
- Choosing the install library from wineo. Steam's dialog asks.
- Web login instead of a key.
- GOG: the next plan.
