Status: draft

# 0020: Explicit lock release

## Goal

Release every wineo `flock` explicitly instead of relying on the file closing,
so a lock is free the moment its holder lets go. This fixes a real race that
shows up as flaky tests today and would show up as spurious "busy" errors in a
multi-threaded client like the planned GUI.

## Evidence

- `download::tests::installation_is_verified_atomic_and_idempotent` and
  `runtime::tests::check_quiet_refuses_runs_lost_runs_and_unresolved_operations`
  fail intermittently in full parallel runs. The download failure is
  `another install is using this destination` with `EAGAIN` on the second of
  two back-to-back installs into the same private temp dir.
- A `/proc` scan at the moment of failure found no other holder of the lock
  file, so the holder existed only for a moment.
- Running only `download::` tests: 0 of 25 runs failed. Running them next to
  the process-spawning tests (`supervisor::`, `process::`, `runtime::`,
  `runner::`, `installer::`): 21 of 25 runs failed, across six lock tests in
  `download` and `runtime`, including `probes_never_mistake_each_other_for_a_run`.
- Adding `flock(Unlock)` to `InstallLock`'s drop, as an experiment, took the
  `download::` failures in the mixed run to 0 of 25. The `runtime::` failures
  remained, since those locks were not changed.

## Cause

A `flock` belongs to the open file description, not to one fd. Closing an fd
only releases the lock when it was the last fd for that description. When any
thread spawns a child, the child gets copies of every open fd until its
`exec` closes the `O_CLOEXEC` ones. If a lock is dropped during that window,
the child's copy keeps it held, and an immediate non-blocking re-acquire
fails. An explicit `flock(Unlock)` releases the lock on the description itself,
whatever copies still exist.

The single-threaded CLI does not hit this, because `Command::spawn` returns
only after the child has exec'd. The test runner and any future multi-threaded
client do.

## Proposed decisions for maintainer approval

- Every lock holder unlocks explicitly on drop, the way `log.rs`'s `Held`
  already does:
  - `download::InstallLock`
  - `operation::Guard` and `operation::ConfigGuard`
  - `runtime::RunLock`
- The momentary shared-lock probes (`runtime::Runtime::is_running` and
  `operation::probe`) unlock explicitly before their file closes.
- `RunLock` and `Guard` keep their existing drop work (clearing the recorded
  holder) and unlock last, so no one can take the lock and then have its
  record cleared.
- `Guard::relabel` keeps handing locks over in place. It already keeps the same
  files, so the new drop behavior does not open a gap there; a test pins this.
- Keep today's non-blocking semantics, messages, and the existing short retry
  loops (`LOCK_ATTEMPTS`, `LOCK_RETRY`). Those retries cover a real case, a
  probe in another process briefly holding its shared lock. No new retries.
- Crash behavior is unchanged: if a holder dies, the kernel still drops its
  locks.

## Implementation sequence

1. Add a small shared helper or pattern for "unlock, ignore errors" and use it
   in each drop and probe listed above.
2. Add a deterministic regression test per lock type: take the lock, duplicate
   its fd with `try_clone` to stand in for a forked child's copy, drop the
   holder, and check that a fresh open can take the lock at once. Without the
   fix this fails every time.
3. Add one stress test that spawns short-lived children on other threads while
   a lock is dropped and retaken in a loop, as a check on the real mechanism.

## Verification and acceptance

- The deterministic tests fail on the current code and pass with the fix.
- Loop the mixed filter (`download:: supervisor:: process:: runtime::
  runner:: installer::`) and the full core suite at least 25 times each with no
  failures.
- `Guard::relabel` still hands locks over without a gap, and a crashed holder
  still leaves no stale lock.
- Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo
  test`.

## Scope limits

This changes how locks are released, not what is locked or when. It does not
add blocking waits, change lock file locations, or change error messages.
