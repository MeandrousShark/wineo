# Core API for frontends

`wineo-core` is what the CLI is built on, and what a GUI will call directly.
This page covers the conventions every long operation follows and the entry
points a frontend needs. It was written for
[plan 0034](plans/0034-core-api-review.md).

## Conventions

**Threads.** Every operation below blocks until it's done, so a frontend runs
it on a worker thread. Nothing in core needs a terminal, reads stdin, or
changes the working directory or environment variables.

**Output.** Core never prints. A test fails if `println!` or `eprintln!`
shows up outside `supervisor.rs`, which is the detached supervisor's own
process. Warnings come back as data: event variants, `Note`s, or message
fields on an outcome.

**Progress.** Each long operation takes an `FnMut` callback with its own
event enum. Events arrive on the worker thread, in order.

| Operation | Events |
|---|---|
| import, clone, portable copy, snapshots | `import::ImportProgress` |
| component downloads | `download::Progress` |
| prefix creation | `prefix::PrefixEvent` |
| environment creation | `create::CreateEvent` |
| launch | `launch::LaunchEvent` |
| winetricks | `winetricks::WinetricksEvent` |
| installers | `supervisor::Watched` |

**Cancellation.** Long operations take a `&cancel::Token`. Clone it into the
UI and call:

- `cancel()` to ask for a clean stop, like a first Ctrl+C. Programs get asked
  to close, and a stop timeout still kills them if they hang.
- `force()` to kill now, like a second Ctrl+C.

A canceled operation cleans up after itself the way the CLI always has: a
half-built prefix is removed, a copy's staging directory is deleted, an
installer's record is resolved. Most return `Error::Canceled`, with the
operation's name in the message. Copies still return `import::Cancelled`,
which `clone`, `portable`, and `snapshot` re-export as `Canceled`.

Core registers no signal handlers. The CLI turns SIGINT, SIGTERM, and SIGHUP
into `cancel()` and SIGUSR1 into `force()`, and only while an operation that
honors the token is running.

**Child output.** Operations that run Wine programs take an
`installer::Output`: `Inherit` or `Log(path)`. A GUI passes a log file and
follows it with `log::Tail`. Prefix creation always writes
`<runtime dir>/create.log`.

**Errors.** Modules return either `wineo_core::Error` or an `anyhow::Error`
wrapping one, so `err.downcast_ref::<Error>()` finds the typed cause. The
variants a frontend usually handles:

| Variant | Meaning |
|---|---|
| `Canceled(what)` | The token stopped it |
| `Network(_)` | A download or API request failed, including rate limits. It keeps the underlying error's message and causes |
| `Busy { holder, .. }` | Another operation holds the lock. `holder` is the `Operation` when known |
| `Running { name, .. }` | The environment is running, so this can't touch it |
| `LostRun(name)` | wineo lost track of a run. `launch::stop` cleans it up |
| `DifferentTargetRunning { .. }` | Starting one target while another runs |
| `EnvironmentExists(name)`, `UnknownEnvironment(name)` | Name conflicts and typos |
| `ProtonWin32(runner)` | Proton prefixes are always win64 |

## The supervisor

`launch::start` runs the environment under a detached supervisor process,
which is the frontend's own binary started again with a hidden argument. So a
frontend's `main` has to hand that off before anything else:

```rust
fn main() -> ExitCode {
    if let Some(result) = wineo_core::supervisor::dispatch_if_supervise() {
        return if result.is_ok() { ExitCode::SUCCESS } else { ExitCode::FAILURE };
    }
    // ... start the GUI
}
```

Pass `std::env::current_exe()` as `exe`. A GUI uses detached starts only. The
CLI's attached `wineo run` holds the terminal and isn't meant for a GUI.

## Entry points

### Reading state

- `report::overview(paths, filter) -> Overview`: every environment with its
  run state, busy lock, unresolved installer, and tags, plus per-environment
  errors. This is the query behind `list` and `status`.
- `report::EnvironmentReport::load_with_warnings(...)`: one environment in
  detail, returned with any warnings.
- `stats::overview(paths)`: playtime for every environment, sorted and
  totaled. `stats::summary` and `stats::last_session_seconds` cover one.
- `view::*`: the serializable shapes behind `--json`, documented in
  [JSON output](json-output.md). A frontend can use the same structs.
- `validate::check_all(paths) -> Report`: problems with codes and levels,
  each with an optional `Fix`.
- `log::Tail`: reads a run's log, tails the last lines, and follows a live
  run into any `Write` sink until the run ends or the token is canceled.

### Changing things

- `create::run(paths, CreateRequest, &token, events) -> Created`: creates an
  environment, building or adopting its prefix, adding a default target, and
  reconciling components and desktop entries. `Created` holds the create
  lock, so keep it alive through an installer that runs right after.
- `settings::check(paths, global, name, pairs, unset)`: the rules `wineo set`
  enforces, run before `settings::update` writes. It returns the component
  changes it applied.
- `launch::start(paths, name, overrides, exe, &token, winetricks_output,
  events) -> StartOutcome`: builds a missing prefix, reconciles, installs
  missing winetricks verbs, and starts the target detached.
  `LaunchEvent::Starting` comes right before the supervisor handshake, which
  the token can't interrupt.
- `launch::stop(paths, name, force) -> StopOutcome`: stops a run, or cleans up
  a lost run or an unresolved installer. The outcome carries the messages to
  show.
- `winetricks::ensure(...)`: installs the environment's missing declared
  verbs. `winetricks::run_in(...)` runs any verbs.
- `validate::apply(paths, fix, &token, winetricks_events, download) ->
  Vec<Note>`: applies one fix.
- `Installer::run(&token, stop_timeout, output, watched) -> Outcome`: runs an
  installer until it and everything it started exit.
- `download::Client::install(..., &token, progress)`, plus
  `Client::select_pinned` for an exact DXVK or vkd3d-proton build.
- Copies: `import::execute_reviewed`, `clone::execute_reviewed`,
  `portable::execute_reviewed`, `snapshot::create_reviewed`, and
  `snapshot::restore_reviewed`. Each takes an inspected review, `&token`, and
  a progress callback.

## Not in core yet

- Heroic import still lives in the CLI's `import_heroic` command. Plan 0035
  moves it.
- `StopOutcome` carries ready-made message strings rather than typed
  warnings.
