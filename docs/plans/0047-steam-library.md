Status: draft

# 0047: Steam installed games

## Goal

Bring games installed through Steam into wineo, so they sit in the same
library as everything else: tagged, counted in `wineo stats`, started and
stopped from the CLI or the desktop app, with hooks around them. Steam keeps
owning the game, its prefix, and its process. Fourth in the queue in
[features.md](../features.md), and the first plan to add the proxied
environment kind.

Owned but uninstalled Steam games, and GOG and Epic through Heroic, are queue
item 5 and a later plan.

## Settled by the maintainer

From the "Store library import" backlog row (2026-10-02):

- Store games are a new proxied environment kind, so wineo overrides apply
  only where the store allows. Snapshots, winetricks, `exec`, clone, and
  portable copies don't apply, since the store owns the prefix.
- Installed games are scanned live. The owned list (item 5) is cached and
  refreshed.
- Uninstalled games show grayscaled and dimmed in the desktop app.
- Heroic store games will use the same kind.
- Steam launches the game, so running status, Stop, and playtime have to find
  the `reaper SteamLaunch AppId=<id>` process instead of supervising it.
- Freebies: Steam's cached art without a SteamGridDB key, store playtime, and
  SteamGridDB lookup by app id.

On 2026-10-07 the maintainer answered the open questions below with each
recommendation:

1. Games come in through an explicit `wineo import steam`, one config per
   game, with install state read live.
2. The config uses flat keys: `kind = "proxied"`, `launcher = "steam"`,
   `app_id = "..."`.
3. Steam's live playtime total is the source of truth. Wineo records no
   sessions for Steam games.
4. Games started from the Steam client show as running, and Stop works on
   them. Their `after` hooks don't run.
5. Tools are filtered by known app ids and name prefixes, not by parsing
   `appinfo.vdf`.
6. Only arguments reach a Steam game. Editing Steam's launch options is a
   backlog row.
7. The desktop app is part of this plan.

## Existing foundation

- **Kinds.** `Kind { Wine(WineConfig), Native }` in
  `crates/wineo-core/src/config.rs`. Most code doesn't name `Native`: it
  checks `config.wine().is_none()`, `Host::Native`, or `RunState.native`,
  about 63 places in all. A third kind would silently take the native paths,
  including `resolve_native_exe` and process group kills.
- **Schema.** `SCHEMA_VERSION = 1`. Per 0026's rule, a new `kind` value needs
  no bump: an older wineo refuses `kind = "proxied"` with an unknown variant
  instead of misreading it.
- **Reserved keys.** `store` and `gameid` are already Wine-only keys for umu,
  so the Steam config can't reuse `store`.
- **Running state.** A run is running when another process holds its run
  lock (`Runtime::is_running`, `runtime.rs`). There's no pid check. The
  supervisor watches its own child with `waitid`, which can't watch a process
  it didn't spawn. `process.rs` already has `pidfd_open`.
- **Stopping.** `stop` signals wineo's supervisor, which ends the program's
  process group. Lost-run cleanup kills the recorded group. For a Steam game,
  either could reach the Steam client itself.
- **Playtime.** Sessions are lines in `stats/<env>.jsonl`. Imported playtime
  is one `imported` slot that `status` labels "from Heroic".
- **Steam paths.** `external_runners.rs` already lists Steam roots (native,
  `~/.steam/*`, and Flatpak) and reads library paths from
  `libraryfolders.vdf` with a hand-written tokenizer. There's no VDF crate.
- **Heroic import** is the precedent for importing from a launcher: a pure
  plan step, setters, then apply (`heroic_import/`), `wineo import heroic`
  with `--list`, `--game`, `--all`, `--art`, and `--dry-run`, and a GUI
  import page with a poster grid.
- **Desktop app.** Cards have no kind. `Card.prefix.is_some()` gates the
  Wine-only actions in the context menu, and Clone and Copy to portable are
  still offered for cards without a prefix, then refused by core.

### What Steam's files say

Checked on a real install (native Steam, one library, 13 manifests):

- Each library's `steamapps/appmanifest_<appid>.acf` has `appid`, `name`,
  `StateFlags` (4 means fully installed), `installdir`, `SizeOnDisk`,
  `LastPlayed`, `LastOwner` (a SteamID64), and download progress.
- The `apps` list in `libraryfolders.vdf` can be stale: one installed game
  had a manifest but wasn't listed. Scan the manifests.
- `StateFlags` has an "app running" bit that's stale too: a game that had
  never been played showed it. Never use it.
- Steam's tools have manifests like games, and nothing in them says so: the
  Steam Linux Runtimes (1070560, 1391110, 1628350, 4183110), Steamworks
  Common Redistributables (228980), and Proton builds. Only Steam's binary
  `appcache/appinfo.vdf` records an app's type.
- Playtime: `userdata/<account>/config/localconfig.vdf` has each app's
  `Playtime` in minutes and `LastPlayed`. The account id is the SteamID64
  minus 76561197960265728, and `config/loginusers.vdf` lists accounts.
- `config/config.vdf`'s `CompatToolMapping` names each game's Proton build,
  with `"0"` as the default.
- Art: `appcache/librarycache/<appid>/` has `library_600x900.jpg`,
  `library_hero.jpg`, and `logo.png`.
- `~/.steam/registry.vdf` holds CD keys and the login name. Wineo never reads
  it.

## Proposed design

### Getting games in

An explicit import, like Heroic (question 1).

```sh
wineo import steam --list
wineo import steam --game 1086940
wineo import steam --game 1086940 -n bg3
wineo import steam --all
wineo import steam --all --dry-run
```

- `--list` shows installed Steam games with their app id, title, size, and
  whether they're already imported.
- The name defaults to a slug of the title (`baldurs-gate-3`), like the
  desktop app's Heroic import. A taken name is an error, unless it's already
  this game, which is skipped as already imported.
- `--all` imports every installed game that isn't imported yet and skips
  tools. It's fail-soft like `import heroic --all`.
- Guided `wineo import` gets a third source, Steam.
- Import writes the config, copies Steam's cached art, and tags the
  environment `game`. It never writes inside Steam's folders.

### Config

Flat keys, like `kind` and `heroic_id` (question 2).

```toml
# environments/baldurs-gate-3.toml
schema_version = 1
kind = "proxied"
launcher = "steam"
app_id = "1086940"
tags = ["game"]

[hooks]
after = 'notify-send "Saves synced"'
```

- `launcher` names who launches it. Only `steam` for now; Heroic's store
  games will add `heroic`. `app_id` is a string, since GOG and Epic ids
  aren't numbers.
- A proxied environment has no `prefix`, `runner`, targets, `[env]`, launch
  options, winetricks, or components. Setting one is refused like a Wine-only
  key on a native environment: `baldurs-gate-3 is a Steam game, so it can't
  set runner.`
- It can have `tags`, `[hooks]`, and `desktop`.
- No schema bump, per 0026's rule.
- Title, install state, size, and Proton build are read from Steam each time,
  never stored.
- A new `docs/proxied-environment.toml.example`, tested like the others.

### Launching

- `wineo start NAME [-- ARGS]` runs `steam -applaunch <app_id> [ARGS]`, or
  `flatpak run com.valvesoftware.Steam -applaunch ...` for Flatpak Steam.
  It's spawned in its own session, never in wineo's process group, since it
  starts the Steam client when Steam isn't running.
- Only arguments reach the game (question 6). `-e`, `[env]`, gamemode,
  MangoHud, and gamescope are refused and point to Steam's launch options:
  `baldurs-gate-3 is a Steam game, so set environment variables in its Steam
  launch options.`
- `before` runs first, as for every kind. A detached supervisor then waits
  for the game's `reaper` process: about 60 seconds, longer while the
  manifest says Steam is updating or downloading the game. If it never
  appears, the run fails: `Steam didn't start baldurs-gate-3.`
- Once found, the supervisor watches the reaper through a pidfd, holds the
  run lock as usual, and the run ends when the reaper exits. Then `after`
  runs.
- `run` does the same in the foreground. `restart` works.
- A game Steam lists as not installed (uninstalled after import) refuses to
  start: `Steam lists baldurs-gate-3 as not installed.`

### Running state and Stop

Games started from the Steam client show as running too (question 4).

- `status` and `list` show a proxied environment as running while wineo's
  supervisor holds its lock, or while a reaper for its app id exists. A run
  Steam started shows as `running (started from Steam)`, Stop works on it,
  and no hooks run for it.
- Stop sends SIGTERM to the reaper and the processes under it, then SIGKILL
  after `stop_timeout`. It never signals anything outside the reaper's tree,
  so the Steam client is safe. `--force` and the desktop app's Force stop
  skip straight to SIGKILL.
- Lost-run cleanup never kills a proxied run. It records the end and leaves
  any reaper alone.
- The reaper is found by scanning `/proc` for a `reaper` whose command line
  has `SteamLaunch` and `AppId=<id>`, verified by its start time like other
  owned processes. The command line shape still needs checking with Steam
  running.

### Playtime

Steam's own total is the source of truth (question 3).

- `stats` reads `Playtime` and `LastPlayed` from `localconfig.vdf` for the
  account that owns the game (the manifest's `LastOwner`).
- Wineo records no sessions for proxied environments, so nothing is counted
  twice and play started from Steam counts too.
- `stats NAME` shows `played 370 h on Steam, last Sep 4` with no session
  breakdown, and `--json` reports the seconds plus `"source": "steam"`.
  `--by-tag` and totals include it.

### Other commands

| Command | Proxied environment |
|---|---|
| `start`, `run`, `restart`, `stop`, `status`, `list`, `stats` | Yes |
| `set` | Tags, hooks, and `desktop` only |
| `art` | Yes, and `--art` uses SteamGridDB's lookup by Steam app id |
| `desktop` | Yes: the entry runs `wineo start NAME`, so hooks run |
| `rm` | Removes wineo's config, art, and stats. Never uninstalls; the review says so |
| `validate` | New checks: Steam not found, game not installed, unknown app id |
| `winetricks`, `exec`, `snapshot`, `clone`, `portable copy` | Refused: `Steam manages baldurs-gate-3's prefix.` |
| `import steam --portable` | Refused: a portable folder can't depend on this machine's Steam |

### Kind plumbing

Before anything Steam-specific, step 1 turns every "not Wine, so native"
check into an explicit match on the kind, so a proxied environment can't fall
into native code. That covers `Host`, `RunState`, `report.rs`, `view.rs`
(`kind` becomes `"wine"`, `"native"`, or `"proxied"`), validate, desktop
reconcile, and the GUI's `Card`. `NoPrefix` stops saying "native" for
every kind. `status --json` and `list --json` get `kind`, plus `launcher`
and `app_id` for proxied environments.

### Steam module

A new `crates/wineo-core/src/steam.rs` owns everything Steam-specific:

- Finding Steam: roots, native or Flatpak (moved from `external_runners.rs`,
  which then uses it).
- A small text KeyValues parser for `.vdf` and `.acf`, replacing
  `vdf_paths`.
- Libraries, manifests, and the tools filter: known tool app ids plus the
  name prefixes `Proton`, `Steam Linux Runtime`, and `Steamworks`
  (question 5).
- Account, playtime, `CompatToolMapping`, and art paths.
- Finding a game's reaper.

It never writes to Steam's folders and never reads `registry.vdf`.

### Desktop app

In this plan (question 7).

- Add gets **Import from Steam**: a poster grid of installed games that
  aren't imported yet, like Heroic's, with select all.
- Cards for Steam games show a small Steam mark, and a game Steam lists as
  not installed is grayscaled and dimmed with Play disabled.
- Play and Stop work like other kinds, including Force stop.
- The panel shows `Steam`, the Proton build if any, and the install size.
- The context menu hides Snapshots, Run program, Winetricks, Clone, and Copy
  to portable for Steam games. It also stops offering Clone and Copy to
  portable for native ones, which core already refuses.
- Settings shows only tags and the desktop entry switch.

## Questions

All answered A by the maintainer on 2026-10-07; see the settled list above.
The options are kept as a record of what was weighed.

1. How Steam games get into wineo:
   - **A (recommended).** An explicit `wineo import steam`, one config per
     game, with install state read live.
   - **B.** Automatic: every installed Steam game shows up without an
     import, behind a global setting, and a config is written only when you
     change something.

   A keeps every environment backed by a config, which the whole codebase
   assumes. B would need environments without configs everywhere.
2. Config shape:
   - **A (recommended).** Flat keys: `kind = "proxied"`, `launcher =
     "steam"`, `app_id = "..."`.
   - **B.** A `[launcher]` table with `name` and `app_id`.
3. Playtime:
   - **A (recommended).** Steam's live total is the source of truth, and
     wineo records no sessions for Steam games.
   - **B.** Wineo records its own sessions and imports Steam's total once,
     at import. Play started from Steam is missed afterward.
4. Games started from the Steam client:
   - **A (recommended).** `status` and the desktop app show them running,
     from a live reaper scan, and Stop works on them. `after` hooks don't
     run, since wineo didn't launch them.
   - **B.** Only wineo's own launches show as running.
5. Telling games from Steam's tools:
   - **A (recommended).** A short list of known tool app ids and name
     prefixes (`Proton`, `Steam Linux Runtime`, `Steamworks`). Import is
     explicit, so a stray soundtrack is just one more row.
   - **B.** Parse Steam's binary `appinfo.vdf` for each app's type. Exact,
     but its format has changed under other tools before.
6. What wineo can pass to a Steam game:
   - **A (recommended).** Arguments only, through `steam -applaunch <id>
     ARGS`. No `-e`, `[env]`, gamemode, MangoHud, or gamescope, which only
     reach the game through Steam's own launch options. Editing those (in
     `localconfig.vdf`, only while Steam is closed) goes to the backlog.
   - **B.** Also edit Steam's launch options from wineo.
7. Desktop app support:
   - **A (recommended).** In this plan, like 0043 did.
   - **B.** A follow-up plan, with this one CLI only.

## Implementation sequence

1. Kind plumbing: explicit kind matches everywhere, `Kind::Proxied`,
   config parsing and refusals, the example file, `kind` in JSON, and a
   kind-neutral `NoPrefix`. No Steam yet; a hand-written proxied config loads,
   shows in `list` and `status`, and refuses everything else.
2. `steam.rs`: roots, KeyValues parser, libraries, manifests, tools filter,
   account, playtime, art paths. `external_runners.rs` moves onto it. Tests
   use fixture Steam trees.
3. `wineo import steam` in core (plan, then apply) and the CLI, with
   `--dry-run`, art, and guided `import`.
4. Launching and stopping: the detached Steam spawn, the reaper wait and
   pidfd watch, Stop on the reaper's tree, lost runs, hooks, and validate
   checks.
5. Playtime from Steam in `stats`, `status`, and JSON.
6. Desktop app: Import from Steam, cards, panel, context menu, settings.
7. Docs: a Steam page, config-files, json-output, gui.md, and an acceptance
   checklist.

## Testing and acceptance

- Fixture Steam trees (native and Flatpak layouts, two libraries, a stale
  `apps` list, tools, an uninstalled game, two accounts) cover discovery,
  the tools filter, and playtime.
- The KeyValues parser gets tests for nesting, escapes, comments, and
  malformed files, which fail with a clear error instead of a panic.
- A fake `steam` script spawns a fake `reaper SteamLaunch AppId=<id>` and
  covers start, run, Stop, `--force`, a reaper that never appears, a lost
  supervisor, and hooks. A test checks Stop never signals the fake Steam
  client.
- Every refused command gets a test with the proxied message.
- Tests from step 1 show Wine and native behavior is unchanged.
- The acceptance checklist uses the maintainer's real Steam library: import
  a few games, play one from wineo and one from Steam, stop both, check
  `stats` against Steam's own numbers, and remove one without touching
  Steam.

## Scope limits

- Owned but uninstalled Steam games, installing from wineo, and install
  progress: queue item 5.
- GOG and Epic through Heroic: queue item 5.
- Editing Steam launch options: backlog.
- Non-Steam shortcuts in Steam (`shortcuts.vdf`): its own backlog row.
- Steam Deck and Big Picture specifics.
- Steam's family sharing and multiple Steam accounts beyond reading the
  owner's playtime.
