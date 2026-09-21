Status: draft

# 0007: Guided game setup

## Goal

Make wineo comfortable as a standalone manager for manually installed games,
with a stable install, configure, launch, update, and remove workflow. The
maintainer wants to replace Heroic without using its store integrations.
Importing a library comes after this workflow works well on its own.

Someone can set up a game, add another launch target later, and recover from
an interrupted setup without hand-editing TOML or recreating the environment.

## Decisions

Settled by the maintainer:

- Use simple terminal prompts and numbered choices.
- `wineo setup <name>` is the entry point for an existing environment.
- Guided creation, a reusable program picker, and recovery from partial setup
  belong together. Existing commands remain useful independently.
- Check a complete game's lifecycle before treating the wizard as finished.
- Defer imports, downloads, store integrations, and the GUI.

The remaining details below are proposals for approval.

## Existing foundation

- `new` creates or adopts a prefix and can run an installer. Explicit flags
  override template settings, which override global creation defaults.
- Installed runners, templates, DXVK, and vkd3d builds can already be listed.
  The `gaming` template requires local DXVK and vkd3d builds; Proton supplies
  its own and rejects those settings. Proton prefixes are always win64.
- `programs` scans shortcuts and loose executables. Shortcuts currently give
  a label and executable, not their arguments or working directory.
- Targets already support arguments, working directory, default selection,
  display name, and desktop entries. Config edits are validated and written
  atomically while preserving comments.
- One target is implicitly the default. Several targets need exactly one
  explicit default for a launch without `--target`.
- A failed installer leaves an existing environment available. `new` cannot
  continue it because creation refuses a name that is already registered.
- `exec` runs arbitrary tools, but permits concurrent launches and does not
  wait for Wine's entire prefix to go idle. It is not sufficient by itself
  for a guided installer or updater that must exclude a running game.

## Command behavior

| Invocation | Behavior |
|---|---|
| `wineo new` | Start guided creation on a terminal. |
| `wineo new -n hades ...` | Preserve today's noninteractive behavior. |
| `wineo new --interactive -n hades ...` | Guide creation with explicit flags prefilled. |
| `wineo setup hades` | Continue setup or manage launch targets in an existing environment. |

Without `-n`, existing `new` flags may also prefill the guided flow. The wizard
does not silently replace explicit values. In guided mode, an already
registered name offers the existing setup flow rather than overwriting
anything. Noninteractive creation keeps refusing a registered name.

Interactive flows require terminal input and output. With redirected input
or output they fail before writing, explaining which existing commands to
use. Fully specified noninteractive commands never acquire new prompts.
Prompt handling is separate from the operations it invokes and is testable
without a real terminal; CLI integration tests also exercise a PTY.

Use numbered choices, visible defaults, and ordinary line input. Invalid
input explains the problem and repeats that question. A back/cancel choice
is available at each decision; EOF and Ctrl+C cancel cleanly. Prompts accept
paths containing spaces without requiring shell escaping or TOML syntax.

## Guided creation

1. Choose the environment name and prefix location. Default to the configured
   prefix root. An existing prefix is adopted in place through the current
   `new --dir` behavior; no copying, moving, or library import is added.
2. Choose an installed runner and an optional template, including no template.
   Preserve current flag/template/global precedence and display the effective
   settings. Show unavailable runners and incompatible templates with a
   reason instead of selecting a combination that cannot work.
3. Choose how to install: run a local EXE/MSI installer, use an existing
   executable, or finish with an empty environment. Existing executables can
   live inside or outside the prefix; wineo does not move their files.
4. Review the name, location, runner, architecture, template settings, and
   installation choice before creating anything. Validate local paths and
   required tools/builds first. Explain umu's first-run runtime download when
   applicable; wineo does not add its own download manager.
5. Create/adopt the prefix, apply the settings, and run the installer if
   selected. Wait for the installer and its remaining Wine processes before
   scanning. Continue into the shared target picker.
6. Review and save selected targets, then print the commands to start the
   default, start another target, revisit setup, and inspect logs. Do not
   launch the game automatically.

The plain flow does not silently enable DXVK, vkd3d, gamemode, protonfixes, or
a template. Selecting a template shows what it enables. Missing builds have
a useful explanation and a way back to the runner/template choice.

## Existing environments and the shared target picker

`setup` first shows the current environment and its targets, then offers:

- Add targets from detected programs or a supplied executable path.
- Edit a target's executable, arguments, working directory, display name,
  default selection, and desktop setting.
- Remove a target, with a review of the target and desktop entry affected.
- Run a local installer or updater, then scan again and review targets.
- Finish without changes.

Removing a target leaves the program's files and prefix intact. Removing an
environment continues to use `rm`, including its existing `--delete-prefix`
distinction. Environment-level runner and component changes remain available
through `set`; this plan does not build an interactive editor for every key.

The picker uses the same scanner and filtering rules as `programs`, keeping
shortcut and loose-executable sources visible. Offer multiple selections,
showing filtered results on request, entering a path, and skipping for now.
An empty scan remains usable through manual path entry.

For each selection:

- Suggest a valid target identifier from its label, with an editable fallback
  when no valid characters remain. Resolve name collisions explicitly.
- Show the executable and whether it is already used by a target. Repeated
  setup does not silently add duplicates or replace an existing target.
- Suggest the shortcut label as the desktop name. Target identifiers remain
  what `--target`, `list`, and `status` display, as settled in 0006.
- Offer arguments as individual values and an optional working directory.
  Preserve argument boundaries and use the existing executable-directory
  default. Explain that shortcut arguments are not currently imported.
- Ask whether to create a desktop entry, defaulting to yes for a newly chosen
  game target. Preserve existing desktop choices when editing.

Preserve an existing default unless the user changes it. If several targets
will remain without a default, ask which should be launched by `start <name>`.
Changing defaults clears the old value and sets the new one in one validated
config write. Removing the default with several targets remaining asks for
its replacement. Broken configs are reported with their path and error;
automatic repair is outside this plan.

Stage target edits until the final review, then apply the batch and reconcile
desktop entries. Never lose an existing target's unrelated settings. If
desktop reconciliation fails after saving the config, report what was saved
and what failed; revisiting setup can retry it without reinstalling the game.

## Foundation and recovery

Add a shared installer operation for guided creation and existing setup. It
uses the environment's runner and `Launch::exec` semantics, including MSI
handling, rather than treating the installer as a game target. Keep ordinary
`exec` behavior unchanged.

Installer/update work holds an exclusive operation lock for the environment
and prefix, with an accurate operation description. Refuse a tracked running
game or an unresolved lost run. For Wine, retain the lock until the prefix is
idle, including installer handoffs to children. For Proton, retain it through
umu completion and use the existing process-group model, not a wineserver.

The installer operation owns its child/process group and cancellation; it
cannot just call today's `exec::run_attached`. Cancellation requests shutdown,
then uses the existing timeout/force-stop pattern if necessary. Only this
environment's Wine prefix or the installer's Proton process group may be
stopped. Retain the lock until cleanup completes. Failure to establish that
cleanup succeeded must be
reported as unresolved, never as a successful setup ready for another run.
Retain the runner/prefix or process-group information needed to recover in
the existing runtime area, and refuse conflicting work until it is resolved.
This is operation recovery state, not a saved wizard conversation.

Before creation, canceling writes nothing. After an environment is registered,
canceling or an installer failure keeps the config, prefix, and prior targets.
The result says what exists and prints `wineo setup <name>` to continue. An
explicit finish/skip is success; cancellation exits 130, and installer failures
retain their nonzero exit outcome.

If both the installer and idle/cleanup handling fail, report both. Unresolved
cleanup takes precedence in the final failure status and recovery guidance;
the installer exit code must still be visible. Do not scan or offer a retry
while the operation's processes may still be active.

If prefix initialization fails before registration, report the directory and
failure accurately. Do not claim `setup <name>` can resume a nonexistent
environment or adopt a partial prefix as healthy. Preserve existing core
checks and provide the appropriate retry/inspection guidance.

Resuming uses the config and a fresh prefix scan. No wizard journal, hidden
installation-complete flag, or automatic installer retry. The user chooses
whether to run an installer again or go straight to target selection.

Collect answers without holding a prefix lock. Before applying changes,
re-read and validate the environment under the appropriate lock. If an input
to the reviewed change has changed meanwhile, refresh the review. Config
updates need a short write lock shared with `set` so simultaneous edits cannot
overwrite each other. Use a separate exclusive lock keyed by config file in
the existing locks directory, shared by config creation, editing, and removal.
Take any required operation locks first, then the config lock; reload and
validate before committing. The config lock is independent of the runtime
lock, preserving `set`'s ability to change next-launch target settings while
a game is running. Release locks before returning to ordinary prompts.

## Scope limits

No prefix migration, library/store import, downloader, full-screen TUI, GUI,
automatic compatibility database, shortcut argument parser expansion, or
general diagnostics command. Dynamic shell completion stays a later polish
item. Bugs that prevent the lifecycle below belong here; unrelated hardening
gets a separate plan.

## Testing and acceptance

- Unit tests for choices, validation, target-name collisions, argument
  boundaries, default changes, and config batches that preserve existing keys.
- Fake-runner integration tests for guided creation, adoption, existing
  executables, installer updates, picking multiple targets, manual paths,
  empty scans, repeated setup, and immediate desktop reconciliation.
- Recovery tests for canceled prompts, EOF, invalid input, failed creation,
  failed/spawn-failed/interrupted installers, and continuation without rerunning
  an installer. Assert which files and targets survive each case.
- Locking tests for game/install conflicts, a Wine installer handing off to a
  child, Proton completion/cancellation, and config edits made while a prompt
  waits. Keep other environments usable and untouched.
- CLI tests for TTY detection, PTY interaction, help/completion registration,
  and unchanged flag-only/noninteractive behavior.
- Run fmt, clippy, and the normal suite. Real Wine GUI tests use throwaway
  prefixes and XDG roots plus a dedicated display/session. Never fall back to
  opening test windows on the maintainer's gaming desktop.
- With a maintainer-chosen game, record a manual acceptance run: create,
  install, select targets/default/desktop entries, launch, change arguments
  or working directory, stop, run an updater, relaunch, and remove. Check a
  desktop launch and icon grouping in the dedicated graphical session.

The core acceptance flow is:

```text
wineo new                  # guided creation and target selection
wineo start hades          # chosen default works
wineo stop hades
wineo setup hades          # add/edit a target or run an updater
wineo start hades --target config
wineo logs hades
wineo stop hades
wineo rm hades             # removes registration and entries, keeps prefix
```

Also exercise `rm --delete-prefix` with a disposable environment. The standard
game setup and maintenance flow requires no manual TOML edits. Existing
commands remain enough to perform the same operations without the wizard.

## Phases

1. Exercise the existing lifecycle and implement the shared installer,
   cancellation, and config-write coordination needed by this plan.
2. Add terminal prompts, the shared target picker/editor, and `setup` for
   registered environments, including recovery and installer/update actions.
3. Connect guided `new`, preserve noninteractive behavior, finish help and
   usage documentation, and complete isolated real-game acceptance.

Commit each logical change separately. Each phase ends with tests, fmt, and
clippy passing, then a push. Amend the draft before implementation if review
changes the command contract or scope. Only the maintainer approves this plan.
