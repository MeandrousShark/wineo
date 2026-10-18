Status: done

# 0051: Lean and simple

Galaxy builds, called 0051 in plans 0049 and 0050, are now 0052.

## Goal

Make wineo's code as easy to read and as simple as it can be without losing
functionality, and make it use as few system resources as it can, above all
while idle: open in the background, or supervising a game. Done before 1.0,
since 1.0 freezes behavior and the repo goes public then.

## How this was found

A read-only audit on 2026-10-13 (v0.31.0): five readers covered core's data
modules, core's runtime modules, the CLI, the desktop app, and the GOG crate
with cross-crate duplication. Their findings were checked against the code,
and the resource ones against measurements.

## Baselines (v0.31.0 release build)

- Code: about 121k lines of app code and 121k of tests. No unused
  dependencies.
- Binaries: `wineo` 17.0 MiB, `wineo-gui` 42.8 MiB. The release profile only
  strips symbols (no LTO).
- CLI, real library of 22: `--version` 1.9 ms, `list` 20.4 ms, `status`
  19.3 ms, `list` peak RSS 12 MB. Already lean.
- Supervisor during a game: 8.4 MB RSS, 4 threads, 0% CPU, but about 100
  wakeups a second for the whole session: a signal thread and a child-exit
  waiter each sleep-poll every 20 ms.
- Desktop app idle on the library (Xvfb, software rendering, so RSS and
  render CPU are inflated): 6.45% CPU, 61 threads (22 tokio workers). It
  updates about twice a second: every 2 s it rescans the whole library
  (configs, locks, a `/proc` sweep, Steam's files, play logs, art, GOG
  folders, trash) and redraws twice, whether or not anything changed.

How to measure again:

- CLI: `hyperfine -N -w 3 -r 30 "wineo list" "wineo status"`.
- Supervisor: a native environment in temporary XDG folders whose program
  is `exec sleep 40`, then `wineo start`, then `pidstat -w -t -p PID 1 15`
  on the `__supervise` process.
- Desktop app: launch in Xvfb with real data, wait 15 s, then
  `pidstat -u -w -t -p PID 1 20` and `VmRSS`/`Threads` from
  `/proc/PID/status`. A real-GPU measurement needs the maintainer's session.

## Track A: resource fixes (patch work)

No visible behavior change, so these need no approval (the maintainer gave
the go-ahead on 2026-10-13). They ship as 0.31.x patch releases, each change
with before and after numbers.

Supervisor and runs:

- The signal thread blocks in `signals.wait()` instead of polling every
  20 ms (core `supervisor.rs:138-159`, and the copy in the CLI's
  `Cancellation`, `transfer.rs:112-141`). Verified: signal-hook 0.4.4's
  `close()` wakes `wait()`.
- The child-exit waiter blocks in `waitid` instead of polling every 20 ms
  (`supervisor.rs:274-320`).
- `Token` gets a real wait (a Condvar) instead of being a polled atomic.
  That removes the supervisor's 50 Hz pulse in attached runs and lets every
  20-100 ms cancel poll (hooks, stop, exec, winetricks, Steam install, prefix)
  sleep until canceled.
- Steam's run phase waits on the pidfd in a blocking `poll` instead of
  20 ms sleeps; its start phase wakes on Stop directly.
- A native group that outlives its launcher is checked with a backoff
  (100 ms, then up to 1 s), not a full `/proc` scan every 100 ms.
- `after_hook` drains other launches with a blocking `flock` instead of
  about 200 retries a second.

Desktop app:

- Cursor moves stop producing a message (and a full view rebuild and
  redraw) each; the pointer is read when the right-click happens.
- The spinner becomes a self-animating widget, so frames tick only while a
  spinner is on screen, and `spinner_angle` stops being threaded through
  about 20 views.
- The toast and scrollbar timers become one-shot delays instead of 100 ms
  and 250 ms ticks (today a hovered toast ticks at 10 Hz forever).
- The hook-capture reader blocks instead of polling every 5 ms.
- The art picker's decode permit is awaited instead of polled every 10 ms,
  and its thumbnails stop opening the keyring (for a key they never send),
  building a new HTTP client, and writing a temp file each.
- No filesystem calls in `view()`: the action menu and remove dialog
  compute their reasons when they open.
- Decoded library art is evicted outside the visible rows (a byte budget,
  like the art picker's cache), uninstalled cards keep only the dimmed copy,
  and graying happens off the UI thread.
- Import flows decode posters through the same capped, disk-cached
  pipeline as the library.

Core and CLI:

- One HTTP client per batch instead of one per call; GOG's download client
  is built only for downloads.
- `stats::overview` and `desktop::list` stop re-reading the same files per
  environment; `build_removal` builds its uses index once; clone, portable
  copy, and snapshots drop the redundant pre-lock fingerprint pass.
- Heroic import plans stop re-parsing Heroic's library per game (O(N^2)
  today).
- `TimeZone::system()` is read only when a row shows a time.

## Track B: mechanical simplification (patch work)

No behavior change, so, like Track A, no approval needed. 0.31.x patches.

- Remove dead code: about 20 unused functions and parameters across core,
  the GOG crate, and the app.
- Move test-only hooks out of production signatures (`Locks::at`, trash's
  fake mounts, injected reflink and lock closures, `WINEO_TEST_*` reads).
- Replace 69 redundant `.map_err(|err| anyhow!(err))` with `?`.
- One copy each of small helpers written several times: byte sizes, hex,
  one-line errors, plurals, shown paths, directory fsync and synced writes,
  `NotFound` handling, build locks from a config, group kill, `/proc`
  readers, cancel checks and exit code 130, keyring constructors, fake HTTP
  servers in tests.
- Logic the CLI and app both wrote moves to core: import preflight checks,
  the Steam bulk import loop, GOG migrate review rows, GOG recorded state,
  Heroic hook parsing, runner candidates, GOG update labels and progress.
- One task helper for the app's ~100 `spawn_blocking` sites.
- Inline tests move to their own files (8k+ lines out of the biggest app
  files), then large files split along their seams: `trash.rs`,
  `import.rs`, `import_heroic.rs`, `settings/mod.rs`, `main.rs`,
  `flow/heroic.rs`.
- Table-driven `validate::Code` (about 200 lines of hand-kept matches).

## Track C: decisions for the maintainer

This is the part of the plan that needs approval. These change behavior or
architecture, so each needs an answer before work starts. Recommendations
first.

Answered (2026-10-13): the maintainer took every recommendation, including
those for items 2, 5, and 8 to 12, which were written after the first
answer and confirmed separately.

1. **The app's 2 s library rescan.** (a, recommended) Pause it while the
   window is unfocused and refresh on focus, and only redraw when the
   result changed. (b) inotify on wineo's folders, no timer. (c) Keep 2 s.
   **Answer: a.**
2. **Steam detection cadence.** The start phase scans `/proc` 4 times a
   second while waiting for Steam to launch a game. Slowing it to 1 s makes
   detection up to a second later. Recommended: keep 250 ms for the first
   10 s, when a launch usually lands, then back off to 1 s, since the wait
   can last minutes while Steam updates the game.
   **Answer: recommended.**
3. **The trash check in every config load.** `Environment::load` reads
   every trash manifest, so `list` costs environments x trash entries.
   Recommended: read only manifest headers, or memoize once per command.
   Integrity code, so it gets its own review. **Answer: recommended.**
4. **GOG package hashing.** Cached packages are MD5-hashed twice per update
   (on reuse, and again when pinned under the lock). Recommended: one full
   verify, under the lock. **Answer: recommended**, then revised to keep
   both hashes. The double only happens when resuming an interrupted
   update from cached packages: the first hash decides the plan needs no
   download, the second confirms the bytes under the lock. A size-only first
   check would let a same-size corrupted package through to a "game changed"
   error that repeats on every retry, since nothing would clear the file.
   Fresh downloads are hashed while streaming and again at the pin, which
   stays too: another process could replace the file between the two locks.
5. **Content vs metadata fingerprints** for clone and portable copies, which
   read the whole prefix 3-4 times today. Recommended: keep content
   fingerprints, since they guard against data loss, and only drop the
   redundant passes (Track A).
   **Answer: recommended.**
6. **One stop-escalation helper.** The "TERM, wait, KILL, settle" loop
   exists five times (supervisor, exec, winetricks, prefix wait, hooks), and
   the process identity triple travels through ~16 signatures. Recommended:
   one shared helper and one `Identity` type. Process teardown is
   security-sensitive, so this one gets its own review. **Answer:
   recommended.**
7. **Shared shapes for the import flows.** A shared import grid for the
   app's Heroic, Steam, and GOG pages; a trait for `flow::Page`; one
   stage-and-publish helper for clone, portable, and import. **Answer: do
   all three**, each its own patch with its own review.
8. **`windows_installer` in JSON.** Core never gives it any more. Removing
   it from the vocabulary touches the 1.0 JSON contract. Recommended:
   remove it now, while still 0.x, with a changelog note.
   **Answer: recommended.**
9. **One D-Bus stack.** The app carries both zbus (iced) and libdbus (the
   keyring crate). A zbus-based keyring would drop libdbus and the `dbus`
   build dependency. Recommended: yes, as its own patch with its own
   review, since it handles secrets.
   **Answer: recommended.**
10. **HTTP stack.** reqwest's blocking client brings tokio and a thread per
    client into the CLI. A synchronous client is a bigger change.
    Recommended: keep reqwest. The CLI already runs `list` in 20 ms and
    12 MB, and one shared client per batch (Track A) removes most of the
    cost.
    **Answer: recommended.**
11. **LTO for release builds.** Smaller and faster binaries, slower release
    builds. Recommended: yes, picking thin or full LTO by measured size
    and speed.
    **Answer: recommended.**
12. **Guided flows** are written four ways (linear, two numbered step
    machines, a typed step enum). Pick one pattern. Recommended: the typed
    step enum, since the compiler checks every step is handled. Flows move
    to it one at a time, not in one rewrite.
    **Answer: recommended.**

Follow-up answers (2026-10-13, all recommended):

- C9: the keyring uses the zbus-based `secret-service` crate through a
  current-thread runtime made per call, not zbus's own multi-thread runtime
  (which added 16 threads to both binaries). Removing a key from a locked
  keyring prompts to unlock first, like reading and saving. The real-keyring
  test skips quietly when it isn't set up. `dbus` is not an optional
  package dependency.
- C6: the stop helper's behavior changes are accepted: a force kills at
  once, a cancelled tool waits until its whole group is gone and kills a
  stubborn child at the stop timeout, and a stop that can't be confirmed
  says so instead of reporting success.
- C7c: import and portable copies hold the install lock as long as clone
  does; clone's final check is cancellable and shows progress like
  import's; portable copies give the same "appeared" messages as clone and
  import. The `Finished` event timing difference stays.

Calls made for review (2026-10-13 night): the maintainer asked for
recommended calls to be made and reviewed later. Each is listed here. The
maintainer approved all of them on 2026-10-14.

- Track B, test hooks: two private functions keep a test-only closure
  parameter (`build_removal::remove_after_check`, trash's
  `restore_locked_noting`). Their public callers don't take it and release
  builds pass no-ops; moving them behind `cfg(test)` would mean rewriting
  four integrity tests to own all their data. Kept as is.
- Track B, import logic: the app's prefix import review now runs the CLI's
  full preflight checks (core `import::preflight`), so it refuses a few
  things it used to let through to a later failure: a destination another
  environment already names, a prefix that nests with another one when
  adopting, an unreadable environment config, and a dangling config
  symlink; its overlap message now reads like the CLI's. The CLI's checks
  were the reference.
- Track B, Heroic playtime notes: the CLI says "could not read Heroic's
  playtime" and the app "Heroic playtime could not be read"; unifying them
  would change one frontend's wording, so both stay.
- Track B, GOG update progress: the shared core `Reporter` starts its
  1-second pacing when it builds a line, where the CLI used to start it
  after printing. Only visible when stderr is so slow that printing one
  line takes over a second; then the next line prints at once. Kept, since
  matching it exactly would add a step to the shared API.
- Track B, splitting the app's big dispatchers: the Heroic flow's `update`,
  settings' `handle` (now `settings/handle.rs`, 1,327 lines), and the app's
  `update_message` (in `main.rs`, now 1,703 lines) are each one large
  `match`. Splitting them further means pulling groups of arms into new
  methods, a refactor rather than a move, so it wasn't done. Worth a
  separate, reviewed step if wanted.

## Possible bugs (fix as patches if confirmed)

- Fixed: the Art page re-decoded every preview whenever the library
  reloaded, flashing "Loading...". Rows now keep a preview while its file's
  mtime and size are unchanged, and a failed decode says "Can't show this
  image" instead of loading forever.
- `list`, `status`, and the app's overview clean up a lost Steam run from a
  read path (`report.rs:496`). Intended, confirmed by the maintainer
  (2026-10-17): it's what keeps a crashed Steam game from reading Running.
  Its doc comment claimed a `lost` session was logged; Steam runs log no
  sessions by design (`stats.md`), so the comment was fixed instead.
- Fixed: CLI tables padded by bytes, so non-ASCII titles misaligned
  (`table::render_columns`, `render_bulk_table`).
- Not a bug: `recorded_games` has the portable guard in core
  (`gog/update.rs`), which both the app and the CLI call.
- Fixed: two test fake-wine processes from an earlier test run were still
  alive 11 hours later. Two winetricks tests ran a fake that looped forever,
  and several tests spawned children with no cleanup on panic. Long-lived
  test spawns now use a kill-on-drop guard, fake loops are bounded, and
  fixture drop sweeps fakes whose environment config is already gone.
- Fixed: the supervisor settles a run with `Settle::Idle` (`supervisor.rs`),
  and Proton's idle wait returned at once, so a run could read `Stopped`
  while something umu handed off to still ran. Found in the C6 review;
  predates it. A Proton run now waits, with no timeout, for umu's process
  group to empty, like a native run (the maintainer approved a timeout, but
  one would end a game that runs for hours after a handoff; stops are still
  bounded by `stop_timeout`). `status` shows the program as exited only when
  something outlives umu. With no recorded identity the run still ends with
  umu, as decided by the maintainer. Members that leave the group, and the
  winetricks settle gap, are backlog rows in `features.md`.

## Sequencing

Track A first, supervisor before the app, each change with its numbers.
Track B interleaves, smallest and safest first. Track C items wait for
answers and land as amendments to this plan.

## Progress

Measured on release builds, same commands as the baselines.

- Supervisor during a game: about 100 wakeups a second to about 1. The
  signal thread blocks in `wait()` and the child waiter in `waitid`, in core
  and in the CLI's `Cancellation`.
- Attached `wineo run`: 50 wakeups a second to 0. `Token` wakes its waiters
  (a subscription and a Condvar) instead of being polled. Heartbeats now
  continue through a stop, so a run lost mid-stop keeps an accurate end.
- Desktop app, moving the mouse over the library (Xvfb, software
  rendering): 109% CPU to 60%. Cursor moves no longer produce a message;
  the right-click menu takes its position from the click. Spinners animate
  themselves only while on screen, without rebuilding the view, and
  "Force stop" gets its own wakeup at its deadline instead of riding the
  spinner's frame tick.

Shipped in 0.31.1.

- C1, the app's rescan (merged after 0.31.1): idle in the background went
  from 6.5% CPU (Xvfb) to 0%, with no rescans and no redraws. Focused, it
  still scans every 2 s but only sends a message, and so redraws, when what
  it read differs from what's on screen: 1.35% CPU. A profile of `wineo
  list` puts about 40% of a library read in the Steam `/proc` sweep and 7%
  in loading the time zone; trash reads are under 1% with an empty trash. Refocusing refreshes at once, and on
  X11 without a window manager a key or click counts as focus. While an Art
  page is open the scan also watches its files and saved game, and a failed
  SteamGridDB name lookup retries with backoff (4 s to 64 s).

- The Steam `/proc` sweep reads each process's command line first and only
  runs the full identity checks for Steam launches, and the system time zone
  loads only when a time is shown: `wineo list` 16.8 ms to 7.2 ms, `status`
  16.4 ms to 6.9 ms (merged after 0.31.1).

- Decoded library art lives in one cache with a 96 MiB budget, evicting art
  outside the visible rows (plus a 2-row margin, the range a panel
  animation sweeps through, and whatever the panel draws, including during
  its fade). A dimmed card keeps only its gray copy, grayscale runs off the
  UI thread, and a closed Art page drops its previews. On a 300-game copy
  that is about 210 MB less after scrolling (Xvfb). Visible change: a
  closing panel keeps its picked target's art through the fade instead of
  switching to the default target's.

- C2: while waiting for Steam to launch a game, `/proc` is scanned every
  250 ms for the first 10 s, then once a second.
- C8: the `windows_installer` update reason is gone from core, the CLI,
  the app, and the JSON docs.

- C11: releases build with full LTO and one codegen unit. Measured on
  main (40 runs each, real library; wall time is within noise since these
  commands are mostly I/O):

  | | No LTO | Thin | Full | Full, 1 unit |
  |---|---|---|---|---|
  | `wineo` | 17.0 MiB | 17.3 MiB | 14.9 MiB | 14.3 MiB |
  | `wineo-gui` | 43.0 MiB | 43.5 MiB | 35.4 MiB | 32.7 MiB |
  | `list` instructions | 11.28 M | 10.88 M | 10.63 M | 10.55 M |
  | `list` peak RSS | 12.0 MiB | 11.5 MiB | 10.6 MiB | 9.2 MiB |
  | `status` peak RSS | 11.8 MiB | 11.2 MiB | 10.2 MiB | 8.8 MiB |
  | Release build | 123 s | 152 s | 376 s | 374 s |

  Thin LTO came out larger than none, so it was never a candidate.

- The toast and the scrollbar fade wake once at their deadline instead of
  ticking every 100 ms and 250 ms. A hovered toast went from 10 wakeups a
  second, for as long as it was hovered, to none (12 s hovered in Xvfb: 0
  main-thread wakeups), with the same timing as before. The hook output
  reader blocks instead of polling every 5 ms: 192 wakeups a second to 0.
  The art picker awaits its decode slot together with cancellation
  (`Token::canceled()`) instead of polling every 10 ms. The scrollbar fade
  and the "what's new" notice now fire at their deadline, not up to 250 ms
  or 100 ms after it. Undo's focus is checked on a click, a tap, a key
  release, and when the toast's time runs out, instead of every 100 ms.

- A Heroic import plan parses Heroic's library and settings once instead
  of once per game (a 21-game bulk plan: 21 parses of each file to 1; 300
  games planned in 48 ms instead of 121 ms). Each game's fingerprint now
  hashes the same bytes its settings were parsed from, which also closes an
  older gap where a game's own config could change between being read and
  being fingerprinted. A bulk dry run of 300 fake games prints the same
  plan as before, verbose included. GOG builds its download client on the
  first download, so a login or listing builds one HTTP client instead of
  two. `validate` lists desktop entries once instead of once per
  environment (7 directory reads to 1 with 6 environments).

- The art picker builds one HTTP client per picker instead of one per
  thumbnail, stops opening the keyring for thumbnails (downloads never sent
  the key), and decodes thumbnails from memory instead of a temp file: per
  30 thumbnails, 30 keyring lookups, 30 clients, and 30 temp files to none,
  one, and none. The client is dropped off the UI thread.
- Views no longer touch the filesystem for action reasons. The two facts
  behind them (whether a restore is pending, and each prefix's resolved
  path) are read whenever what is on screen changes and when the library
  changes, and views read the stored copy; every action still checks the
  disk itself.

- The Heroic, Steam, and GOG import grids decode posters capped to the
  tile size, cached on disk beside the library's art, and at most 8 at a
  time (half the cores, 2 to 8) in their own pool, so the library keeps
  its 2. Before, a 300-game Heroic or Steam page started 300 full-size
  decodes at once. 300 large posters:

  | | Peak threads | Peak RSS | First open | Later opens |
  |---|---|---|---|---|
  | Before | 305 | 1079 MB | 1.2 s | 1.2 s |
  | After (8 at a time) | 13 | 153 MB | 2.1 s | 0.05 s |

  The maintainer picked 8 from measured 2, 4, 8, and 16. When posters are
  still decoding half a second after a page opens, a progress bar ("Preparing
  art... 34 of 300") shows in the pinned footer, with Import moved to the
  right of that row (approved). The bar is a reusable `progress` component.

- C5: clone, portable copy, snapshot create, and snapshot restore no longer
  inspect the whole prefix before taking their locks; the inspection under
  the lock and the check after copying stay, and nothing is written before
  the one under the lock. Full-prefix reads per operation: 3 to 2 (restore
  6 to 4). On a 376 MB, 3,000-file prefix: snapshot create 0.87 s to 0.63 s,
  restore 2.4 s to 2.0 s, clone 1.6 s to 1.4 s. When a review is stale and
  a lock also refuses, the lock's error now shows first, as native copies
  already did.
- `validate::Code` is generated from one table (one line per code), pinned
  by a test of every code's name, level, and area.

- Bulk imports build one HTTP client for the batch instead of one (or two)
  per game, in the CLI's Steam and Heroic imports and the app's Heroic
  import. A bulk GOG migration opens the GOG login once for its extras
  instead of once per game: N token refreshes and N account lookups to 1.
  `stats` reads the global config and Steam's account files once per
  overview instead of once per Steam game. `stats`, `list`, and `status`
  print the same output as before on the maintainer's library.

- C12: the CLI's guided Steam import, GOG migration, and prefix import use
  typed step enums like the new-environment wizard, instead of numbered
  steps. Prompts, order, and back navigation are unchanged, and new tests
  walk each flow forward and back.

- C3: one `list`, `status`, app library load, or `stats --tag` read checks
  the trash once instead of once per environment, keeping every check
  (each manifest is still fully validated). `wineo list` with 20
  environments and 50 trash entries: 43 ms to 4.5 ms. An app library load
  with 4 environments and 5 trash entries reads 5 manifests instead of 45.
  The Heroic playtime import, which writes play logs, still checks each
  environment as it goes.
- C7c: clone, portable copy, and prefix import stage, verify, and publish
  through one helper (`stage.rs`), with tests that a failed or cancelled
  copy, a failed rename, or a panic leaves nothing behind, and that a
  published prefix is never cleaned up. Snapshot create and restore keep
  their own staging.

- C7a: the app's Heroic, Steam, and GOG import pages share one import grid
  (`flow/import_grid.rs`: selection, keys, focus, scroll to reveal, poster
  bookkeeping, progress bar, footer) and one pinned step footer. Each page
  keeps only its own listing, rules, and steps. Screenshots of every page,
  wide and narrow, match the old build pixel for pixel. Lines roughly break
  even (+56): about 190 duplicated lines left the pages, and the shared
  grid's API added about the same.

- C7b: the app's guided flow pages dispatch through one small macro and
  `From` conversions instead of a hand-written match per method:
  `flow/mod.rs` 410 to 353 lines, per-page arms about 55 to 24. A trait
  was tried and dropped: the pages' methods take different arguments, so
  it would have added adapters without removing code.

- C6: one stop helper (`process::stop_group`, `Escalation`) replaces the
  five TERM-wait-KILL-settle loops in the supervisor, exec, winetricks,
  prefix creation, and hooks, and one `Identity` carries a run's process
  group, boot, and start time. The loops shrank by about 40% (hooks 53 to
  18 lines, prefix 52 to 23). The helper sleeps until the program exits or
  a stop is asked for, wakes only at stop deadlines, and falls back to
  polling only when it can't open a pidfd. It fixes two places that could
  signal a reused group number after reaping its leader, kills a group
  before running any cleanup that might fail, never reports a stop it
  couldn't confirm as complete, and gives up cleanup after 10 s instead of
  hanging on a process the kernel won't let die. The supervisor still
  wakes about once a second during a game.
- C7c follow-ups: import holds the install lock through registration like
  clone; clone's final check can be cancelled; portable copies say
  "portable copy destination appeared during review/publication" instead
  of a raw rename error.

- C9: the keyring goes through the zbus-based `secret-service` crate on a
  current-thread runtime made per call, so the app has one D-Bus stack and
  the build no longer needs libdbus. Same items, attributes, unlock prompts,
  2 s method timeout, and error texts; keys saved by either version read in
  the other. Threads match libdbus (CLI 1, app 55 after opening Settings);
  `wineo` grows from 15.0 to 16.9 MB.

- Track B, app: one `task` module replaces about 70 hand-written
  `spawn_blocking` wrappers (the rest hold permits, check cancel tokens, or
  fall back to a default on purpose); the last identity `map_err` joins
  are gone; the app's byte-size formatter is core's; one library reload
  guard instead of four copies. Net -258 lines.

- Track B, core: one copy each of hex encoding, one-line errors, the
  runner build lock for a Wine config, the pidfd and boot-id readers, and
  the GOG and Steam keyring constructors; `files::read_optional`,
  `remove_if_present`, `sync_dir`, and `persist_synced` replace about 30
  hand-written NotFound and fsync sites (variants whose order or errors
  differ were left as they are). Test-only hooks are out of release builds
  (`strings` finds no `WINEO_TEST_` in `wineo`), and core and the app share
  one scripted fake GOG server. About +450/-570 lines. `list`, `status`,
  `validate`, `stats`, `trash`, and `runners` print the same JSON as before
  on the maintainer's library.

- Track B, CLI: one helper each for cancellation and exit code 130,
  plurals and "X of Y games." summaries, prompt plumbing (`Ask`,
  `terminal()`, one no-input stub, one key save/remove flow), aligned
  label rows, and completion setup. Tables now pad by display width, so
  accented and CJK titles line up (plan bug fixed). The CLI's tables print
  the same on the maintainer's (ASCII) library.
- The PTY tests sent Ctrl+C right after a question's heading, before the
  line editor switched the terminal to raw mode, so the terminal could turn
  it into a signal and the test hung (about 1 run in 15). They now wait for
  the prompt itself, and their wait for exit is bounded.

- Track B, GOG logic into core: migrate and update review rows, recorded
  games, update reasons, labels, product names, and change lists, the
  progress reporter, and the extras result lines live in core; the CLI and
  app keep their own wording (lowercase vs sentence case) over the same
  data. The extras import driver stays per frontend: they look up the
  SteamGridDB key and word failures differently on purpose. 18 files,
  net -214 lines.

- Track B, import logic into core: `import::preflight` (the CLI's checks,
  now also used by the app and by `execute_reviewed`, checking a given
  destination where it resolves), `steam_import::{plan_batch,
  apply_batch}`, `SettingsReview::hooks()`, and `Runners::candidates()`
  replace copies in the CLI and app, with core tests for each. A 300-game
  Heroic dry run, `list`, `status`, `runners`, and `validate` print the
  same as before.

- Track B, tests: every inline test module over 300 lines moved to its
  own `tests.rs` (73 files across core, the CLI, and the app; test counts
  and names unchanged). The biggest files shrank most: the app's
  `settings/mod.rs` 6,049 to 2,529 lines and `main.rs` 4,582 to 2,096,
  core's `trash.rs` 5,206 to 2,847, the CLI's `import_heroic.rs` 4,000 to
  2,441.

- Track B, file splits (moves only, checked line for line): the CLI's
  `import_heroic.rs` 2,441 to 748 lines (bulk, review, runner, summary);
  the app's `flow/heroic.rs` 3,275 to 1,125 (update, view, work, and the
  shared page frame in `flow/frame.rs`), `settings/mod.rs` 2,528 to 991
  (editor, handle), and `main.rs` 2,095 to 1,703 (art, library, flow).
  Core's `trash.rs` 2,846 to 829 (restore, payload, remove, identity,
  digest, manifest, markers; every fsync stays in its function, 28 before
  and after) and `import.rs` 2,137 to 791 (runner copy, config text, copy,
  fingerprint). An end-to-end import, clone, trash, and restore on a
  throwaway root gives byte-identical prefixes.

Final release builds (full LTO, 40 runs, the maintainer's library),
against the installed build from the 0050 acceptance merge:

| | Before | After |
|---|---|---|
| `wineo list` | 16.6 ms, 11.9 MiB, 25.0 M instructions | 6.4 ms, 9.1 MiB, 9.3 M |
| `wineo status` | 16.0 ms, 11.7 MiB, 21.9 M instructions | 5.8 ms, 8.8 MiB, 6.3 M |
| `wineo` binary | 17.4 MB | 16.9 MB |
| `wineo-gui` binary | 44.2 MB | 34.8 MB |

The maintainer's machine (real GPU, library of 22, after scrolling the
library), the 0050 acceptance build against main with everything above:

| | Before | After |
|---|---|---|
| Background CPU | 1.6% | 0.0% |
| Background main-thread wakeups | 6.5/s | 1.2/s |
| Focused idle CPU | 1.7% | 1.0% |
| Focused main-thread wakeups | 5.9/s | 1.1/s |
| RSS, focused | 156 MB | 134 MB |
| Threads | 35 | 29 to 31 |

Limits accepted in review:

- A spinner inside a scrollable that is itself scrolled out of an outer
  scrollable can keep asking for frames: iced hands an inner scrollable its
  own viewport. No spinner sits in nested scrollables today.
- The trash page's "removed N s ago" labels update when their text
  changes, at most every 2 s, rather than every spinner frame.
- A wineo window that is visible but not focused stops updating until it
  is focused or clicked: "focused" means keyboard focus.
- An open, focused settings page still checks its file every 2 s.
- The Art page notices a replaced file by its modification time and size,
  so a same-size replacement that keeps the old time shows on reopening.
- A card scrolled back faster than the 2-row prefetch can show its title
  on the surface color until its art decodes, as a never-visited card does.
  On very large windows the protected art alone can exceed the budget; it
  is never evicted, so the app just keeps less earlier art.
- At most one periodic scan runs at a time, and an explicit reload makes
  scans read before it stale. A scan stuck on a hung filesystem holds that
  slot; there is no scan timeout.
- If Undo loses keyboard focus with no click, tap, key release, or other
  message (none is known to do this), the toast stays paused until the
  next one.
- Removing a runner build reads every environment's config twice: once to
  refuse an in-use build before taking any lock, and once under the locks.
  Both stay; dropping the first would let an in-use build take the install
  lock before being refused. The interactive CLI reads it a third time to
  show warnings before its prompt.
- Heroic's GOG-store files are still parsed and fingerprinted from separate
  reads, as before; only sideload files share their bytes.
- A restore started or a prefix symlink retargeted outside wineo, with no
  library change, shows in action reasons once what is on screen changes
  (a menu, dialog, page, panel, or result opening). Actions are unaffected:
  they check the disk when chosen.
- A bulk GOG migration reads the login once. If one of its games finds the
  login revoked, later games report "not logged in" as before; a login,
  logout, or credentials change made by another process during the batch
  takes effect on the next command.
- After a failed stop, cleanup waits at most 10 s for a killed group; a
  process stuck in uninterruptible sleep (a hung disk or network mount) is
  left to the kernel and the stop is reported as unconfirmed.
- Exec and hooks still wake every 50 ms and 20 ms while their program runs:
  exec streams output from a file and hooks read stop requests from a lock
  file, and neither has anything to block on.
- The new keyring crate doesn't wipe its session encryption key from memory
  when done, as the libdbus one did. The secret itself was never wiped by
  either.

Found during acceptance:

- `makepkg` run from a terminal hung and then failed two `art fetch` unit
  tests: they inherit the terminal cargo runs in, so a "no matches" result
  asked the real terminal for a new search. Since 0.7.1, not this plan.
  `prompt::interactive()` is now false in unit-test builds; the PTY tests
  run the real binary and still cover the prompts. The full PKGBUILD test
  command passes under a pseudo-terminal.
- `wineo trash` with an empty trash and `wineo list` with no environments
  printed nothing. They now print `Trash is empty` and `No environments.
  Create one with: wineo new` on stderr, like `status`'s `Nothing is
  running`, so piped stdout stays empty. A config that fails to load still
  lists as a row, so it never triggers the hint.

## Acceptance checklist

Passed on 2026-10-14.

Nothing should look different except where a step says so. Use a small
existing environment as SMALL; the steps make a throwaway copy, `accept51`,
so your real ones are only read. Pick a Steam game as STEAM and any other
game as GAME.

1. `wineo list`, `status`, `stats`, `validate`, and `trash` look as they
   did on 0.31.1 and come back noticeably quicker. If any title has
   accented or CJK characters, its columns line up.
2. `wineo clone SMALL accept51`, then `wineo snapshot create accept51
   before`. Change something in it (`wineo exec accept51 winecfg`, flip a
   setting), `wineo snapshot restore accept51 before`, and the setting is
   back. `wineo portable copy accept51 --to ~/wineo-0051-portable` copies
   it, and running the same command again says the destination exists.
3. Clone a bigger environment and press Ctrl+C partway through: exit 130,
   and neither `wineo list` nor the prefix folder shows anything left
   behind.
4. `wineo exec accept51 winecfg`, then Ctrl+C in the terminal: winecfg
   closes and the command exits 130. `wineo run GAME`, Ctrl+C: the game
   closes. `wineo start GAME`, `wineo stop GAME`: it closes and `status`
   says stopped. Start it again and `wineo stop --force GAME`: it dies at
   once.
5. `wineo start STEAM`: Steam launches it and `status` shows it running
   within about a second of the game appearing. `wineo stop STEAM` stops
   it.
6. Keyring (now zbus): `wineo art key`, paste your SteamGridDB key, and
   `wineo art fetch accept51` finds art. Lock the keyring (Seahorse, or log
   out and back in), run `wineo art key --remove`, and it asks you to
   unlock first; save the key again afterwards. `wineo import gog --list
   --refresh` still uses your saved GOG login.
7. App, idle: open it, then focus another window for a minute or two (it
   should sit at about 0% CPU in `top`). Meanwhile run `wineo rm accept51`
   in a terminal. Focus the app: the library drops `accept51` right away.
   Bring it back with `wineo restore` (`wineo trash` shows the entry).
8. App, art: scroll the library fast to the bottom and back; posters come
   back as you go (a card can show its title alone for a moment). Open a
   card's panel and close it: the art stays through the fade. Open the Art
   page for `accept51` and set new art from the terminal (`wineo art set`):
   the page picks it up.
9. App, import pages: **Add**, **Import from Steam** and **Import from GOG**.
   Posters fill in, a "Preparing art..." bar shows in the footer if they
   take a moment, and reopening a page is near instant. Selection, arrow
   keys, Ctrl+A, and scrolling work as before. Shrink the window and check
   both pages in light and dark themes if you use both.
10. App, prefix import (now stricter): **Add**, **Import prefix**, pick
    `accept51`'s prefix, choose adopt: the review refuses it, since
    `accept51` already uses that prefix, in the same words as `wineo import
    prefix --adopt`. It used to get further and fail later.
11. App, toasts: remove `accept51` in the app. The toast with **Undo**
    pauses while hovered and runs out about 10 s after you leave it.
    Remove it again and press **Undo**: it comes back.
12. If you use hooks, start something with a hook that prints output: the
    output shows as before.
13. Build and install the package (`makepkg -si`): it builds, and
    `pacman -Qi wineo` lists no `dbus` dependency.

Afterwards: `wineo rm accept51 --delete-prefix`, delete
`~/wineo-0051-portable`, and prune the test entries from the trash.
