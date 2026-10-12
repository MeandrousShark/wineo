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
| frontend exec | `launch::ExecEvent` |
| frontend snapshots | `snapshot::SnapshotEvent` |
| frontend portable copies | `portable::CopyEvent` |
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
  each with an optional `Fix`. `validate::check_environment(paths, name)`
  checks one. `Report::fixes()` lists each fix once, in report order, so
  two environments missing the same build share one download.
- `log::Tail`: reads a run's log, tails the last lines, and follows a live
  run into any `Write` sink until the run ends or the token is canceled.

### Changing things

- `create::run(paths, CreateRequest, &token, events) -> Created`: creates an
  environment, building or adopting its prefix, adding a default target, and
  reconciling components and desktop entries. `Created` holds the create
  lock, so keep it alive through an installer that runs right after.
  An optional `expected_adopted` checks the reviewed build/adopt decision
  under that lock and refuses a changed destination before writing.
  `target_name` gives the default target a display name, which becomes its
  desktop entry's `Name=`; `Environment::create_native_titled` does the same
  for native games.
- `name::from_title(title)`: derives a valid environment name from a game
  title, capped at `name::TITLE_MAX` bytes. It can return an empty string
  for titles with no letters or digits, so a frontend still validates it.
- `art::set_manual_as(paths, env, target, kind, file, original)`: stores a
  file as manual art but records `original` as its source name, for art
  that came from somewhere other than a file the person picked.
- `settings::check(paths, global, name, pairs, unset)`: the rules `wineo set`
  enforces, run before `settings::update` writes. It returns the component
  changes it applied.
- `settings::apply(paths, file, pairs, unset) -> Applied`: one whole
  settings change, as `wineo set` makes it. It locks when the launch changes,
  checks, writes, and reconciles desktop entries. `Applied` holds the
  component changes, unset keys that weren't set, desktop changes, the new
  runner, and whether the environment is running. An error means nothing
  changed. A failure after the checks passed comes back in `failed`, with
  the other fields holding whatever completed first. That can be no
  component changes at all, or a config that was already written.
- `launch::start(paths, name, overrides, exe, &token, winetricks_output,
  events) -> StartOutcome`: builds a missing prefix, reconciles, installs
  missing winetricks verbs, and starts the target detached.
  `LaunchEvent::Starting` comes right before the supervisor handshake, which
  the token can't interrupt.
- `launch::stop(paths, name, force) -> StopOutcome`: stops a run, or cleans up
  a lost run or an unresolved installer. The outcome carries the messages to
  show.
- `launch::exec(paths, name, program, args, env, &token, events) -> ExitStatus`:
  validates explicit paths, builds a missing prefix, reconciles, and runs an
  arbitrary program. Shared launch and build locks last until the immediate
  command exits. Output arrives as `ExecEvent::OutputLine`; nonzero and
  canceled exits are statuses. Cancellation signals only its process group.
  Wine handoffs may survive, and a frontend crash leaves the program running
  while releasing locks. It doesn't record playtime or install declared verbs.
  `Launch::exec` retains CLI cwd-relative resolution; frontend
  `ExecResolution::ExplicitPaths` never consults the current directory.
- GOG game folders: `launch::start` and `launch::exec` refuse a launch whose
  game folder is mid-update, being updated, or has lost its state, before
  any hook runs, and hold the folder's in-use lock for the whole run, so
  updates refuse while it runs. `start` hands the lock to the supervisor.
  An `exec` from a game folder holds it, and its launch and build locks,
  until the prefix is idle by the rule a `start` run ends on, so a launcher
  that starts the game and exits still counts, and only then sends
  `ExecEvent::Exited` with the started program's status. Canceling during
  that wait asks the prefix's programs to close, and forces them after the
  stop timeout. If the frontend is killed during the wait, the locks are
  released and the idle waiter keeps running until the prefix is idle,
  holding nothing. The refusals are
  `gog::folder::FolderError` variants: `MidUpdate`, `OwnerlessMidUpdate`,
  `Updating`, `StateMissing`, `UnreadableFolder`, `Unreadable`, and
  `OwnedByOther`. Canceling while waiting for a folder's lock returns
  `Error::Canceled`. A frontend that runs its own launch flow calls
  `launch::precheck` before side effects and holds the guards from
  `launch::protect`, taken after the operation lock, for the whole run.
- `winetricks::ensure(...)`: installs the environment's missing declared
  verbs in an existing prefix. `ensure_in(...)` builds a missing prefix first,
  then calls the locking `ensure`; its events are `EnsureEvent`.
  `winetricks::run_in(...)` runs arbitrary arguments, or opens winetricks with
  empty arguments. Both preserve declarations and enforce exclusive idle checks.
- `validate::apply(paths, fix, &token, winetricks_output, winetricks_events,
  download) -> Vec<Note>`: applies one fix. Pass
  `EnsureOutput::Log(path)` to get the `LogStarted`, `Progress`, and
  `OutputLine` events a launch reports. `Stderr` prints winetricks output
  instead and skips those three.
- `validate::apply_single(...)`: explicitly applies one fix, including
  `Fix::RecoverSnapshot`. Snapshot rollback is non-cancellable, must be
  confirmed on its own, and is excluded by `Report::fixes()` and refused by
  batch-oriented `apply`.
- `trash::remove_with(paths, name, include_prefix, &token, events) -> Entry`:
  moves an environment to the trash, like `wineo rm`. The token cancels the
  wait for wine to exit, which comes before anything is written, so a
  canceled removal changes nothing. `RemoveEvent::Waiting` says wine is
  still up; `RemoveEvent::Moving` comes once, right before the first write,
  and after it the token is ignored. `trash::restore`, `trash::review_prune`
  with `trash::prune_reviewed`, and `trash::recover` cover the rest.
- `Installer::run(&token, stop_timeout, output, watched) -> Outcome`: runs an
  installer until it and everything it started exit.
- `download::Client::install(..., &token, progress)`, plus
  `Client::select_pinned` for an exact DXVK or vkd3d-proton build.
- Copies: `import::execute_reviewed`, `clone::execute_reviewed`,
  `portable::execute_reviewed`, `snapshot::create_reviewed`, and
  `snapshot::restore_reviewed`. Each takes an inspected review, `&token`, and
  a progress callback.
- Frontend snapshots use `inspect_create_with` / `inspect_restore_with` for
  cancellable tree checks, then `create_reviewed_with` / `restore_reviewed_with`.
  `SnapshotEvent::Finishing` marks the start of non-cancellable publication or
  swap. `inspect_recovery` validates a marker without rolling it back;
  `recover` performs the rollback. Delete and recovery finish before close.
- Frontend portable copies use `inspect_with` / `inspect_native_with`, then
  `execute_reviewed_with` / `execute_native_with`. `CopyEvent::Progress` carries
  checking/copy progress, including native art and templates. `Publishing`
  marks the non-cancellable boundary. Late cancellation preserves success.
  The older entry points remain available for CLI callers.
- `Runner::wait_idle_for_copy(prefix, &token)`: waits for wineo-tracked Wine
  processes to exit before a GUI starts a prefix copy. Canceling this wait
  cancels only the waiter; it does not stop those processes.

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
which takes no token since nothing is copied. `NativePlan` has setters for
the name, executable, working directory, launcher arguments, environment
variables, gamemode, MangoHud, Gamescope, and
desktop entry to support an editable GUI review.

A whole library, for `--all`:

- `sources` and `listing` find Heroic's configs and list every game. Each
  `HeroicCandidate` carries `poster`, Heroic's cached copy of the game's
  poster when it exists (`HeroicSource::cached_image(url)` resolves the
  cache path). Listing never touches the network.
- `plan_selection(paths, global, &listing, &selected, runner, &options)` plans
  only the chosen games; unchosen ones still count for shared-prefix
  warnings.
- `store_poster(paths, env, app, poster)` stores a Heroic poster as Grid art
  named `heroic-<app>.<ext>`. It checks only the file signature, so callers
  must pass a poster they already decoded.
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

### GOG migration

`gog::migrate` moves a GOG game Heroic installed into wineo. It works only
in the host root, needs no GOG login, and never edits Heroic's files.

- `HeroicSource::gog_installed()` lists Heroic's installed GOG games
  (`HeroicGogGame`: id, title, platform, folder, version, language, build,
  and DLC), with unusable entries as `issues`. `gog_game(id)` returns one,
  or None when Heroic hasn't installed it.
- `plan(paths, &source, gog_id) -> Plan` only reads and takes no locks. It
  returns `NotInstalled`, `AlreadyMigrated { environment, folder }`, or
  `Migrate(Migration)`. A `Migration` says which environment the game
  attaches to or creates (`target()`: `Existing`, `Windows`, or `Native`),
  what happens to the folder's state (`state()`: `Adopt`, `Reclaim`, or
  `Keep`), and the state a first adoption commits (`committed()`). A new
  Windows environment needs a runner first, through `plan_runner`; its
  prefix is always adopted in place. `game_plan_mut` and `native_plan_mut`
  edit a new environment like a Heroic import.
- `apply(paths, global, &source, &migration, &token, progress) -> Outcome`
  checks everything again under the locks and writes `gog_id` into the
  config before the folder's state. It returns `AlreadyMigrated` or
  `Migrated`. For a new environment, call `registered.finish(paths,
  global)` afterwards to write desktop entries.

The refusals are `MigrateError` variants, plus `FolderError`'s `Overlap`,
`OwnedByOther`, and `InUse`. `Running` means the environment it attaches to
is running: apply holds its operation lock, so no launch starts meanwhile.
A game started another way (from Heroic, a clone, or another environment)
before migrating isn't seen. `Changed` carries which one changed since planning (`Changed::Heroic`,
`Folder`, or `Environment`); plan again.

After a migration, `gog::extras::import(paths, session, &request) ->
Report` adds GOG's playtime and art. With a session, the environment's
imported playtime baseline becomes the larger of what it had and GOG's
total, so running it again changes nothing; logged out, playtime is
skipped with a note. Art fills only kinds the environment has none of,
from GOG's own images (recorded with source `gog`), falling back to
SteamGridDB for a kind GOG lacks when a key is set. `Request` names the
environment, game id, title, the art kinds to fetch, a download transport,
and the SteamGridDB key and language. Every GOG or network problem is a
note in the `Report`; the call fails only for a portable root, an unknown
environment, or one whose `gog_id` is another game, all checked before any
request. Nothing is ever posted to GOG.

### GOG updates

`gog::update` compares installed GOG games with what GOG offers now. It
needs a logged-in `account::Session`, works only in the host root, and
reads product details through `gog::product::load`, so they're cached for
an hour unless `refresh` is set.

- `check(&mut session, name, refresh) -> Status` checks one environment.
  `check_all(&mut session, refresh) -> Batch` checks every environment
  that records a `gog_id`, sorted by name, and collects per-game problems
  in `errors` (`GameError`: environment, id, message) instead of stopping.
  It fails as a whole only for a portable root or an unreadable folder
  directory.
- A `Status` has the environment, game id, title, folder, platform, track,
  language, `installed` and `available` (versions, or builds on the Galaxy
  track), `state` (`UpToDate`, `Available`, `OnGog`, `CantCompare`, whose
  `as_str()` is the JSON `update` value), `applicable` (true only for a
  native game with an update), `reason`, `unfinished` (a journal is
  present, so the game needs finishing before it launches), `warnings`,
  and `dlc` (`DlcStatus` per DLC, with `Unknown` for a migrated DLC whose
  version Heroic never recorded).
- Status comes from the last committed state even when a journal is
  present. A Galaxy-tracked game whose `goggame-<id>.info` shows a newer
  build from another tool has that build recorded, under the folder's
  state lock and only if nothing changed meanwhile, with a warning. That
  is the only write a check makes.
- `UpdateError::NotTracked` and `Unloadable` refuse an environment with no
  `gog_id` or a config that can't be read. A lost or unreadable state file
  is an error naming it.
- An environment whose game folder has no owner (a crash between a trash
  restore's config and state writes) is found through its targets' paths
  by `folder::game_folder`, the same lookup launches use, and reported
  normally without claiming it. Two such folders refuse with
  `FolderError::SeveralFolders`.

Updating a native game, reinstalling it, or finishing an interrupted one
goes through `gog::update::plan` and `apply`:

- `plan(paths, session, name, Options { reinstall, refresh }, &token) ->
  Plan` reads only. It returns `UpToDate` or `Update(Pending)`, which says
  what will happen (`kind`, `is_recovery`, `is_offline`, `claims` for an
  ownerless folder), each product's installed and new version
  (`products()`), the download size, and an unpacked estimate (labeled as
  one: GOG publishes no unpacked size). A folder with a journal is always
  `Update`, finishing the journal's own kind. `session` may be None only
  for recovery whose cached packages all verify.
- `apply(paths, session, &pending, &token, progress) -> Updated` downloads
  and verifies every package holding no lock, so the game can be played
  meanwhile, then takes the owner's operation lock, the in-use lock, and
  the state lock, rechecks the plan, writes the journal, stages, publishes,
  syncs, and commits. `Progress` reports download, verify, stage, and
  publish steps. `Updated` lists what was saved aside (your files at new
  paths, as `.wineo-saved`), kept (old files you changed), and overwritten
  (shipped files you changed, and on a migrated game's first rebuild, any
  file that differed from the new one).
- Windows games refuse (`UpdateError::WindowsInstaller` names plan 0050,
  `Galaxy` plan 0051), as do `install` and `repair` journals from later
  releases (`LaterRelease`). `Running` means the environment is running,
  `Changed` that the folder changed since planning, and
  `Canceled { journal }` says whether a journal was left for recovery.
  Wording is placeholder until the CLI settles it.
- Downloaded packages live in `Paths::gog_download_cache()` and are removed
  after the commit unless another folder's journal still names them.

A folder in use refuses an update or migration with `FolderError::InUse`,
which names the environments running from it (`rimworld-copy is running
from this game folder. Stop it before updating rimworld.`). Launches leave
a record naming their environment beside the folder's in-use lock for as
long as they hold it. When none can be named, the refusal says `Something
is running from rimworld's game folder. Stop it first.`

## Not in core yet

- `StopOutcome` carries ready-made message strings rather than typed
  warnings.
