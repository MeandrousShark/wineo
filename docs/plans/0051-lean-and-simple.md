Status: draft

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

Answered (2026-10-13): the maintainer took every recommendation. Items 2, 5,
and 8 to 12 had no recommendation written yet when that answer came, so
theirs were added afterwards and wait for a separate yes.

1. **The app's 2 s library rescan.** (a, recommended) Pause it while the
   window is unfocused and refresh on focus, and only redraw when the
   result changed. (b) inotify on wineo's folders, no timer. (c) Keep 2 s.
   **Answer: a.**
2. **Steam detection cadence.** The start phase scans `/proc` 4 times a
   second while waiting for Steam to launch a game. Slowing it to 1 s makes
   detection up to a second later. Recommended: keep 250 ms for the first
   10 s, when a launch usually lands, then back off to 1 s, since the wait
   can last minutes while Steam updates the game.
3. **The trash check in every config load.** `Environment::load` reads
   every trash manifest, so `list` costs environments x trash entries.
   Recommended: read only manifest headers, or memoize once per command.
   Integrity code, so it gets its own review. **Answer: recommended.**
4. **GOG package hashing.** Cached packages are MD5-hashed twice per update
   (on reuse, and again when pinned under the lock). Recommended: one full
   verify, under the lock. **Answer: recommended.**
5. **Content vs metadata fingerprints** for clone and portable copies, which
   read the whole prefix 3-4 times today. Recommended: keep content
   fingerprints, since they guard against data loss, and only drop the
   redundant passes (Track A).
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
9. **One D-Bus stack.** The app carries both zbus (iced) and libdbus (the
   keyring crate). A zbus-based keyring would drop libdbus and the `dbus`
   build dependency. Recommended: yes, as its own patch with its own
   review, since it handles secrets.
10. **HTTP stack.** reqwest's blocking client brings tokio and a thread per
    client into the CLI. A synchronous client is a bigger change.
    Recommended: keep reqwest. The CLI already runs `list` in 20 ms and
    12 MB, and one shared client per batch (Track A) removes most of the
    cost.
11. **LTO for release builds.** Smaller and faster binaries, slower release
    builds. Recommended: yes, picking thin or full LTO by measured size
    and speed.
12. **Guided flows** are written four ways (linear, two numbered step
    machines, a typed step enum). Pick one pattern. Recommended: the typed
    step enum, since the compiler checks every step is handled. Flows move
    to it one at a time, not in one rewrite.

## Possible bugs (fix as patches if confirmed)

- The Art page re-reads and re-decodes every preview every 2 s, flashing
  "Loading...".
- `list`, `status`, and the app's overview clean up a lost Steam run from a
  read path (`report.rs:481`). Possibly intended.
- CLI tables pad by bytes, so non-ASCII titles misalign
  (`table::render_columns`, `render_bulk_table`).
- The app's `recorded_games` lacks the CLI's portable guard (harmless today).
- Two test fake-wine processes from an earlier test run were still alive
  11 hours later: a fixture misses cleanup.

## Sequencing

Track A first, supervisor before the app, each change with its numbers.
Track B interleaves, smallest and safest first. Track C items wait for
answers and land as amendments to this plan.
