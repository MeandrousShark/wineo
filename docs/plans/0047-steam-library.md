Status: approved

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

A review on 2026-10-07 found gaps, and the design was revised: native
environments keep Clone and Copy to portable; claiming a game and Stop got
identity rules; startup, cancellation, and lost runs got a contract; one
Steam install is active; missing Steam data has states; stats say what Steam
can't supply; and the implicit `steam` target covers desktop entries and hook
variables. Questions 8 to 10 came out of it, and the maintainer answered
each with its recommendation the same day:

8. One active Steam install for everything: native, unless the global config
   says `steam = "flatpak"`.
9. `start` returns once Steam accepts the launch, and `status` shows
   `starting` until the game appears. `run` waits through it.
10. Import writes `title` to the config, and the live manifest name wins
    while it exists.

A second review the same day tightened the launch: a recheck after `before`,
a concrete handoff, staging progress, run state compatibility, and art. The
maintainer kept the recheck's adopt-it behavior (a game Steam starts during
`before` is claimed, with no second launch, and `after` runs once it exits)
and approved the plan on 2026-10-07.

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
  Wine-only actions in the context menu (`tools_navigation.rs`
  `wine_actions`, which also stays true while a restore is pending). Clone
  and Copy to portable are offered for every card, and native environments
  have their own working paths for both (`portable::inspect_native`, and
  [native.md](../native.md)).

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

### Choosing a Steam install

Native Steam (`~/.local/share/Steam`, which `~/.steam/steam` and
`~/.steam/root` point at) and Flatpak Steam
(`~/.var/app/com.valvesoftware.Steam/.local/share/Steam`) are found the way
`external_runners.rs` finds them, deduplicated by canonical path.

Wineo uses one active install for everything: import, launch, art, playtime,
and the reaper scan (question 8). When only one is found, it's that one. When
both are, native wins unless the global config says `steam = "flatpak"`. An
app id that only the inactive install has is treated as not installed, so
the two installs are never mixed for one game.

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
- A game is already imported when any environment has `launcher = "steam"`
  and its `app_id`, whatever that environment is called. Importing it again
  is skipped with `Already imported as bg3.`
- The name defaults to a slug of the title (`baldurs-gate-3`), like the
  desktop app's Heroic import. A name taken by a different environment is an
  error.
- `--all` imports every installed game that isn't imported yet and skips
  tools. It's fail-soft like `import heroic --all`.
- Guided `wineo import` gets a third source, Steam.
- Import writes the config, copies Steam's cached art, and tags the
  environment `game`. It uses no template, and it never writes inside
  Steam's folders.
- Art comes from `appcache/librarycache/<app_id>/`: `library_600x900.jpg` as
  Grid, `library_hero.jpg` as Hero, and `logo.png` as Logo. Icons aren't
  mapped, since Steam's icon is a hash-named file. A missing file is
  skipped, and a file that isn't a valid image is skipped with a warning.
  Art never fails an import.
- `--art` fills in the rest from SteamGridDB by Steam app id
  (`/games/steam/<app_id>`), not by a title search.

### Config

Flat keys, like `kind` and `heroic_id` (question 2).

```toml
# environments/baldurs-gate-3.toml
schema_version = 1
kind = "proxied"
launcher = "steam"
app_id = "1086940"
title = "Baldur's Gate 3"
tags = ["game"]
desktop = false

[hooks]
after = 'notify-send "Saves synced"'
```

- `launcher` names who launches it. Only `steam` for now; Heroic's store
  games will add `heroic`. `app_id` is a string, since GOG and Epic ids
  aren't numbers.
- `title` is written at import and shown when Steam no longer has the game's
  manifest. While the manifest exists, its `name` wins (question 10).
- A proxied environment has no `prefix`, `runner`, targets, `[env]`, launch
  options, winetricks, or components. Setting one is refused like a Wine-only
  key on a native environment: `baldurs-gate-3 is a Steam game, so it can't
  set runner.`
- It can have `tags`, `title`, `[hooks]`, and `desktop`.
- No schema bump, per 0026's rule.
- Install state, size, and Proton build are read from Steam each time, never
  stored.
- A new `docs/proxied-environment.toml.example`, tested like the others.

### The implicit target

A proxied environment has exactly one launch, so it gets one implicit target
named after its launcher: `steam`. It's never written to the config, and
`[targets]` is still refused. It gives the rest of wineo a target name where
it expects one:

- The desktop entry is `wineo-<env>-steam.desktop`, from the top-level
  `desktop` key, which defaults to `false`. Its title is the game's title.
- Run state records target `steam`, and `status` shows `target  steam
  (Baldur's Gate 3)`.
- `wineo start NAME steam` works, and any other target name is an error.

### Global defaults

Nothing global reaches a Steam game: the global `[env]`, `gamemode`,
`mangohud`, `gamescope`, `wayland`, and template defaults are skipped for
proxied environments, and `status` doesn't list them. Only `stop_timeout`
applies, to Stop.

### Launching

- `wineo start NAME [-- ARGS]` runs `steam -applaunch <app_id> [ARGS]`, or
  `flatpak run com.valvesoftware.Steam -applaunch ...` for the Flatpak
  install. It's spawned in its own session, never in wineo's process group,
  since it starts the Steam client when Steam isn't running.
- Only arguments reach the game (question 6). `-e`, `[env]`, gamemode,
  MangoHud, and gamescope are refused and point to Steam's launch options:
  `baldurs-gate-3 is a Steam game, so set environment variables in its Steam
  launch options.`
- Before anything runs, wineo checks for a running game: if a reaper for the
  app id already exists, `start` says `baldurs-gate-3 is already running
  (started from Steam).` and exits 0, like `start` on any running
  environment, and `run` exits 1. No hooks run. This first check also takes
  the claiming baseline (see below).
- Then `before` runs, as for every kind.
- The supervisor takes the run lock and checks again. If Steam started the
  game while `before` ran, say from a click in Steam, that reaper is
  eligible, since it started after the baseline. The supervisor claims it
  without asking Steam for a second launch, prints `Started baldurs-gate-3
  (Steam had already launched it).`, and the run goes straight to
  `running`. `before` already ran for this game, so `after` runs when it
  exits, as usual. No false timeout, and no `after` while the game is alive.
- Otherwise it dispatches the request: the `steam` command is spawned
  through an intermediate process that exits at once, so the request
  process, which becomes the Steam client on a cold start, is never wineo's
  child.
- Steam has accepted the handoff once the request process has spawned.
  Wineo never waits for it to exit, since on a cold start it doesn't. A
  spawn failure, like a missing `steam` binary, fails the launch right away,
  and `after` runs per the contract.
- A run has three phases, which `status` and the desktop app show:
  `starting` (asked Steam, waiting for the game), `running`, and ended.
- `start` returns once the handoff is accepted, printing `Asked Steam to
  start baldurs-gate-3.` and exiting 0 (question 9). `run` stays in the
  foreground through all three phases.
- The game must appear within 60 seconds of dispatch. While Steam is working
  on the game (the `updating` state below), the wait goes on as long as it
  makes progress, and fails after 10 minutes without any. Progress is any
  increase in `BytesDownloaded` or `BytesStaged`, or any change in
  `StateFlags`, so staging counts even when nothing is downloading. A run
  that never sees its game fails: `Steam didn't start baldurs-gate-3.`
- Stop or Ctrl+C during `starting` ends the wait right away. The run ends
  `stopped`. Wineo can't take back a launch Steam has already queued, so a
  game that appears afterward isn't claimed by this run. It shows as
  `running (started from Steam)`.
- `after` follows the existing contract: it runs once the launch got past
  `before`, however the run ended, including a failed or canceled
  `starting` phase.
- A game Steam lists as not installed refuses to start: `Steam lists
  baldurs-gate-3 as not installed.` An unavailable library refuses with its
  own reason (see below).

### Claiming the game

A run claims exactly one reaper, so it never takes over someone else's
launch or runs hooks for it.

- A reaper matches an environment when it's a `reaper` process whose command
  line has the token `SteamLaunch` and exactly `AppId=<app_id>`, and whose
  ancestors include the active install's Steam client.
- Confirmed on 2026-10-07 with Civ VI under Proton: the reaper's argv is
  exactly `reaper`, `SteamLaunch`, `AppId=289070`, `--`, then the command,
  so matching compares whole arguments. Its parent is the Steam client
  itself. The reaper and the runtime's `srt-bwrap` share the Steam client's
  process group and session, while pressure-vessel's `pv-adverb` starts a
  new session and each Wine process (wineserver, explorer, the game) gets
  its own. So a group signal would hit Steam and miss the game, and only
  the tree walk works. `pv-adverb` adopts Wine's processes, so the game and
  wineserver stay descendants of the reaper. There's no separate pid
  namespace, so host pids and pidfds work.
- Steam's own Stop, watched the same day: the whole tree exited within
  about 240 ms. `srt-bwrap` went first, every Wine process within about 10
  ms of it, then `pv-adverb` and the game, and the reaper last. Steam gives
  the game no grace period, so wineo's polite Stop (SIGTERM to the verified
  tree, then SIGKILL after `stop_timeout`) is already gentler than Steam's,
  and ending only the reaper isn't how Steam does it. After Steam's Stop,
  nothing from the game is left while the Steam client's own runtime keeps
  running, which is the baseline for acceptance.
- The baseline is taken at the first running check, before `before`: its
  time, and every matching reaper that exists then (normally none, since
  `start` refused if one did). A run only claims a reaper that isn't in the
  baseline and started after the baseline time.
- Anchoring to the baseline, not to dispatch or the accepted handoff, means
  a game that starts fast during dispatch, or one launched from Steam during
  `before`, is never excluded.
- Steam runs one instance of a game at a time, so a reaper that starts after
  the baseline belongs to this launch, even if Play was also clicked in
  Steam during that window. Claiming it is correct.
- More than one candidate at once is ambiguous: the run claims none, ends
  `failed` with `More than one Steam process matches baldurs-gate-3.`, and
  leaves them all alone.
- The claim is recorded as pid, start time, and boot id, the same identity
  `process.rs` checks for groups, and the supervisor watches it through a
  pidfd.

### Running state and Stop

Games started from the Steam client show as running too (question 4).

- `status` and `list` show a proxied environment as `starting` or `running`
  while wineo's supervisor holds its lock. Without a wineo run, a matching
  reaper shows as `running (started from Steam)`. Stop works on it, and no
  hooks run for it.
- Two matching reapers without a wineo run show as running, and Stop refuses
  with the ambiguity message rather than picking one.
- Stop works on the reaper's tree: the reaper plus every process whose
  parent chain leads to it, found by walking `/proc` from the verified
  reaper. Each one is opened as a pidfd and checked against its start time
  before it's signaled, so a reused pid is never hit. Nothing outside the
  tree is signaled, so the Steam client is safe.
- Stop sends SIGTERM to the tree, rescans for new members while it waits,
  and sends SIGKILL to whatever is left after `stop_timeout`. `--force` and
  the desktop app's Force stop skip straight to SIGKILL.
- Processes that outlive the reaper during a Stop are still killed: the
  supervisor keeps pidfds for every tree member it has seen, so a child that
  was reparented away from the reaper still gets its SIGKILL.
- When the game exits on its own, the run ends when the reaper exits.
  Leftover children are Steam's business, the same as when Steam launches it.
- The game's exit code can't be collected, since wineo isn't the reaper's
  parent.

### Lost runs

When a proxied run's supervisor dies, the next `status`, `start`, or `stop`
finds the lock free and the state still `starting` or `running`. Cleanup:

- never signals anything;
- marks the run `stopped` and lost, ending at its last heartbeat;
- writes no playtime session, since Steam's total is the source of truth;
- releases the run, so the next launch isn't blocked. A reaper that's still
  alive shows as `running (started from Steam)`.

### Hook variables

For a proxied environment's hooks:

- `WINEO_ENVIRONMENT`, `WINEO_HOOK`, `WINEO_PORTABLE_DIR` as usual, and
  `WINEO_TARGET=steam`.
- `WINEO_LAUNCHER=steam` and `WINEO_APP_ID=1086940`, which are new.
- `WINEO_PREFIX` is Steam's `compatdata/<app_id>/pfx` when that exists, so a
  save-sync hook can find a Proton game's saves. It's set but never written
  to by wineo.
- No `WINEO_EXE`. `WINEO_EXIT_CODE` is always empty, since the exit code
  can't be collected.
- `WINEO_END` is `exited` when the reaper exited on its own, `stopped`, or
  `failed` when the game never started. `WINEO_SECONDS` covers the
  `running` phase only.

### When Steam data is missing

Each Steam game is in one of these states, shown in `status`, `list`,
validate, and the desktop app:

| State | Meaning | Play |
|---|---|---|
| installed | Its manifest says fully installed | Works |
| updating | Its manifest says Steam is updating it | Works, waits in `starting` |
| not installed | Steam is readable and no available library has its manifest | Refused, grayscaled card |
| unavailable | A library listed in `libraryfolders.vdf` is missing, so it might be there | Refused: `Steam's library at /mnt/games isn't available.` |
| unknown | Steam isn't found, or a file can't be read or parsed | Refused with the reason |

- Unavailable and unknown aren't uninstalled, so their cards aren't
  grayscaled. They show a warning instead.
- A manifest that can't be read or parsed affects only its own app id. That
  game is unknown, and every other game still lists and imports: `--list`
  shows it as `unreadable manifest`, and `--all` reports it as failed
  without stopping.

Installed and updating come from the manifest's `StateFlags` bits:

- **updating:** any of UpdateRunning (256), UpdatePaused (512),
  UpdateStarted (1024), Validating (131072), AddingFiles (262144),
  Preallocating (524288), Downloading (1048576), Staging (2097152), or
  Committing (4194304) is set, whether or not FullyInstalled is.
- **installed:** FullyInstalled (4) is set and none of those are.
  UpdateRequired (2) alongside FullyInstalled (flags 6) still counts as
  installed, since Steam updates on launch and the `starting` wait covers
  it.
- **not installed:** Uninstalling (2048) is set, even alongside an updating
  bit, or FullyInstalled isn't set and no updating bit is. UpdateRequired
  without FullyInstalled (flags 2) is not installed.
- AppRunning (64) is always ignored. FilesMissing and FilesCorrupt don't
  change the state, since Steam repairs those on launch.
- Watched on 2026-10-07 during a real Overwatch update: `StateFlags` was
  1030 (FullyInstalled, UpdateRequired, and UpdateStarted) the whole time,
  with no Downloading or Staging bits, then 4 when it finished.
  `BytesStaged` kept growing after `BytesDownloaded` reached its total, so
  staging-only progress is real. Steam rewrote the manifest about every 40
  seconds, well inside the 10-minute stall limit.
- The title falls back to the config's `title` when there's no manifest.

### Playtime

Steam's own total is the source of truth (question 3).

- The account is the most recent one in `config/loginusers.vdf`. Playtime
  belongs to whoever played, which is the logged-in account, not the
  manifest's `LastOwner` (they differ for family sharing). So the account
  doesn't depend on the manifest still existing.
- `stats` reads `Playtime` and `LastPlayed` for the app from that account's
  `localconfig.vdf`.
- Known zero and unknown are different. A readable `localconfig.vdf` with no
  entry or `Playtime` 0 is 0 h. A missing account, or a file that can't be
  read or parsed, is unknown.
- Wineo records no sessions for proxied environments, so nothing is counted
  twice and play started from Steam counts too.

What Steam can't supply is left out, not shown as zero:

- `stats NAME` shows only `played  370 h on Steam, last Sep 4`. There are no
  session, average, longest, first played, this week, or this month lines.
  Unknown shows as `played  unknown (Steam's playtime can't be read)`.
- The overview shows Steam's total under played and `-` under sessions.
- `--json` has `seconds` and `last_played` (`null` when unknown), `"source":
  "steam"`, and `null` for `sessions`, `average_seconds`, `longest`,
  `first_played`, and the week and month totals. Wine and native
  environments get `"source": "wineo"`.
- `--by-tag` and the overview total add Steam's totals to the played column,
  while sessions count only wineo's own. When any Steam game contributes,
  a note says `Steam games count Steam's total, without sessions.` A game
  whose playtime is unknown is left out of totals, with `1 Steam game's
  playtime is unknown.`
- Nothing for a Steam game says "from Heroic".

### Other commands

| Command | Proxied environment |
|---|---|
| `start`, `run`, `restart`, `stop`, `status`, `list`, `stats` | Yes |
| `set` | `tags`, `title`, `hooks`, and `desktop` only |
| `art` | Yes, and `--art` uses SteamGridDB's lookup by Steam app id |
| `desktop` | Yes: the entry runs `wineo start NAME`, so hooks run |
| `rm` | Removes wineo's config, art, and stats. Never uninstalls; the review says so |
| `validate` | New checks: Steam not found, no active install, game not installed, library unavailable, unreadable files |
| `winetricks`, `exec`, `snapshot`, `clone`, `portable copy` | Refused: `Steam manages baldurs-gate-3's prefix.` |
| `import steam --portable` | Refused: a portable folder can't depend on this machine's Steam |

### Kind plumbing

Before anything Steam-specific, step 1 turns every "not Wine, so native"
check into an explicit match on the kind, so a proxied environment can't fall
into native code. That covers `Host`, `RunState`, `report.rs`, `view.rs`
(`kind` becomes `"wine"`, `"native"`, or `"proxied"`), validate, desktop
reconcile, and the GUI's `Card`. Old run state stays readable: a
`state.toml` with `native = true` is a native run, and `native = false` or
no `native` key is a Wine run. Proxied runs add a `launcher` field, which
older files never have. `NoPrefix` stops saying "native" for
every kind. `status --json` and `list --json` get `kind`, plus `launcher`
and `app_id` for proxied environments. The GUI keeps showing Snapshots, Run
program, and Winetricks while a restore is pending, as `wine_actions` does
today.

### Steam module

A new `crates/wineo-core/src/steam.rs` owns everything Steam-specific:

- Finding installs and choosing the active one (moved from
  `external_runners.rs`, which then uses it). Runner discovery from 0043
  keeps scanning every Steam install it finds, native and Flatpak, since the
  active install only applies to games.
- A small text KeyValues parser for `.vdf` and `.acf`, replacing
  `vdf_paths`.
- Libraries, their availability, manifests, and the tools filter: known tool
  app ids plus the name prefixes `Proton`, `Steam Linux Runtime`, and
  `Steamworks` (question 5).
- Account, playtime, `CompatToolMapping`, and art paths.
- Finding, verifying, and walking a game's reaper.

It never writes to Steam's folders and never reads `registry.vdf`.

### Desktop app

In this plan (question 7).

- Add gets **Import from Steam**: a poster grid of installed games that
  aren't imported yet, like Heroic's, with select all.
- Cards for Steam games show a small Steam mark. Not installed is
  grayscaled and dimmed with Play disabled; unavailable and unknown show a
  warning.
- Play and Stop work like other kinds, including Force stop. The panel shows
  `Waiting for Steam...` during `starting`, with Stop available.
- The panel shows `Steam`, the Proton build if any, and the install size.
- The context menu hides Snapshots, Run program, Winetricks, Clone, and Copy
  to portable for Steam games. Native environments keep Clone and Copy to
  portable, which work for them.
- Settings shows tags, title, and the desktop entry switch.

## Decisions during implementation

- Step 2 (2026-10-07): a manifest without `StateFlags` is an error for its
  app, not "not installed". A library is available only when its
  `steamapps` folder is readable or absent. An unreadable
  `libraryfolders.vdf` makes a game with no manifest unknown, not "not
  installed". `compat_tool` reads an unreadable `config.vdf` as no tool,
  since it's display-only. KeyValues conditions are evaluated for Linux
  (`$LINUX` and `$POSIX` true, everything else false, `!` negates, joins
  read left to right, every tag on an entry must hold), and unknown escapes
  keep their backslash. Tool names match `Proton ` and `Steamworks ` with a
  trailing space.
- Step 1 (2026-10-07): a proxied-only key on a Wine or native config is
  refused with `NAME isn't a Steam game, so it can't set KEY.`, and a bad
  app id with `Invalid app_id: empty.` or `Invalid app_id: contains a NUL
  byte.` JSON leaves `launcher` and `app_id` out for other kinds, and list
  JSON's `kind` is `null` for an unreadable config. `wineo setup` refuses
  Steam games. `Host` gets its proxied variant in step 4, and until then a
  lost proxied run reports unresolved instead of being cleaned up. Step 4's
  lost-run test must use a live process group, so it fails if the proxied
  branch is removed (QA note).
- Step 3 (2026-10-07, decided while the maintainer was away and confirmed
  by them the same day): apply rechecks already imported, install state, and the name
  under an exclusive per-app lock (`steam-<appid>.lock`), using the install
  it reselects for the title and art too, and returns `AlreadyImported`
  instead of importing twice. `--all` mirrors Heroic's summary: `Imported 1
  of 2 games.`, with games already imported at review or apply time left out
  of the total, then `1 already imported: NAME` for ones found at apply
  time, then `1 failed: NAME`. Portable mode refuses with `Steam games can't
  be imported into a portable folder.`
- Step 5 (2026-10-07, confirmed by the maintainer): when the global config can't be loaded, a Steam
  game's playtime is unknown rather than read from a guessed install. In the
  desktop app, a card whose stats summary is missing shows `unknown`, `-`,
  and `unknown` for a Steam game, and `never played` as before for Wine and
  native.
- Step 4 (2026-10-07, maintainer): `list` shows a Steam game's install
  state in place of `inactive` when it isn't plainly installed (`not
  installed`, `updating`, `unavailable`, `unknown`); a starting or running
  run still shows as such. `status` says it the same way, with the reason:
  `satisfactory: unavailable (Steam's library at /mnt/games isn't
  available)`. JSON gets `install` (`installed`, `updating`,
  `not_installed`, `unavailable`, `unknown`) and `install_problem` when
  there's a reason, for proxied environments only. `wineo run` on a Steam
  game exits 0 when the game exits on its own, 130 when it's stopped with
  Ctrl+C or `wineo stop`, and 1 when Steam never starts it, since the
  game's own exit code can't be collected.

## Questions

All ten were answered A by the maintainer on 2026-10-07; see the settled
list above. The options are kept as a record of what was weighed.

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

8. Which Steam install wineo uses when both native and Flatpak exist:
   - **A (recommended).** One active install for everything: native, unless
     the global config says `steam = "flatpak"`.
   - **B.** Record the install in each environment at import
     (`steam_install = "flatpak"`), so different games can use different
     installs.
9. When `start` reports success for a Steam game:
   - **A (recommended).** Once Steam accepts the launch: `Asked Steam to
     start baldurs-gate-3.`, with `starting` in `status` until the game
     appears. A download can take hours, and `start` shouldn't block on it.
   - **B.** Once the game's process appears, so `start` blocks through any
     update.
10. The title when Steam no longer has the game's manifest:
   - **A (recommended).** Import writes `title` to the config, and the live
     manifest name wins while it exists.
   - **B.** Fall back to the environment name.

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
4. Launching and stopping. First confirm the reaper's command line and
   ancestry with Steam running, and record it in the plan. Then the detached
   Steam spawn, the `starting` phase and its waits, claiming, the pidfd
   watch, Stop on the verified tree, lost runs, the implicit target, hook
   variables, and validate checks.
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
  covers start, run, Stop, `--force`, a reaper that never appears, a stalled
  and a progressing download, Stop and Ctrl+C during `starting`, a game
  already running, two matching reapers, a reaper for a different app id or
  install, a child that outlives the reaper during Stop, a lost supervisor
  (no session written, next launch not blocked), and hooks with their
  variables. A test checks Stop never signals the fake Steam client.
- Install selection with both installs present, and the missing-data states:
  not installed, an unavailable library, an unreadable manifest, and unknown
  versus zero playtime.
- Stats text and JSON for Steam games, with no session fields and no "from
  Heroic", and mixed `--by-tag` totals.
- Native Clone and Copy to portable still work in the desktop app.
- Launch ordering: a reaper that appears during `before` is claimed with no
  second dispatch, and `after` runs only once it exits. A reaper that
  appears during dispatch is claimed. A failed spawn fails the launch. A
  request process that never exits, like a cold-started client, doesn't
  block `start`.
- Install states: a table test over `StateFlags` combinations, and a
  staging-only wait where `BytesStaged` grows while `BytesDownloaded` doesn't,
  which must not time out.
- Compatibility: old `state.toml` files with `native = true`, `native =
  false`, and no `native` key load as before. Runner discovery still finds
  builds in both Steam installs when only one is active.
- Art: the cache file mapping, missing files, an invalid image, and an
  import that succeeds while every art file fails. SteamGridDB's lookup by
  Steam app id gets its own test, separate from the existing game id lookup.
- A malformed manifest beside healthy ones: the healthy games still list,
  import, and launch.
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
- Family sharing and multiple Steam accounts, beyond reading the most
  recent account's playtime.
