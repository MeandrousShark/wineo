Status: approved

# 0034: GUI-facing core API review

## Goal

Make `wineo-core` something a GUI can drive directly, before the GUI exists.
A GUI calls core from a worker thread, shows progress, lets the user cancel,
and renders errors it can tell apart. It can't read a terminal, catch
Ctrl+C, or reimplement logic that only lives in the CLI. This is GUI entry
criterion 8.

This plan records the review and fixes what it found, except the Heroic
import move, which is big enough for its own plan (0035, see decision 1).
Nothing here changes CLI behavior or `--json` output. It's a refactor with
new core entry points, and the existing CLI tests are the safety net.

## Review findings

The review covered every CLI command and the core modules behind them. File
references are as of `56519b8`.

### What is already in good shape

- **Import, clone, portable copy, snapshots.** `import::execute_reviewed`
  and its siblings do the locking, re-inspection, staging, and publishing.
  Progress is `ImportProgress::{Started, Copied, Finished}` through `FnMut`,
  cancellation is an `&AtomicBool` checked per entry, and a cancel leaves
  only a staging directory that is removed on drop. The copy machinery
  (`copy_tree_with`, `validate_staged_prefix`, `ensure_same_review`,
  `write_config_once`, `StagedArt`) is already shared by all four.
- **Downloads.** `Progress::{Downloading, Verifying, Extracting, Installed,
  AlreadyInstalled}`, an `&AtomicBool`, and atomic publishing.
- **Validate.** `check_all` returns a structured `Report` of `Problem`s with
  stable codes, and `apply` runs one `Fix`. This is the model for the rest.
- **Art, snapshots, portable.** The CLI commands only print and prompt.
- Core never reads stdin (apart from the supervisor's own IPC), never calls
  `process::exit`, and never changes env vars or the working directory.
- Trash copies differently on purpose (journaled, digest-checked), and stays
  separate from the reflink copy path.

### Blockers: a GUI can't work around these

1. **The installer can only be cancelled by a signal.** `Installer::run`
   calls `Supervisor::listen()` (`installer.rs:210`, `supervisor.rs:104`),
   which catches SIGINT, SIGTERM, and SIGHUP for the whole process. A GUI
   would have to signal itself to cancel, and while an installer runs,
   SIGTERM no longer quits the GUI. The installer's child already runs in
   its own process group, so only the source of the stop request is wrong.
2. **Winetricks can only be cancelled by SIGINT, and it prints.**
   `winetricks::run` registers a SIGINT handler (`winetricks.rs:205`), keeps
   winetricks in wineo's process group so the terminal's Ctrl+C reaches it
   (`:227`), sends its output to wineo's stdout (`:198`), and core prints
   status lines with `eprintln!` (`:101`, `:292-317`). This covers `wineo
   winetricks`, the install before a launch, and validate's fix.
3. **Prefix creation can't be cancelled or followed.** `prefix::create`
   blocks on `.status()` (`prefix.rs:174`). The CLI can stop it only because
   wineboot and umu share its process group. A first Proton prefix can spend
   minutes downloading the Steam Linux Runtime with no progress. Cleanup of a
   half-made prefix is already correct (`environment.rs:476`).
4. **Launch, stop, and create are orchestrated in the CLI.** A GUI would
   have to copy the lock order and the checks around them:
   - `start` (`commands/start.rs:35-100`, `commands/build.rs:27-131`): build
     a missing prefix, lock, `ensure_exists` and `check_resolved`, reconcile,
     refuse a different running target, install winetricks verbs, build the
     `Spec`, then `supervisor::start`.
   - `stop` (`commands/stop.rs:70-150`): the busy check, lost-run cleanup,
     and installer cleanup, with a second copy of the cleanup in
     `validate.rs:1391-1426`.
   - `new` and `setup` create the same way twice (`commands/new.rs:78-306`,
     `wizard.rs:1090+`), including the Proton plus win32 check
     (`new.rs:114`, `wizard.rs:717-756`).
   - `wineo winetricks` (`commands/winetricks.rs:56-96`) and validate's fix
     (`validate.rs:1351-1388`) orchestrate winetricks twice.
5. **`set`'s rules live in the CLI.** `check_runner`, `check_native`,
   `check_desktop`, and `apply_components` (`commands/set.rs:20-257`) decide
   which keys a runner or native environment accepts and reconcile DXVK and
   vkd3d. Core's `settings::update` writes raw keys. A GUI settings page
   needs the same rules.
6. **The environment list query lives in the CLI.** `table::gather`
   (`commands/table.rs:108-158`) is the "every environment with its state,
   busy lock, unresolved installer, and tags" query behind `list` and
   `status`. A GUI library view is this exact query.

### Should fix: workable, but the GUI would duplicate logic

7. **The `--json` view types live in the CLI** (`commands/json.rs`, about 200
   lines). They're thin views over core types with no CLI dependencies. A GUI
   wants the same stable data model.
8. **Stats aggregation lives in the CLI.** The overview sort and totals
   (`commands/stats.rs:53-68`, `102-139`), the last session's length
   (`:168-179`), and `stats --import-heroic` for every environment
   (`:269-318`).
9. **Log tailing lives in the CLI.** `Reader`, `tail_offset`, `follow`, and
   rotation detection (`commands/logs.rs:97-242`, about 145 lines). A GUI log
   view needs the same semantics.
10. **Core prints in two more places.** `report.rs:114` warns about an
    unreadable `winetricks.log` with `eprintln!`, and validate's lost-run
    fix loses the "session could not be logged" warning because
    `Fix::apply` returns `Result<()>`.
11. **The pinned-build check for downloads** (`commands/download.rs:444-495`)
    belongs next to `Client::select`.

### Nice to have

12. **Errors a GUI can match on.** Most of core returns `wineo_core::Error`,
    but some cases are strings: "already exists" (`import.rs:274`), "download
    canceled" (`download.rs:657`), a GitHub rate limit (`download.rs:200`),
    "is running" (`validate.rs:1366`), and the CLI's own "already running"
    strings (`start.rs:60`, `run.rs:74`, `stop.rs:74`). `Error::Busy` names
    its holder with a `&'static str` instead of the `Operation`.
13. **Supervisor re-exec.** `supervisor::start` re-runs its `exe` with
    `__supervise`, so a GUI binary must dispatch that before anything else,
    like `main.rs:110`. A core helper makes that one line.
14. **Small duplication.** Three wrappers around
    `environment::sharing_prefix` with the same unreadable-config guard
    (`clone.rs:577`, `portable.rs:699`, `snapshot.rs:523`); five copies of
    the "Copied N entries" progress closure in the CLI; three copies of the
    external-link review block with `-v` truncation; and `file_stem` in both
    `picker.rs` and `wizard.rs`.

### Heroic import (for 0035)

`commands/import_heroic.rs` has about 3,600 lines of non-test code, plus 550
in `native.rs`. The runner pipeline is written out three times (direct,
guided, `--all`), there are three issue policies, the summary renderer
decides which issues are warnings, the apply sequence (source check,
register, playtime, art, desktop) appears six times, and `heroic_id` is
backfilled during planning, before the user confirms. A core
`heroic_import` module with a per-game plan, typed findings and warnings, and
a two-phase apply would move about 1,500 lines and delete a few hundred more.

## Decisions (settled with the maintainer, 2026-09-28)

The maintainer approved every recommendation below as written.

### 1. One plan or two

**Decided:** this plan covers findings 1-14. The Heroic import move gets
its own plan, 0035, drafted after this one lands. Criterion 8 is done when
both are.

The Heroic move has its own decisions (backfill timing, how the three issue
policies map onto one plan type, default name collisions) and touches none
of the same code.

### 2. How cancellation reaches core

**Decided:** a small `wineo_core::cancel::Token`, cheap to clone, with
two levels: `cancel()` (stop cleanly) and `force()` (kill now). This matches
today's first and second Ctrl+C. Every long operation takes `&Token`. The CLI
turns signals into token calls in one place, and core stops registering
signal handlers anywhere except the detached supervisor, which is its own
process.

Import, copies, and downloads move from `&AtomicBool` to `&Token` too, so
there's one way to cancel.

The alternative is keeping `&AtomicBool` and adding a second flag for force.
That's less code today but leaves two conventions.

### 3. How progress reaches the caller

**Decided:** keep one typed event enum per operation through `FnMut`,
like `ImportProgress` and download's `Progress`. New ones:
`PrefixEvent` (started, output line, finished), `WinetricksEvent` (installing
verbs, output line, stopping, killing, done), and `LaunchEvent` (building
prefix, installing verbs, starting, started). No shared event bus.

### 4. Where child output goes

**Decided:** wineboot, umu, and winetricks take the installer's existing
`Output` (inherit or a log file). The CLI keeps inheriting, so the terminal
looks the same. A GUI uses a log file and follows it with the tailer from
finding 9.

### 5. Process groups for winetricks and wineboot

**Decided:** both move into their own process group, like the installer
already is, and core signals the group when the token fires.

This is the one visible CLI change. Ctrl+C now goes to wineo, which asks the
child to stop, instead of the terminal hitting both at once. The result should
look the same, but it's worth trying by hand during acceptance.

### 6. Where the new entry points live

**Decided:**

| Entry point | Module | Replaces |
|---|---|---|
| `launch::start(paths, name, request, exe, &Token, events)` | `launch.rs` | `start.rs`, `build.rs` |
| `launch::stop(paths, name, force) -> StopOutcome` | `launch.rs` | `stop.rs`, `validate.rs` cleanup copy |
| `create::run(paths, request, &Token, events)` | new `create.rs` | `new.rs` and `wizard.rs` create steps |
| `winetricks::ensure(paths, name, &Token, output, events)` | `winetricks.rs` | `commands/winetricks.rs`, validate fix |
| `settings::check(...)` then `settings::update` | `settings.rs` | `set.rs` rules |
| `report::overview(paths, filter) -> Overview` | `report.rs` | `table::gather` |
| `view::*` | new `view.rs` | `commands/json.rs` |
| `stats::overview`, `stats::import_heroic_all` | `stats.rs` | `commands/stats.rs` logic |
| `log::Tail` | `log.rs` | `commands/logs.rs` reader |
| `supervisor::dispatch_if_supervise()` | `supervisor.rs` | `main.rs` hand-rolled check |

Foreground `wineo run` stays in the CLI. It holds the terminal, and a GUI
only ever uses the detached `start`.

### 7. Error types

**Decided:** add `Error::Canceled` and `Error::Network`, use
`Error::Running` and `Error::EnvironmentExists` for the string cases
in finding 12, and give `Error::Busy` an `Option<Operation>` holder. Modules
that return `anyhow` keep it, so a GUI downcasts the chain the way the CLI
already does. A full move off `anyhow` isn't worth it.

### 8. `view` and the JSON contract

**Decided:** the view types move to core unchanged, and
`docs/json-output.md` stays the contract. The GUI reads the same structs the
CLI serializes, so the two can't drift. Renaming or reshaping anything is out
of scope.

### 9. Validate's fixes report notes

**Decided:** `Fix::apply` returns `Vec<Note>` instead of `()`, so the
lost-run fix can say its session wasn't logged, as `stop` already does.

## Implementation sequence

Each step keeps every existing test green and is committed on its own.

1. **Small fixes.** `environment::checked_sharing`, the CLI progress and
   external-link helpers, `file_stem`, `report.rs` warnings as data,
   `Fix::apply` notes, the error variants, `Busy` holder,
   `dispatch_if_supervise`, and the pinned-build check in core.
2. **Cancellation and output.** `cancel::Token`, CLI signal wiring,
   `Output` and events for prefix creation and winetricks, their own process
   groups, the installer off `Supervisor::listen`, and import, copies, and
   downloads moved to `&Token`.
3. **Core orchestration.** `launch::start`, `launch::stop`, `create::run`,
   and `winetricks::ensure`, with `new`, `setup`, `start`, `restart`,
   `stop`, `winetricks`, and validate calling them.
4. **Read side.** `view`, `report::overview`, `settings::check`, stats
   overview and Heroic import, and `log::Tail`.
5. **Docs.** A short `docs/core-api.md` for the GUI: the entry points, the
   token, the event enums, errors, and the supervisor dispatch rule.

## Verification and acceptance

- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`
  pass, with the existing CLI integration tests unchanged. Their output is
  the behavior contract.
- New core tests cancel the installer, winetricks, and prefix creation with a
  token, from a plain thread and with no signal involved, using the fake
  runners. Each checks that partial state is cleaned up the same way it is
  today.
- A test fails if `println!` or `eprintln!` appears in `wineo-core/src`
  outside the supervisor.
- `--json` output for `list`, `status`, `stats`, and `validate` is
  byte-identical before and after on the same fixtures.
- Maintainer acceptance: a normal day with the CLI. Create a Wine and a
  Proton environment, press Ctrl+C once and then twice during wineboot, an
  installer, and a winetricks install, then start, stop, and `validate
  --fix`. Everything should look and behave like 0.14.0.

## Scope limits

- No GUI code, and no GUI crate.
- The Heroic import move is 0035.
- No CLI behavior or `--json` changes, apart from the Ctrl+C routing in
  decision 5.
- No move off `anyhow`.
