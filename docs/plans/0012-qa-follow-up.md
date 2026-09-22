Status: approved

# 0012: QA follow-up for guided setup, imports, and downloads

## Goal

Resolve the findings selected by the maintainer from the QA of plans 0007,
0008, and 0009 at `7f3c831`. Start with process cleanup and existing-config
safety, then correct import/download behavior and output, close the test gaps,
and record real lifecycle acceptance.

Finding IDs below refer to the numbered lists in that QA report, not source
line numbers. The reviewed ranges were `ee6a6bb..1d76897`,
`6989c49..21059e9`, and `6ce3c51..7f3c831`.

## Decisions

Settled by the maintainer during QA triage and plan approval:

- Fix 0007 findings 1, 2, 3, 5, and 6.
- Keep 0007 finding 4 as-is: invalid-name errors should be concise and identify
  the problem, without suggesting a replacement name. This supersedes the
  original 0007 wording and is recorded there too.
- Fix 0008 findings 1, 2, 3, 5, 6, 8, 9, and 10.
- For 0008 finding 4, use option 1: offer copy only for an accessible, validated
  build; otherwise explain the problem and offer a compatible host runner.
  Portable builds used by Flatpak Heroic remain eligible for copy. Do not add
  sandbox path discovery or a Flatpak launch backend as part of this fix.
- For 0008 finding 7 (`disableUMU`), use option 1: report the launcher mismatch
  through the existing review policy and retain Wineo's UMU-only Proton model.
  H records the investigation and selected handling.
- Fix all twelve 0009 findings.
- The maintainer approved this plan using the recommended solutions. Preserve
  manual recovery when process ownership is uncertain and report success for
  cancellation after atomic publication. For a missing pinned component,
  offer an explicit download of that exact version and resume the operation.

Preserve the earlier decisions on copied versus adopted prefixes, external
game files staying in place, reviewed target batches, retained completed
runner copies, warning-only handling of supported review omissions, explicit
downloads, and one-time bulk desktop selection.

The edit-exe game-directory row, Kitty Ctrl+Backspace, extracting icons directly
from an EXE, JSON output, and 0011's key bindings remain outside this plan.
Cancellation cleanup and truthful operation outcomes are in scope; changing
which keys invoke cancel or back is not.

## Finding coverage

| Finding | Disposition | Work and acceptance |
|---|---|---|
| 0007.1 | Fix | A: safe identity checks for lost Proton groups |
| 0007.2 | Fix | A: keep unconfirmed installer cleanup unresolved |
| 0007.3 | Fix | A: recovery guidance after registration failures |
| 0007.4 | Keep | Concise invalid-name errors; correct stale spec/test wording |
| 0007.5 | Fix | F: guided adoption, existing exe, repeated and resumed setup |
| 0007.6 | Fix | G: recorded manual lifecycle acceptance |
| 0008.1 | Fix | C: publication decides download success |
| 0008.2 | Fix | D: home expansion for edited exe/cwd paths |
| 0008.3 | Fix | D: shared-prefix disclosure |
| 0008.4 | Fix, option 1 selected | D/F, specified in H: validate copying or offer a host runner |
| 0008.5 | Fix | D: external dependencies of copied runners |
| 0008.6 | Fix | D: Wine NVAPI dependency warning |
| 0008.7 | Fix, option 1 selected | D/E/F, specified in H: disclose the `disableUMU` mismatch |
| 0008.8 | Fix | F: adoption followed by `rm --delete-prefix` |
| 0008.9 | Fix | C: abandoned download staging retry test |
| 0008.10 | Fix | F: local runner-copy failure and concurrency tests |
| 0009.1 | Fix | B: safe Heroic ID matching and backfill |
| 0009.2 | Fix | E: bulk warnings for dropped and omitted settings |
| 0009.3 | Fix | E: retain full review evidence for verbose output/logs |
| 0009.4 | Fix | E: color follows the actual output stream |
| 0009.5 | Fix | E: render effective Wine component choices |
| 0009.6 | Fix | E: show nonempty arguments |
| 0009.7 | Fix | C: release details once on every download path |
| 0009.8 | Fix | E: print and flush per-game progress before copying |
| 0009.9 | Fix | E: enable color in guided Heroic prompts |
| 0009.10 | Fix | F: complete bulk import flow tests |
| 0009.11 | Fix | C: release picker, pagination, and output tests |
| 0009.12 | Fix | C: accepting a missing-component download |

## A. Process ownership and setup recovery

Primary code: core `runtime`, `process`, `supervisor`, and `installer`; CLI
`stop`, `setup`, `new`, and `wizard`.

### Lost Proton runs and installer cleanup

- Record the boot and process identity needed to distinguish the launched
  Proton group from a later group with the same numeric ID. Apply the same
  ownership rules to lost runs and installer records.
- Keep old runtime records readable. Missing identity or an unreadable record
  must not grant permission to signal a group or count as confirmed cleanup.
- Distinguish a verified original group, an original group proven gone, and
  ownership that cannot be established. A reused group must never be signaled.
  Unknown ownership remains unresolved with concise recovery guidance.
- Account for a group leader exiting while its children remain. A missing
  leader PID alone is not proof that its whole group is gone. Check identity
  at the signaling boundary and keep race handling conservative.
- Confirm tracked processes have stopped before releasing recovery protection.
  Preserve the original concrete runner/prefix snapshot when aliases change.
- If persisting the post-spawn installer identity fails, stop and settle the
  child using the live handle and known group before returning. If that cannot
  be confirmed, retain the recovery record and refuse conflicting work.
- Do not clear an operation merely because cleanup reports `Untracked`.
  `stop` and setup's cleanup offer must report unresolved cleanup, keep the
  block in place, and avoid proceeding into scanning or launching.
- Preserve existing manual recovery guidance for cases that cannot be safely
  resolved automatically; do not add an unsafe force-by-PGID fallback.

Tests use disposable processes and synthetic stale records, never real PID
recycling or unrelated applications. Assert that a foreign process survives,
the original group's children are cleaned up when ownership is established,
old/missing/unreadable identity stays safe, and reboot/start-time mismatches
cannot signal a later group. Inject record-write failure and test that
`start`, `exec`, installers, setup, and destructive removal remain blocked
while cleanup is unresolved. Keep a separate environment usable throughout.

### Errors after registration

Route installer preparation and spawn errors through shared post-registration
reporting in both guided and flag-driven creation. Say that the environment
was kept and print `wineo setup <name>` when it is safe to continue. Unresolved
cleanup must instead direct the user to cleanup first. Preserve nonzero exit
status, installer failure information, saved targets, config, and prefix.

Inject disappearance/failure between validation, registration, preparation,
and spawn. Assert surviving files and exact recovery guidance. Failures before
registration must not claim that a resumable environment exists.

## B. Heroic identity and existing-config safety

Primary code: CLI `import_heroic` matching/classification and core
`Environment::record_heroic_id`.

- Match recorded IDs first. Prefix, executable, and default-name fallback
  matching applies only to environments without a recorded Heroic ID.
- Backfill under the existing config lock, re-reading the current config.
  Recheck that it still represents the match and that its ID remains absent
  or already equals the requested ID. Never overwrite a different ID.
- Keep the in-memory matching view current after backfill so another candidate
  in the same invocation cannot claim the same environment through stale
  legacy-match data.
- Preserve comments and unrelated settings, atomic writes, and existing
  registration/name-collision behavior. A removed or concurrently changed
  environment must not be recreated or silently retagged.
- Preserve the approved timing of legacy ID backfill during classification;
  this plan does not move those writes behind the import confirmation.

Test existing A/B IDs sharing prefixes, executables, and default names; genuine
legacy matches; repeated imports; multiple candidates matching one legacy
environment; concurrent ID/config edits; and removal before backfill. Assert
exact config content, imports/skips, warnings, and idempotence rather than only
the matching helper's return value.

## C. Download completion, recovery, and guided choices

Primary code: core `download`; CLI `download`, `transfer`, `components`, and
the guided callers in `wizard` and `import_heroic`.

- Treat successful atomic publication as the completion boundary. A late
  cancellation flag must not turn a published success into exit 130 or claim
  that the completed installation was canceled. Honor cancellation before
  publication and clean only this attempt's staging files.
- Keep a successfully installed runner when a later prefix import is canceled,
  as already specified. Do not implement rollback of completed installations.
- Show concrete release, asset, human-readable size, verification method, and
  destination exactly once before transfer on both the picker and quick-latest
  paths. Preserve progress and the final installed name.
- Keep guided lists at three releases with the first marked `[latest]`, and
  retain `m) more`, pagination, and complete `--list` behavior.
- Cover accepting a missing DXVK/vkd3d download and resuming the original
  guided operation. Preserve Wine/Proton compatibility rules, explicit version
  requirements, and vkd3d's DXVK dependency. Do not replace a requested pinned
  version with `latest` silently. When a pinned build is missing, offer to
  download that exact version and resume the operation after installation.

Use an injectable fixture transport/catalog and deterministic phase hooks where
needed; tests must not contact upstream or depend on signal timing. Assert:

- Cancellation before publication leaves no installed build and removes the
  current staging tree; cancellation after publication reports success and
  leaves the complete verified build intact.
- A simulated abandoned stage is cleaned on retry under its destination lock;
  unrelated staging and completed installs remain untouched, and a concurrent
  install cannot have its active staging removed.
- Three-release/default selection, `[latest]`, `more`, page navigation, fewer
  than three releases, and complete scriptable listings produce the promised
  choices and output, including details exactly once.
- Accepting downloads resolves the missing component and continues the caller;
  decline/failure/cancel preserves the correct state and guidance. Bulk guided
  import asks once per component. Flag-driven import and `--all --yes` never
  download implicitly.

## D. Import paths and dependency disclosure

Primary code: core `heroic` and `import`; CLI `import_heroic` review/edit paths.

- Expand `~/` through the existing `Paths` helpers when editing executables
  and working directories, while preserving the established meaning of
  prefix-relative paths. Reuse pasted-path cleaning and keep spaces intact.
  Test absolute, home-relative, prefix-relative, quoted, and escaped paths
  through saved config and launch resolution.
- Identify other candidates in the selected Heroic source that use the same
  normalized prefix. Review names/counts and explain that copying includes
  the whole prefix while adoption preserves shared mutable state. Account for
  Proton `pfx/` normalization and path aliases; expose the disclosure in guided,
  flag-driven, and bulk review and retain full details in verbose logs.
- Include external runner symlinks/dependencies in runner-copy review and the
  copy result so they survive subsequent summary/log rendering. Group them in
  concise output and expand them with `-v`. Preserve/rebase internal links,
  never follow external links to copy host files, and never imply that a
  retained copy is independent of those dependencies.
- Report the Wine NVAPI DLL requirement when carrying its enable settings.
  Distinguish enabling variables from installing DLLs; do not add automatic
  NVAPI installation or an unconditional claim that DLLs are missing. Preserve
  the settled suppression of settings covered by Proton.
- Apply H's selected runner-copy preflight and compatible host fallback for
  0008.4, and collect the specific `disableUMU` mismatch evidence for 0008.7.
  E handles its review/log presentation; F covers the complete import flows.

Tests assert both data and disclosure: edited targets launch through fake
runners with the intended paths; shared-prefix warnings name the affected
games; copied internal links still work; external targets remain untouched;
normal/verbose output and logs retain external dependencies and the Wine
NVAPI warning.

## E. Review output, logs, color, and progress

Primary code: CLI `import_heroic`, `style`, `prompt`, and download output.

- Separate immutable review evidence from the mutable set of unresolved
  blockers. Applying approved omission/drop defaults must not erase what was
  omitted from later output or the import log.
- Give bulk rows a `warn:` note for omitted unsupported settings, dropped
  managed conflicts, and attention items. Preserve shared-warning collapsing;
  it must not hide additional per-game warnings.
- Render normal, verbose, and logged reviews from the retained evidence.
  Verbose output/logs must identify dropped `WINEPREFIX`-style conflicts and
  unsupported fields after the import defaults have been applied. Preserve
  existing sensitive-value redaction and never import launcher credentials.
- Resolve effective runner/component settings before rendering their summary.
  Show Wine DXVK/vkd3d carry-over rows, omit Proton-covered settings, and show
  nonempty arguments with unambiguous boundaries in every applicable flow,
  including flag-driven review. Keep empty/default rows suppressed.
- Make styling depend on the stream actually receiving the output. Preserve
  `anstream` handling of `NO_COLOR` and `CLICOLOR_FORCE`; do not assume stdout
  and stderr have the same terminal status. Enable styling in guided Heroic
  prompts through the shared helpers. Persist plain text in import logs.
- Print and flush `Importing <name>...` before work starts, then complete that
  same per-game line with success or failure. Keep verbose detail readable and
  preserve continuation past failures and the final summary/exit status.

Tests must exercise review-before-resolution and rendering/logging afterward,
not only preconstructed summaries. Assert complete warnings and log fields,
component/argument rows, and one-time output. Use a controlled blocked copy to
prove progress is visible before completion. Test terminal/pipe combinations
for stdout and stderr independently, `NO_COLOR`, and forced color in isolated
subprocesses; distinguish styling from line-editor control sequences.

## F. End-to-end regression coverage

Add behavioral fixtures to the existing test suites rather than a parallel
test framework. Tests belong with each logical fix; this section names the
remaining workflow gaps that helper tests do not cover.

### Guided setup and continuation (0007.5)

- Guided adoption preserves existing prefix content and uses the detected
  architecture; it does not initialize the prefix as a new empty install.
- Guided creation with an existing executable preserves its location and
  argument boundaries, including spaces and external paths.
- Repeated setup does not silently duplicate targets or replace existing
  choices. A resumed setup can select targets without rerunning an installer.
- Keep assertions for staged targets applied as one validated batch, saving
  them before a failed/canceled installer, and immediate desktop reconciliation.
- Assert unrelated config keys, targets, desktop entries, and another
  environment survive errors and cancellation.

### Adoption followed by deletion (0008.8)

Exercise import adoption followed by `rm --delete-prefix` on disposable Wine
and Proton prefixes, including `pfx/` normalization. Assert the configured
prefix and registration are removed, external symlink targets remain intact,
shared/aliased registered prefixes are refused, and a failed/partial deletion
retains the config and recovery guidance. Preserve plain `rm` retaining the
adopted prefix. Record byte/content checks for source data that must survive.

### Local runner copying (0008.10)

Exercise Wine and Proton layouts, executable modes, internal/external links,
source changes during copy, incomplete layouts, collisions, concurrent
publication, and cancellation before publication. Assert complete publication
or no publication, staging cleanup, and unchanged source content. A completed
runner must survive cancellation of a later prefix import and be reported as
retained. Reuse the dependency-disclosure fixtures from D.

### Complete bulk Heroic flow (0009.10)

Invoke bulk import through CLI/PTY fixtures with isolated XDG roots and fake
runners. Cover classification, real config backfill, multiple games, copy and
adopt, confirmation/defaults, desktop choices, warnings, verbose logs,
per-game failure continuation, summary counts, and exit status. Include
`--desktop`, `--yes` without `--desktop`, all-skipped results, missing components,
and a failure after another game succeeds. Assert saved configs, unchanged
sources, desktop entries, actual output, and final registration state.

The existing terse invalid-name behavior stays. Rename any misleading test
that claims to check replacement suggestions so it describes the assertion
it actually makes; do not restore the removed suggestion.

## G. Recorded manual acceptance (0007.6)

After automated checks and independent QA, use a maintainer-chosen local game
with an available installer and updater. Use disposable prefixes and isolated
XDG roots. The maintainer's active display is allowed by the settled 0007
decision; a dedicated display is not a new requirement.

Record the tested commit, runner/build versions, fixtures, exact commands or
guided choices, expected/actual results, and remaining issues in this plan's
acceptance results. Run the full sequence:

1. Create, install, select targets, choose the default, and create desktop entries.
2. Launch from the desktop and CLI; check arguments, working directory, saves,
   and icon grouping with the existing supported icon sources.
3. Stop, edit targets, revisit setup, run an updater, and relaunch.
4. Remove registration while retaining the prefix; separately adopt and delete
   a disposable prefix with `rm --delete-prefix` and verify external data survives.
5. Check the corrected Heroic summary/logs and bulk flow using disposable
   metadata/prefix copies, leaving the original Heroic library untouched.

Use locally available assets. Do not download games, runners, components, or a
missing runtime as part of this acceptance without separate authorization.
Ask for the game/assets when needed; do not substitute an invented acceptance
record or mark acceptance complete because fake-runner tests passed.

## H. Clarification and investigation

### 0008.4: What the Flatpak finding means

This concerns a sandbox-only Wine binary, not every runner used by Flatpak
Heroic. Flatpak applications have their own filesystem view: application files
are installed under `/app`, with runtime files under `/usr`. A stored path such
as `/app/bin/wine` can be meaningful inside Heroic's sandbox but unavailable
under the same path to wineo on the host. See Flatpak's
[filesystem conventions](https://docs.flatpak.org/en/latest/conventions.html)
and [sandbox model](https://docs.flatpak.org/en/latest/basic-concepts.html).

The current parser can classify metadata like `type = wine`,
`name = Flatpak Wine`, and `bin = /app/bin/wine` as a copyable Wine build and
derive `/app` as its source. The guided chooser then offers that copy as the
default before checking whether it is a portable host-accessible build.
Selecting it fails when `/app` is absent. This is a code-path finding, not a
claim that the maintainer's installed Heroic currently stores those exact values.

A GE-Proton or Wine build downloaded into a user directory by Flatpak Heroic
may still be copyable from the host. Do not reject all Flatpak-origin runners.

Selected by the maintainer: option 1, validate the source before offering copy.
For a sandbox-only or otherwise unavailable build, explain the problem in one
sentence and route guided import to compatible host runner selection. Wine
can use host system Wine or an installed Wine build; Proton can use an
installed build or an explicitly downloaded supported Proton build. Scripted
imports get a clear error directing them to `--runner` when a replacement is
required. Keep portable host-accessible builds eligible for copy.

Do not silently switch Wine/Proton kind, install Flatpak, copy the application
or runtime filesystem, add sandbox-to-host path discovery, or add a Flatpak
launch backend. Preflight does not replace validation immediately before
copying, since source contents can change after the menu was shown.

Add fixtures distinguishing sandbox-only metadata, host system Wine, and a
copyable build downloaded by Flatpak Heroic. Assert the unavailable copy is
never selected, replacement choices remain usable, and scripted imports do
not silently choose a different runner.

### 0008.7: Investigate `disableUMU`

Investigation completed on 2026-09-22 against Heroic v2.22.3 at commit
`97078f6b81e1db86eb8b287d1468ef31a5b6cf13`. The same UMU eligibility rule was
checked on main at `3934a83a0707baad23cd2c06bc94bd23f51e6622`.

Evidence:

- Heroic's [UMU eligibility check](https://github.com/Heroic-Games-Launcher/HeroicGamesLauncher/blob/97078f6b81e1db86eb8b287d1468ef31a5b6cf13/src/backend/utils/compatibility_layers.ts#L581-L594)
  requires Linux and a Proton runner, and returns false for
  `disableUMU === true`. False or absent does not disable UMU; plain Wine is
  ineligible regardless of this field. This check has no Flatpak-only branch.
- The [sideload launch path](https://github.com/Heroic-Games-Launcher/HeroicGamesLauncher/blob/97078f6b81e1db86eb8b287d1468ef31a5b6cf13/src/backend/storeManagers/storeManagerCommon/games.ts#L255-L271)
  calls `runWineCommand` with `waitforexitandrun`. That
  [command builder](https://github.com/Heroic-Games-Launcher/HeroicGamesLauncher/blob/97078f6b81e1db86eb8b287d1468ef31a5b6cf13/src/backend/launcher.ts#L1518-L1524)
  selects the Proton binary instead of UMU when UMU is disabled, and prepends
  the Proton verb. The field changes the launcher; it does not change a
  Proton prefix into a Wine prefix.
- The reviewed [game settings loader](https://github.com/Heroic-Games-Launcher/HeroicGamesLauncher/blob/97078f6b81e1db86eb8b287d1468ef31a5b6cf13/src/backend/game_config.ts#L203-L317)
  preserves the per-game value, leaves it absent on existing configs when not
  set, and supplies false for new games. Its whitelist of inherited global
  fields does not include `disableUMU`. Do not infer an effective true value
  solely from an arbitrary global field; match the supported schema's loader.
- Wineo's core `heroic::collect_unsupported` lists `disableUMU` as known but
  neither translates nor reports it. `Runner::command` always invokes
  `umu-run` for Proton, as settled in plan 0004.

Six disposable CLI fixtures were imported with explicit runners, `--adopt`,
and `-v`, using isolated XDG roots and synthetic Wine/Proton prefixes. Each
import succeeded; none mentioned `disableUMU` in its saved config, output, or
import log. No game, Wine, Proton, or UMU process was launched.

| Fixture | Heroic's relevant behavior | Current wineo result |
|---|---|---|
| Proton, absent | Does not disable UMU | Imports silently; uses UMU |
| Proton, false | Does not disable UMU | Imports silently; uses UMU |
| Proton, true | Runs Proton without UMU | Imports silently; uses UMU |
| Wine, absent/false/true | This flag has no effect on runner eligibility | Imports silently; uses Wine |

The confirmed mismatch is a Linux Proton sideload with effective
`disableUMU=true`. Real-game compatibility under the changed launcher was not
tested. This investigation establishes the behavior difference and missing
disclosure, not that every affected game fails.

Selected by the maintainer: option 1, report the mismatch using existing review
policy. Keep Wineo's UMU-only Proton model and report a specific warning such as
`Heroic disabled umu; wineo uses umu for Proton`. Guided and bulk import continue
using their approved warning defaults, without another prompt. Flag-driven
single-game import follows its existing unresolved-setting refusal and
guidance. Preserve the evidence in verbose output and logs; do not warn for
absent/false values or plain Wine. This does not preserve direct-Proton
behavior, and the warning must say so.

A new acknowledgment mechanism and a direct-Proton launch mode are outside
this plan. A direct-Proton mode would revisit plan 0004 and require a separate
design covering initialization, launch, tools, stopping, and recovery.

Add regression fixtures for the table above,
per-game versus global-only values under the supported schema, and every
affected review mode/log. The investigation itself did not add permanent tests
or change runtime code; implementing the selected handling is part of D/E/F.

## Approval clarifications

The maintainer approved the proposed recovery and completion behavior:

- **Recovery for legacy or unreadable records (A):** keep work
  blocked when ownership cannot be proven and rely on existing manual
  state-removal guidance after the user verifies the processes are gone.
  A dedicated state-clear command is outside this plan. Never signal a process
  based solely on a stale group number.
- **Cancellation after publication (C):** report success once the
  verified build has been atomically installed, even if cancel arrives before
  the CLI returns. This preserves disk truth without deleting a completed
  build.
- **A missing pinned component build (C):** retaining the pin is already
  required. Offer an explicit download of that exact version, then resume the
  guided operation after installation. Downloading latest must not silently
  replace the pin.

Existing 0009 policy already backfills legacy Heroic IDs during classification,
before the import confirmation. Keeping that order means declining the later
import can still leave a valid ID backfill. This is preserved behavior, not a
new proposal; changing it would reopen that part of 0009. No nonempty different
ID may be overwritten under either ordering.

The warning defaults and real manual acceptance requirement were also settled
previously. Multiple warnings on a single bulk row still need a concise
presentation during implementation; no new review prompt is implied.

## Sequence and verification

The lettered sections are implementation and acceptance batches:

| Batch | Work |
|---|---|
| A | Process ownership, cleanup, and setup recovery |
| B | Heroic identity and existing-config safety |
| C | Download completion, recovery, and guided choices |
| D | Import paths, runner selection, and dependency disclosure |
| E | Review output, logs, color, and progress |
| F | Remaining integration and workflow test coverage |
| G | Recorded manual lifecycle acceptance |
| H | Completed investigation and selected import designs, implemented in D/E/F |

1. Implement A and B first, with regression tests before moving to UI work.
2. Implement C and D, with fixture-backed tests and preserved source content.
3. Implement E and complete the remaining F workflow coverage.
4. Run independent QA, then perform and record G's manual acceptance.

Each implementation batch includes its own regression tests and appropriate
checks; F closes the remaining cross-flow gaps. A batch may contain several
logical commits. H is supporting specification, not a separate investigation
to repeat after the implementation batches.

Commit every logical change with a short message and no attribution trailers.
When delegating, use scoped coder ownership, QA for independent review, and
documentation/release agents for their matching roles. Coordinate shared
`import_heroic.rs` work instead of editing it concurrently across phases.

Required automated checks after implementation:

```text
cargo fmt --check
cargo clippy --offline --all-targets -- -D warnings
TERM=linux cargo test --offline
```

Run `cargo test --offline -- --ignored` only when both `WINEO_TEST_PROTON` and
`WINEO_TEST_DXVK` are set, the local fixtures/runtime are available, and the run
can honor the no-download constraint. Otherwise record the skip and complete
all independent verification. The prior QA needed an unsandboxed test run for
signal-handler behavior; do not mistake that sandbox restriction for a product
failure or bypass it by weakening assertions.

Check docs, comments, messages, and commit text for plain hyphens. Keep messages
terse, comments focused on why, redirected/logged output plain by default, and
all automated test I/O isolated from the maintainer's real prefixes/configs.

## Completion criteria

- Every approved fix has a behavioral regression test or recorded acceptance
  evidence, mapped back to its finding ID above.
- No foreign process is signaled and no unresolved cleanup is reported as safe.
- Existing Heroic identities/configs and source prefixes remain protected.
- Completed and canceled operations report outcomes consistent with disk state.
- Normal/verbose/logged reviews preserve the required facts and warnings.
- Automated checks and independent QA pass; real lifecycle results are recorded.
- 0008.4's selected fallback and 0008.7's selected mismatch disclosure have
  been implemented and tested through the applicable review modes.
- Known deferred behavior remains deferred, and 0007.4 remains concise.

## Acceptance results

Manual acceptance has not run. This plan is approved, with the investigation
results recorded in H; no implementation or real-game acceptance run has
been performed for it.
