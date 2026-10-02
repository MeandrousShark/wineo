Status: approved

# 0041: GUI snapshots, portable copies, exec, and winetricks

## Goal

Bring snapshots, portable copies, arbitrary programs in a prefix, and
on-demand winetricks to `wineo-gui`. Call `wineo-core` directly, as the
existing iced pages do. Keep CLI process and prompt code out of the GUI.

Reuse the detail actions, per-environment pages, review dialogs, running
changes, progress, logs, toasts, and Checkup. Layout stays plain until the
later polish pass. Copy stays neutral: "Run program", "Copy to portable folder...",
and "Stop it first". Key hints use full verb phrases.

## Settled by the maintainer

- On 2026-10-01, split the old 0040 scope: 0040 covers art and downloads;
  0041 covers snapshots, portable copies, exec, and winetricks.
- On 2026-10-02: rolling back an interrupted restore only happens when asked
  for by itself. It is never part of a batch, so `wineo validate --fix` and
  Checkup's fix-everything action skip it.
- On 2026-10-02 the maintainer approved the plan with every recommended
  answer below: 1A, 2A, 3A, 4A, 5A, 6A, 7A, and 8B. Step 4 is dropped.
- Exec lifecycle under 5A (2026-10-02): Cancel stops only the process
  wineo started, as its own process group. Programs it hands off to Wine
  keep running, and the result says "Other programs may still be running in
  the prefix." when the run exits. Locks last until that process exits. If
  the GUI dies, the program keeps running and the locks are released with
  the GUI's process. Closing offers **Keep working** or **Cancel and close**.

## Existing foundation

- `snapshot::{inspect_create, create_reviewed, list, inspect_restore,
  restore_reviewed, delete, recover}` cover the snapshot commands. Reviews
  carry prefix and snapshot paths, layout, runner, external links for
  create, and current/saved config text and runners for restore. Lists are
  ordered by label. Create and restore require an unshared Wine prefix,
  strict reflinks, operation/config locks, and a quiet runtime. They
  recheck the inspected state before changing anything.
- Snapshots contain the configured prefix root and exact config text,
  including Proton compatdata and legacy TOML. They exclude art, playtime,
  runners, external files, and host state. Restore preserves playtime and
  keeps art for targets a saved config drops. `RestoreOutcome` can name an
  old Btrfs subvolume that must be removed manually.
- Restore stages first, then writes a durable recovery marker and swaps
  the prefix/config. Cancellation is checked during copying and before
  the final checks, not during the swap and cleanup. `delete` and `recover`
  have no token or progress callback. `check_recovery_marker` refuses
  ordinary operations until recovery; `Paths::restore_marker` locates the
  marker even in a portable folder. `validate::Code::RestoreInterrupted`
  currently has no `Fix` and points to the CLI recovery command.
- `portable::{inspect, execute_reviewed}` copy Wine environments;
  `inspect_native` and `execute_native` copy native registrations. Both
  require a standard-layout source. Wine copies include prefix, snapshots,
  art, and playtime. Native copies include registration, art, and playtime,
  while executable files stay external. Both disable desktop entries.
  The first copy can bring global config and templates; an existing
  destination config directory keeps its own. Reviews expose the changes,
  external paths, and, for Wine, `Storage::{Reflink, FullCopy, Unknown}`.
- Portable copies stage and write the environment config last, refusing
  collisions and overlap. Wine copies report `ImportProgress` and take a
  token; native execution takes a token but has no progress callback.
  Full copies are allowed when reflinks are unavailable. Snapshots carried
  to that destination still need reflinks for future create/restore.
- CLI snapshot and Wine portable commands wait for wine before inspection.
  Core reviewed execution does not provide that wait itself. The GUI clone
  worker already resolves and holds the runner build lock, calls
  `Runner::wait_idle_for_copy`, then inspects. Reuse that pattern.
- `Launch::{check_exec, exec}` build arbitrary Wine/Proton commands, with
  Windows paths, Unix paths, bare names such as `winecfg`, and MSI handling.
  Exec gets environment variables but no target launch wrappers, Wayland
  setting, or game/store protonfixes. It does not install the declared
  winetricks list. Core already has the preparation pieces:
  `launch::{runner_locked, ensure_prefix, reconcile}`. CLI exec
  (`wineo-cli/src/commands/exec.rs`) only sequences them, takes the
  `Operation::Launch` lock, and spawns and waits attached to the terminal.
  `Operation::Launch` is shared, so CLI exec can coexist with a tracked
  application.
- `Launch::exec` resolves programs relative to the process's current
  directory: a bare name like `setup.exe` counts as a Unix path when a file
  with that name exists there (`is_path` in `launch.rs`). That suits a
  terminal, but a GUI's current directory is wherever it was started.
- `Installer` already provides exclusive, recorded runs with logs,
  cancellation, and waiting for descendant Wine programs. Its semantics
  differ from exec; using it for every GUI program would change behavior.
- `winetricks::run_in` already builds a missing prefix, locks it exclusively,
  refuses running/lost/unresolved state, reconciles components and desktop
  entries, and takes overrides, arbitrary arguments, a token, log output,
  and `EnsureEvent`s. Empty arguments open winetricks' own window. Proton
  uses umu's copy; Wine needs winetricks on PATH.
- `winetricks::ensure` installs missing declared verbs with detailed
  progress. `installed`, `missing_for`, and `validate_verb` cover the
  record and verb shape. Arbitrary `run_in` emits log lines and lifecycle
  events but not the requested-verb step progress that `ensure` emits.
  Removing a declared verb does not uninstall it. Existing Settings chips
  already edit the list; Play and Checkup already install missing verbs.
- GUI `removal::Change`, `management_busy`, close prompts, `logview`,
  `picker`, and the clone/Art pages provide the presentation patterns.
  Downloads occupies Ctrl+4. The current GUI root comes from `Paths` and
  `WINEO_PORTABLE_DIR`; the GUI has no root picker or `--portable` flag.

## Open questions

Settled with the recommended answers (see above). The other options stay for
the record. Each pair shows the same task.

### 1. Where do snapshots live?

**Recommend A:** a sidebar page with an environment picker, plus a detail
action that opens it with that environment selected. This follows the
prefix-management direction of 0039 without making a second browser.

```
A. Shared page                         B. Per-environment page
Sidebar: Snapshots                     Library > notes > Actions
Snapshots  [Environment: notes v]       [Snapshots...]
before-update  [Restore] [Delete]       [< Back] Notes snapshots
[Create snapshot...]                   before-update [Restore] [Delete]
                                       [Create snapshot...]
```

A makes recovery and browsing discoverable across environments. B matches
Art and Settings, with fewer sidebar entries. Neither option discovers
orphaned snapshots by crawling disks; only registered environments in the
active root are available.

### 2. How much restore review, and where is recovery offered?

**Recommend A:** compact confirmation with expandable saved/current config,
and recovery offered both on Snapshots and as a Checkup fix. Recovery is
core's rollback, never an attempt to finish the new restore. Per the
settled rule above, rollback only runs when asked for by itself.

```
A. Compact review                      B. Dedicated review page
Restore notes from before-update?      [< Back] Restore notes
Prefix: ~/prefixes/notes               Current          Saved
Runner: wine-tkg -> wine-tkg            wine-tkg         wine-tkg
Config: restore settings and targets   settings/targets settings/targets
[Show config changes]                  [Current config] [Saved config]
External files stay in place.          [Cancel] [Restore]
[Cancel] [Restore]                     Recovery: Snapshots > [Recover]
Recovery: Snapshots or Checkup
          > [Roll back restore]
```

A avoids a full page for unchanged config and gives Checkup a working fix.
That fix is single-item only: Checkup shows its own **Roll back restore**
button, and batch fixing, in Checkup and in CLI `validate --fix`, skips it.
B keeps recovery separate and leaves Checkup with a link to Snapshots.
Both show the prefix/config replacement warning and require confirmation.

### 3. What happens after a portable copy, and in a portable session?

**Recommend A:** copy from the active standard root, show its destination,
and keep the current library. Disable copying in a portable session with
"Copy from the standard library." No silent switch to host environments.

```
A. Keep this library                   B. Offer another session
Notes > [Copy to portable folder...]    Notes > [Copy to portable folder...]
Folder [~/usb/wineo] [Browse...]        Folder [~/usb/wineo] [Browse...]
[Review] > [Copy]                      [Review] > [Copy]
Copied notes to ~/usb/wineo.            Copied notes. [Open folder library]
Library still shows host environments  Opens another GUI using that root
Portable session: action disabled      Portable session: action disabled
```

A fits the copy-only scope and current startup selector. B needs a defined
second-process startup selector, instance isolation, and launch/close rules;
it must not repoint an existing worker's `Paths`. Neither copies runners or
native program files, nor adds portable-to-portable copying.

### 4. How are exec programs and arguments entered?

**Recommend A:** a per-environment "Run program" page, with program separate
from argument chips, as in Settings. Each added chip is one exact argument,
including spaces. Never run the text through a shell. Paths from
Browse are absolute. Typed Unix paths must be absolute or start with `~/`;
Windows paths and Wine built-in names retain core meaning. A bare name is
never resolved against the GUI's current directory (see step 3).

```
A. Exact argument chips                B. Quoted argument field
[< Back] Run program in Notes          [< Back] Run program in Notes
Program [winecfg       ] [Browse...]    Program [winecfg       ] [Browse...]
Arguments [two words x] [/quiet x]     Arguments ["two words" /quiet    ]
Add argument [           ] [+]         Parsed: [two words] [/quiet]
[Environment variables v] [Run]        [Environment variables v] [Run]
```

A matches Settings and needs no quoting. B reuses Heroic's quoted-argument
parser and shows exact argv before Run. Both provide temporary KEY/VALUE overrides,
use core validation, and leave config and targets unchanged. No history or
saved presets are proposed.

### 5. What is exec's lifetime and access policy?

**Recommend A:** a GUI-owned shared exec worker that keeps its launch/build
locks through the immediate command's exit, with log and exit result. It can
run beside a tracked application as CLI exec does. Closing offers Cancel
and close, waits for cleanup, and offers Force stop only if safe for that
worker. Core must define cancellation that cannot kill an unrelated program
sharing the prefix; do not reuse prefix-wide installer killing blindly.

```
A. GUI-owned exec                      B. Recorded exclusive tool run
Notes is running > Run winecfg         Notes is running > Run winecfg
winecfg opens beside Notes             "Stop it first."
[Output] [Cancel]                      Stop Notes > Run winecfg
Exit 0 > result stays                  [Output] [Cancel] [Force stop]
Close > cancel and wait                Wait until the prefix is idle
                                      Crash > Clean up recorded tool
```

A preserves exec access and immediate-child semantics, but descendant
handoffs and crash cleanup need explicit limits. B reuses installer-style
tracking and settlement, but is a different exclusive workflow. If B is
chosen, label it accordingly and preserve CLI exec behavior. Neither counts
tool time as playtime. This decision blocks the exec lifecycle core step;
its outcome, cancellation boundary, and descendant policy must be recorded
before implementation.

### 6. What winetricks controls should be exposed?

**Recommend A:** declared status and Install missing, plus separate Run verbs
and Open winetricks actions. Settings remains the only editor of the declared
list. One-off verbs do not silently become dependencies.

```
A. Declared and one-off work            B. Only forward winetricks arguments
[< Back] Notes winetricks               [< Back] Notes winetricks
corefonts Installed                    Arguments [-q corefonts]
gdiplus   Missing                      [Run] [Open winetricks]
[Install missing] [Edit in Settings]    Change declarations in Settings;
Verbs [corefonts gdiplus] [Run verbs]    install them with Play or Checkup.
[Additional arguments v]
[Open winetricks]
```

A makes the existing declared workflow visible and offers arbitrary CLI
arguments under Additional arguments. B is smaller and closest to the
command. Both use `run_in` for arbitrary arguments, never imply uninstall,
and allow installer windows to appear. "An installer window may open." is
shown before starting. No embedded winetricks window or verb catalog.

### 7. Do these operations use the single running-change slot?

The slot (`management_busy` in `wineo-gui/src/main.rs`) disables Add, Clone,
Remove, Downloads, Art, and Settings saves for every environment. Exec and
**Open winetricks** can stay active as long as an external window is open,
so putting them in the slot would freeze all of that until the window
closes.

**Recommend A:** snapshot create/restore/delete/recover and portable copies
use the existing slot. Exec, Run verbs, Install missing, and Open winetricks
run as per-environment tool work: they block mutations of their own
environment (and of others sharing the prefix) but leave the rest of the
app usable. Results stay at the originating page. Launches in other
environments keep working, subject to core locks.

```
A. Slot for filesystem changes          B. One slot for everything
winecfg open in Notes                  winecfg open in Notes
Editor > Clone: works                  Editor > Clone: "Busy: running winecfg."
Notes > Restore: "Stop it first."      Notes > Restore: "Busy: running winecfg."
Close > cancel tools and the copy      Close > cancel it and wait
```

A keeps long-lived windows from blocking unrelated work, at the cost of
per-environment busy state and settling several tool workers on close. B
reuses current handling but blocks the app while any external window is
open. Both respect environment/prefix locks; the UI's busy state is not a
replacement for core locking.

### 8. Should restore reconcile desktop entries right away?

Launch, exec, and winetricks already reconcile desktop entries before they
run (`launch::reconcile` in `launch/orchestration.rs`). A restore that drops
a target leaves its entry stale only until the next run in that
environment.

**Recommend B:** keep current restore behavior, which needs no CLI change.
The restore result offers **Check for problems**, and Checkup's existing
stale-entry fix covers the gap.

```
A. Reconcile after restore              B. Leave it to the next run or Checkup
Restore before-update                 Restore before-update
Saved targets: editor                  Saved targets: editor
Entry for removed viewer disappears    Entry for removed viewer remains
Restored notes.                        Restored notes. [Check for problems]
Warning if entry update fails          Next run or Checkup removes it
```

A removes the window entirely but is a CLI behavior change: a shared
post-restore helper for GUI and CLI, reporting a completed restore
separately from reconciliation warnings. Neither deletes art for removed
targets or turns reconciliation failure into rollback of a successful
restore.

## Proposed design

The following assumes the recommended answers. Adjust it and the step
boundaries when the maintainer chooses differently.

### Snapshots

- Add Snapshots to the tools group without moving Downloads' Ctrl+4;
  Ctrl+5 opens Snapshots. **Snapshots...** in the detail/card actions
  selects that environment. Native environments have no snapshots.
- List labels and creation times with **Create snapshot...**, **Restore**,
  **Delete**, and **Refresh**. Preserve core label order. No measured sizes
  or automatic scans of prefixes outside the active library.
- Create takes a label, waits for wine on a worker, then shows the inspected
  prefix, runner, layout, destination, and excluded external links. Explain
  "Snapshots need reflinks." and "External files aren't included." before
  **Create**. A bad/duplicate label stays in the form with its error.
- Restore waits and inspects before confirming the frozen review from Q2.
  A changed config/prefix/snapshot requires a fresh review, never automatic
  acceptance. Restore writes the saved config bytes, not a settings edit.
  Art and playtime stay. Refresh the panel's targets and any open settings;
  preserve Settings' existing conflict handling for unsaved edits.
- Delete asks "Delete snapshot before-update?" and says it cannot be
  restored from Trash. Delete/recover run off the UI thread and finish
  before close. A failed delete can leave an unreadable snapshot; show the
  list error and Refresh rather than pretending the list is empty.
- A recovery marker shows a banner with **Roll back restore** even if the
  environment's normal report cannot load. Ordinary mutations stay disabled
  until recovery. Read marker state through core/Paths, not error-string
  matching. A malformed marker shows its error and is never guessed around.
  With Q2-A, `Fix::RecoverSnapshot` calls existing `snapshot::recover` and
  uses the existing Checkup batch behavior. Recovery is not cancellable.
- Reflink failures keep the live prefix/config untouched before publishing;
  no full-copy fallback for snapshots. Surface retained old subvolume paths
  and recovery instructions in a persistent result, not just a toast.

### Portable copies

- **Copy to portable folder...** opens a review flow like Clone, with a
  folder chooser and typed-path fallback. Build destination `Paths` with
  the explicit folder so `WINEO_PORTABLE_DIR` cannot override the pick.
- Review Wine and native copies separately using core's inspection types.
  Include source/destination, runner/layout where applicable, targets,
  snapshot labels, art/playtime, first-copy config/templates policy,
  disabled desktop entries, literal source paths, and external files/links.
  Expand long lists instead of dropping warnings. Native review says
  "Program files stay where they are."
- Wine first waits for the source prefix to become idle. FullCopy says
  "This copy takes full space." Unknown says "Reflink support is unknown."
  Neither blocks the copy. Mention future snapshot requirements when
  snapshots come along. Do not offer downloads or promise a self-contained
  executable bundle.
- **Copy** executes the exact review. Collisions or changed inspection
  state keep the flow open with an error and a fresh-review action.
  Cancellation cleans staging; once publishing starts, wait for completion.
  A publish failure lists core's reported paths to move aside before retry,
  without an overwrite/cleanup button. Source data stays unchanged.
- Success shows the destination with a toast and persistent finish result;
  retain the current library as in Q3-A.

### Exec and winetricks

- Add **Run program...** and **Winetricks...** to the detail/card actions.
  Each opens a per-environment page with Back, like Art. Hide Wine-only
  actions on native environments; portable Wine environments use them too.
- Exec validates input before building a missing prefix. A new core entry
  point sequences the existing `runner_locked`, `ensure_prefix`, and
  `reconcile`, then owns spawning, output, cancellation, and the chosen
  lifecycle from Q5. It never resolves a program against the process's
  current directory. The GUI only translates form values and renders
  events/results. No shell, terminal launch, `wineo` subprocess, or
  duplicated CLI lifecycle code.
- Show creation/reconciliation status, **Output**, and the exit result.
  Nonzero exit is a result, not a successful toast. Do not register a target,
  enable launch wrappers, install declared verbs, or change playtime.
- Winetricks uses `run_in` for explicit arguments and empty arguments for
  Open winetricks. Install missing needs a small core wrapper: `ensure`
  refuses a missing prefix, so the wrapper builds it first through
  `launch::ensure_prefix`, as `run_in` does, then installs under `ensure`'s
  locks. The GUI never sequences prefix preparation itself, and
  `orchestration::install_winetricks` (which expects a prepared launch and
  skips locks) is not the entry point. Do not open winetricks' picker as a
  side effect of this action.
- Reuse install log viewing and `EnsureEvent` handling. Declared installs
  have verb steps; arbitrary arguments have a spinner, log lines, and
  lifecycle status, without invented percentages. Empty arguments can stay
  active while the external window is open.
- Cancel and Force stop use core tokens. Winetricks can kill Wine processes
  in the prefix while settling, so keep its exclusive idle checks. Completed
  verbs stay installed after cancellation; refresh the installed/missing
  record and Checkup results after failure or cancel too. No rollback or
  uninstall promise. Session draft inputs are not saved as history.

### Running changes and core gaps

- Extend the running-change slot, per-environment tool state, and close
  prompt per Q7. Keep
  tokens/workers outside disposable page state. Switching pages or closing
  a panel must not release locks, cancel an operation, or lose its result.
  Reject stale inspection and log messages with worker generations.
- Disable conflicting actions with the known running/busy/lost reason.
  Core rechecks remain authoritative when the CLI races the GUI. Shared
  prefixes are refused for snapshots; portable copying checks all source
  registrations sharing the prefix. A lost run points to existing Clean up.
  Waits only wait, never stop another application's processes.
- Closing offers **Keep working** or **Cancel and close**. Cancel stops
  cancellable checks, waits, copies, and tools; once publishing/swap begins,
  say "Finishing changes..." and wait. No Force stop for filesystem changes.
  Unexpected process death during restore uses the existing recovery marker.
- Gap: snapshot/portable inspection and final fingerprint walks have no
  token/progress interface. Add frontend-capable variants for cancellable
  inspection/revalidation, preserving existing entry points for the CLI.
  Reuse `ImportProgress::Checking` where possible; do not fake total bytes.
- Gap: snapshot restore and portable publication do not emit a boundary
  event before the non-cancellable changes. Add an explicit finishing phase
  through operation-specific events or sibling APIs. Native portable work
  also needs progress for template/art copies. Preserve published success
  when a cancel request arrives after the boundary.
- Gap: exec has command construction but no frontend lifecycle API. Q5 must
  settle access, descendant lifetime, cancellation, logs, and crash limits
  before shaping that API. Existing Installer is reusable only for Q5-B.
- Conditional gap: Checkup has no snapshot recovery fix (Q2-A). Add the typed
  fix, label, application, and tests; malformed markers still fail safely.
- Snapshot restore does not reconcile desktop entries; the next launch,
  exec, or winetricks run does. With Q8-B the restore result offers
  **Check for problems**. Q8-A would add a shared post-restore helper, used
  by GUI and CLI, reporting partial failure without hiding a completed
  restore and never deleting retained target art. No blanket cleanup of
  unrelated files.
- After every outcome refresh the relevant list, library, trash badge,
  installed verbs, and any visible check report. Results and warnings survive
  navigation. A completed restore with a reconciliation failure remains a
  completed restore plus a warning; never report it as canceled.
- Keyboard follows existing pages: Esc goes back or closes a confirmation,
  not a running worker. Enter submits a valid idle form. A hint reads
  "Press Enter to run it, Esc to go back." Logs keep current selection/copy
  controls. Additional shortcuts beyond Ctrl+5 need no new scheme.

## Implementation sequence

Each step is one coder's scoped change, with tests, and can land after its
listed prerequisites. No implementation starts before the decisions above
are approved.

1. **Core snapshot frontend support.** Cancellable inspection/revalidation,
   finishing event, and Q2's recovery integration as a single-item fix that
   batch fixing skips. Preserve existing APIs; test cancellation before swap,
   recovery refusal, and that `validate --fix` leaves the marker alone.
2. **Core portable frontend support.** Cancellable reviews/revalidation,
   native copy progress, and publishing event. Keep current collisions,
   first-copy policy, and CLI output. Independent of step 1.
3. **Core exec lifecycle.** A frontend entry point that reuses
   `runner_locked`, `ensure_prefix`, and `reconcile`, and adds the
   Q5-approved spawn, wait, output, events, and token handling under the
   build/operation locks. Add a resolution mode that never consults the
   current directory, so a bare name stays a Wine name. Keep the CLI's
   attached behavior and cwd-relative paths. Independent of 1-2.
4. **Core restore reconciliation.** Only if Q8-A is chosen: shared helper
   and GUI/CLI outcome notes for entries affected by restored targets. Tests
   retain art and preserve successful restore after a reconciliation
   failure. Uses 1. With Q8-B this step is dropped.
5. **GUI snapshots.** Page/menu entry, picker, list, create and restore
   reviews, delete confirm, recovery banner, status/results, and tests.
   Uses 1, and 4 if chosen. Checkup rendering reuses typed fixes.
6. **GUI portable copy.** Folder form, Wine/native review, worker,
   progress/cancel, and finish/errors. Uses 2 and the existing Clone flow.
7. **GUI exec.** Form/argv preview, temporary overrides, worker, log,
   exit result, and tests. Uses 3. No config/target editor changes.
8. **GUI winetricks.** The core Install missing wrapper that builds a
   missing prefix, then Q6's controls, status record, arbitrary run/log,
   cancel/force, and record refresh tests. Independent of 5-7.
9. **GUI integration and acceptance checks.** Wire 5-8 into the slot and
   per-environment tool state,
   navigation generations, shared close prompt, refreshes, disabled reasons,
   and keyboard handling. Each earlier GUI step includes its own busy/close
   safety before landing; this step checks interactions across features.
10. **Docs and maintainer acceptance.** The primary updates
    `docs/gui.md`, `docs/core-api.md`, feature pages, README links, and the
    features backlog. Record any CLI changes. The primary prepares the
    concrete acceptance checklist and the maintainer runs it before done.

## Implementation progress

On 2026-10-02, core steps 1-3 and GUI steps 5-9 are implemented. Step 4
remains dropped under Q8-B. Documentation is updated; maintainer window
acceptance is still pending, so the plan stays approved with no version bump.

The GUI keeps snapshot and portable workers in the filesystem-change slot
and tool workers by environment. Workers and results survive navigation;
close waits for every worker. Snapshot rollback is separately confirmed in
Snapshots and Checkup and is skipped by batches. Install missing has a core
wrapper that builds a missing prefix without opening the winetricks picker.

Independent QA found and verified fixes for repeat portable copies, the
destination success notice, and refreshing cached winetricks declarations
after Settings edits. A shared-prefix running refusal was also fixed, with
a regression test preserving exec coexistence.

Verification: full `cargo test` passed outside the socket-restricted sandbox;
the sandbox run failed only the three existing local HTTP fixture tests.
The checkout supports reflinks, so snapshot success tests were able to run.
Ignored real-program/window tests have not been run. GUI tests also cover
navigation, stale replies, multiple-worker close, source/destination notices,
repeat copies, and prefix conflicts. No GUI window was opened for smoke tests.

## Testing and acceptance

- Core fixtures: Wine/Proton snapshot create/restore, exact legacy config
  bytes, shared-prefix refusal, changed reviews, duplicate labels,
  cancellation during wait/check/copy, and late cancellation during finish.
  Test unsupported reflinks explicitly; reflink-only success tests can skip
  on unsupported checkout filesystems, but report those skips.
- Restore recovery: interrupted swap/config write, malformed marker,
  failed rollback, preserved old subvolume, portable marker relocation on
  the same filesystem, and identity refusal across filesystems. If approved,
  Checkup and CLI validate apply the same recovery once under core locks.
- Portable: Wine/Proton/native copies, existing/fresh destination config,
  template/art progress, playtime, relative links, full-copy fallback,
  Unknown storage, collisions, overlap, running/shared source, changed
  review, cancel before publish, and partial-publish error paths. Do not
  weaken existing snapshot, portable, or lifecycle tests.
- Exec: fake Wine/umu commands, built-ins and MSI, a bare name that matches a
  file in the current directory still resolving in the prefix, quoted argv
  and overrides,
  prefix build cancellation, no wrappers/protonfixes/declared installs,
  nonzero exit, locks held for the selected lifetime, cancel without killing
  another shared program, and Q5's descendant/crash behavior. Preserve CLI
  integration tests and attached exec semantics.
- Winetricks: Wine and Proton, unavailable Wine winetricks, missing prefix,
  declared and one-off installs, empty-args external picker, arbitrary flags,
  logs, partial records, running/lost/busy refusal, cancel/force settlement,
  and config bytes unchanged by a one-off run.
- GUI update/worker tests without windows: menu eligibility, frozen reviews,
  list errors instead of empty success, page return/input handling, stale
  messages, persistent results, selected root, slot refusal, navigation
  during work, phase-aware close/cancel, and refresh after every outcome.
  Ensure restoring targets reloads panel/settings without losing dirty edits.
- Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo test`; report reflink skips and any socket-dependent checks the
  primary must rerun outside the Codex sandbox. Ignored real-program tests
  run only with their available prerequisites.

Maintainer acceptance uses throwaway Wine, Proton, and native environments
and disposable portable folders. Start the built `target/release/wineo-gui`
for the standard library, or set `WINEO_PORTABLE_DIR` to test a portable root.
Stop external launchers before prefix operations.

- [ ] Ctrl+4 still opens Downloads; Ctrl+5 opens Snapshots. The Actions menu
  selects the intended environment. Native actions omit snapshots and tools.
- [ ] Create a `before-update` snapshot for Wine and Proton. Change settings,
  targets, prefix contents, art, and playtime, then inspect Restore. Expand
  both config texts and restore. Prefix/config return exactly; art, external
  files, and playtime stay. Check for problems handles stale desktop entries.
- [ ] Try a duplicate label, shared/running/locked prefix, and a filesystem
  without reflinks. Cancel inspection or a large copy before finishing. No
  live prefix/config changes occur. Delete asks for confirmation and Refresh
  reports unreadable snapshots as an error, not an empty list.
- [ ] Interrupt a restore only in a disposable environment. Reopen and use
  Roll back restore in Snapshots, then repeat through Checkup's separate
  confirmation. Fix everything and CLI `validate --fix` leave rollback alone.
  Verify malformed markers refuse recovery and retained subvolume paths stay
  visible when applicable.
- [ ] Copy Wine, Proton, and native environments to fresh and existing
  portable folders. Review config/templates policy, snapshots, art/playtime,
  desktop policy, external links, and literal source paths. Verify full-copy
  wording on a destination without reflinks, collision/overlap refusal, and
  no source changes. Success shows the destination and a notice, keeping the
  current library; Copy to another folder starts a second review.
- [ ] Start the GUI with `WINEO_PORTABLE_DIR`. Snapshot/tool actions use that
  root; portable copying says Copy from the standard library.
- [ ] Run `winecfg`, a browsed executable/MSI, a bare Wine program name that
  also exists in the GUI's working directory, and exact arguments containing
  spaces. Try temporary variables and a nonzero exit. Verify output/results,
  exec coexistence with a tracked app, no declared installs, and no playtime.
  Cancel affects only the started process group; the result warns that other
  programs may still run in the prefix.
- [ ] In Winetricks, edit declarations in Settings and reopen to see fresh
  Installed/Missing status. Install missing in a missing prefix, run a one-off
  verb, and open the external picker. Try an installer window, cancel and
  force-stop separately, and check partial records and unchanged declarations.
- [ ] While a tool is open, change an unrelated environment. Changes to its
  own/shared prefix and winetricks in a running shared prefix are refused.
  Switch pages during every operation and race a conflicting CLI command.
  Results stay available and changed reviews require another review.
- [ ] Close during inspection, copying, publishing/swap, and multiple tool
  runs. Keep working leaves everything active; Cancel and close waits for all
  workers. Finishing filesystem work offers no Force stop and completes
  before exit. Repeat with a tool and an unrelated copy active together.

## Not in this plan

- Art and build download management, already covered by 0040.
- Automatic snapshots, retention, bulk operations, filesystem backups,
  cross-filesystem snapshot restore, or changing the reflink requirement.
- Discovering orphaned snapshots or changing trash/recovery storage formats.
- Portable-to-portable copying, bundling runners/external program files,
  or switching a live GUI's storage root. Q3-B needs explicit extra approval.
- A terminal emulator, shell command language, saved exec presets/history,
  target registration after exec, or native exec.
- A winetricks catalog, uninstall, rollback of installed verbs, new global
  or target dependency lists, or embedding external installer windows.
- Concurrent filesystem-changing workers. Q7-A only lets tool work run
  beside them.
- A general UI polish pass or unrelated CLI behavior changes.
