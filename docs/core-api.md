# Core API for frontends

`wineo-core` is what the CLI is built on, and what a GUI will call directly.
This page covers the conventions every long operation follows and the entry
points a frontend needs. It was written for
[plan 0034](plans/0034-core-api-review.md), and
[plan 0035](plans/0035-heroic-import-in-core.md) added Heroic import.

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
| import, Heroic import, clone, portable copy, snapshots | `import::ImportProgress` |
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

### Heroic import

`heroic_import` holds every decision `wineo import heroic` makes. Plans only
read, so a frontend can build, edit, and rebuild them freely. Nothing is
written until `apply`.

One game:

1. `plan_game(paths, global, &source, id, runner, options) -> GamePlan`
   inspects a Windows game and plans it against a runner. `suggest_runner`
   picks one: use an installed runner, copy Heroic's build into wineo, or
   let the person choose.
2. `plan.resolve(Policy::Strict)` for a direct import with flags, or
   `Policy::Defaults` for a guided one. It labels each `Finding` with a
   `Resolution` rather than failing. Then `blocking()` lists what stops the
   import, `needs_input()` says what a person has to supply (today only
   launcher arguments Heroic stored unparsed), and `warnings()` returns
   typed `Warning`s.
3. The setters (`set_name`, `set_executable`, `set_launcher_args`,
   `set_copy`, `set_desktop_enabled`, and the rest) apply a person's edits
   and return a typed `PlanError` when a value is wrong.
4. `missing_build` reports a DXVK or vkd3d-proton build the game needs that
   isn't installed, so the frontend can offer the download.
5. `apply(paths, global, &source, &plan, &token, progress) -> Registered`
   copies or adopts the prefix, writes the config, and carries over Heroic's
   playtime. It refuses if Heroic's settings changed since the plan was made.
   Then `registered.finish(paths, global)` writes desktop entries.

Native games follow the same steps with `plan_native` and `apply_native`,
which takes no token since nothing is copied.

A whole library, for `--all`:

- `sources` and `listing` find Heroic's configs and list every game.
- `plan_library(paths, global, &listing, runner, &options) -> LibraryPlan`
  sorts each game into ready, pending a build download, skipped with a
  `SkipReason`, or already imported.
- `record_backfills(paths)` writes `heroic_id` into environments that were
  imported before wineo recorded it. It's separate so a frontend decides when
  that happens. The CLI calls it right after planning, even if the user then
  cancels, since it only helps matching next time.
- `pending_builds()` lists the downloads to offer, `settle` moves games whose
  builds arrived to ready and skips the rest, and `into_ready()` returns the
  games to apply.

`SkipReason` and `Warning` carry no wording. The CLI keeps its own text, such
as "Proton runner, needs --runner", and a GUI shows its own.

## Not in core yet

- `StopOutcome` carries ready-made message strings rather than typed
  warnings.
