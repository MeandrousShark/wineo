Status: done

# 0038: GUI new, import, and clone

## Goal

Create, import, and clone environments from `wineo-gui`, including native
games, installers, existing prefixes, and Heroic's manually added games.
Reuse core's operations and the settings page from 0037, with a review
before anything is registered or copied.

This is the next GUI parity plan after
[0037: GUI settings](0037-gui-settings.md). Removal, trash, and validate
follow in 0039. General artwork, download, snapshot, portable-copy, exec,
and winetricks tools follow in 0040.

## Settled by the maintainer

- Manual artwork selection is a first-class option alongside SteamGridDB.
  Each art kind has **Browse...**, available without a key or network access.
- The maintainer approved the recommendations on 2026-09-29. The new
  environment finish page also has **Launch game when this page closes**,
  unchecked by default. It is available only with a playable target and
  resolved installer cleanup. Leaving that page starts the chosen default
  target once when checked; imports and clones keep their no-launch default.

## Maintainer refinements

The maintainer approved these changes during acceptance testing:

- Add, creation, import, and clone use a compact modal overlay above the
  visible library. Initial Back returns to Add choices, with existing draft
  discard prompts preserved. Context menus open near their triggering pointer.
- Artwork uses clickable poster, icon, logo, and hero placeholders/previews
  instead of visible path fields. Clicking opens Browse; manual path entry
  remains an optional fallback. Title-based fetching may populate previews
  when a key and global art settings allow it, without replacing local art.
- Windows creation groups infrequent options in a collapsed Additional wine
  options section. Template selection shows a compact summary of its changes.
- Dismissing a chooser without a selection is silent, including portals that
  report the dismissal as an Other response.
- New environment previews the game's page: a hero banner with the logo in
  its corner like the library panel, the poster beside the form, and the
  icon beside the name. Art controls and Back/Next align with the form, and
  narrow windows stack the poster and icon below it. Hero, logo, and icon
  status share one line under the banner.
- Choosing a SteamGridDB match downloads each enabled kind right away and
  shows it in its tile; creation stores those bytes instead of fetching
  again. Changing the match cancels superseded fetches before decoding.
- Logo and hero ranking prefers big enough over biggest (logos 1200 px wide,
  heroes 1920 px), and images over 4096 px on the long side are used only
  when nothing smaller exists. This also changes CLI `art fetch` picks. The
  spinner tick runs only while something spins, so the idle GUI stays quiet.
- New environment's Name is the game's title for the desktop entry; the
  environment name is derived from it (`name::from_title`, capped at 64
  bytes) with a live preview and an Edit override. Desktop entry names
  escape backslashes.
- GUI logs (panel and installer) are selectable, with Copy and Jump to end.
  Force stop replaces Cancel only after canceling, in every flow. The
  spinner is a smooth rotating arc, and the winetricks settings field calls
  its entries packages.
- Library launches that install winetricks packages show which one is
  installing ("2 of 3") and offer the install log live while it runs.
- Heroic review opens at the top and shows only the anti-cheat note (once),
  blocking findings, and the shared prefix note when adopting in place.
  Import, runner copy, and download progress read as sentences with file
  counts, throttled to 100 ms, and batch statuses lead with "(2 of 5)". Skip
  reasons and desktop entry notes are sentences too.
- Import sources are fingerprinted by metadata instead of content, with
  progress and cancel while checking (`ImportProgress::Checking`). Snapshot,
  clone, and portable keep the content fingerprint, since restore markers
  persist it and a rename changes the moved folder's ctime. A folder holding
  other prefixes is refused everywhere. A Heroic runner wineo already copied
  is recognized from its receipt instead of offered for copying again.
- New environment's starting point is a radio group: Run an installer
  (Windows only), Existing install, and Empty registration. Its review
  leaves artwork out, since Details already picks it. Choose game target
  leads with a Confirm button and offers Skip for now as a quieter link.

These refinements supersede the full-page and visible-art-path details below.
The implementation keeps SteamGridDB requests explicit; entering a name
seeds its search field without starting a network request.
Displayed GUI checks verified pointer placement, the library behind the
overlay, Back to Add, silent Other chooser dismissal, clickable art tiles,
and the gaming-template summary. Regression checks also cover modal keyboard
isolation and stale title/configuration replies.

## Amendment: Heroic poster grid

Amendment status: approved on 2026-09-29. The maintainer
chose each option below during acceptance on 2026-09-29.

Heroic import's game step becomes a poster grid with multiselect, replacing
the single-game list and the separate import-all path.

- Core reads each library entry's `art_square` URL (Heroic's portrait
  poster) and resolves the cached copy at `images-cache/<sha256 of the URL>`
  under that source's config directory, native or Flatpak. Lookups never
  touch the network or change Heroic's files.
- Each tile shows the cached poster, decoded off the UI thread at tile size,
  with the title below. A missing or unreadable poster shows a title
  placeholder instead; the picker makes no network requests.
- Already imported and unsupported games stay visible but dimmed and not
  selectable, with the existing reason under the title.
- Clicking a tile toggles its selection. **Select all** selects every
  eligible game and **Select none** clears the selection. Keyboard: arrows
  move, Space toggles, Ctrl+A selects all, Enter imports.
- **Import N games** is disabled with nothing selected. One selected game
  opens today's editable single review; two or more open the batch review
  with a plan built only from the selection. Batch results, cancellation,
  and art fetching work as they do today.
- When an imported game has a cached poster and no local Grid pick, it is
  stored as the environment's Grid art through `art::set_manual`, named
  `heroic-<app name>.<ext>` so its provenance reads as a manual pick from
  Heroic without an `art.toml` schema change. SteamGridDB fetching then
  fills only the kinds still missing.

Tests cover cache path resolution for native and Flatpak roots, placeholder
fallback for missing and invalid files, selection with ineligible games,
select all and none, one versus several selected, a batch plan limited to
the selection, the stored poster and its provenance with SteamGridDB not
replacing it, and Heroic's files staying byte-for-byte unchanged.
Acceptance adds: pick one game and several games from the grid, including
Select all then deselecting some.

## Existing foundation

- `create::run` accepts a name, prefix location, runner, architecture,
  template, optional executable, and desktop-entry choice. It reports
  `CreateEvent`s and returns `Created`, which holds the create lock through
  a subsequent installer. A missing executable can leave a registered
  environment without a target; the GUI must show that outcome.
- `Environment::create_native` registers a native environment, optionally
  with an executable and desktop entry.
- `Installer::prepare_with_runner` takes the create guard and the concrete
  runner. `Installer::run` watches the installer and its descendants,
  supports cancellation, logs output, and distinguishes finished,
  canceled, and unresolved cleanup.
- `programs::scan`, `targets::Batch`, and the settings target controls
  already support choosing programs and adding targets without a terminal.
- `import::inspect` and `import::execute_reviewed` inspect and copy or adopt
  Wine prefixes and Proton compatdata, preserving the existing source
  checks, locks, external-dependency review, and recovery behavior.
- `heroic_import` has read-only single-game and library planning, typed
  findings, editable plans, missing-build information, source-change
  refusal, and Windows/native apply operations. See
  [Core API](../core-api.md#heroic-import).
- `clone` has separate Wine and native inspections and execution. Wine
  clones copy prefixes and art; native clones copy registration and art.
  External game files stay in place, desktop entries are disabled, the
  Heroic id is removed, and playtime starts at zero.
- Runner and template lists, download clients, SteamGridDB search, and
  art-fetch primitives exist in core. Guided question order and some
  orchestration still live in the CLI's `wizard.rs` and import commands.
- The GUI runs blocking work on workers and already has progress,
  cancellation, generation ids, settings dirty tracking, and both palettes.
  It has no file chooser dependency yet; 0037 explicitly deferred that to
  this plan.

## Proposed design

The design below was approved by the maintainer on 2026-09-29.

### Entry points and page shape

Add an **Add** button beside search, opening a choice of **New environment**,
**Import prefix**, or **Import from Heroic**. An empty library shows the
same entry point. Add **Clone** to a detail-panel actions dropdown and to
a card's right-click menu. Put the existing **Settings** action in both
menus too, while keeping its current panel button.

Use full pages with Back, Next, and a final named action, like **Create**,
**Import**, or **Clone**. Short flows need only Details and Review;
Heroic adds a source/game selection page. Preserve answers when going
back. Leaving a filled form asks before discarding it, using the settings
page's existing prompt style.

Keep one foreground mutation flow at a time. Refreshing the library and
watching already-running games continues. Worker and chooser replies have
generation ids, so leaving or changing a source cannot apply stale results.

### New environment

Offer **Windows** and **Native Linux**. Windows offers three starting modes:
an empty environment, an existing game executable, or an installer. Native
offers an executable or an empty registration. Empty environments remain
valid, since targets can be added later.

Common fields: name, optional desktop entry when there is a target, and
optional art fetch. Windows adds template, runner, architecture, and prefix
location. Resolve the template and global defaults with the same precedence
as the CLI. Show the concrete runner alongside an alias, pin that runner
for an active installer, and force win64 for Proton. Show omitted template
components before creation rather than silently dropping them.

Show the resolved default prefix path even when no override is set. If the
chosen folder already contains a compatible prefix, explicitly say it will
be registered in place. Refuse incompatible or non-prefix occupied folders
using core's existing checks. A name with an existing config is a conflict:
offer to open that environment's Settings instead of replacing its config.
The existing Play path keeps its config-first prefix creation behavior.

Keep general launch tuning on the existing settings page. Once creation
finishes, offer **Open settings** and **Back to library**, selecting the new
card and clearing a search that would hide it. The finish page has **Launch
game when this page closes**, unchecked by default. Closing the page through
Back to library or Open settings launches once only when checked and there
is a playable target with resolved installer cleanup. Closing the application
does not launch a game.

### Installers and target setup

File picking and typed paths both work for `.exe` and `.msi` installers.
Validate the installer before creating the environment. After creation,
keep `Created` and its guard alive until it is handed to the installer;
there must be no unlocked gap. Log installer output instead of inheriting
the GUI's terminal.

Show creation and installation progress, a log view, Cancel, and then Force
stop after cancellation. Report the installer's exit code separately from
whether its descendants have stopped. Only resolved cleanup permits target
setup. Unresolved cleanup offers **Clean up** through the existing core
stop path, and keeps the environment visible in the library.

After a successful installer, scan programs and open the existing target
editor with the installed choices plus a file picker and typed path. The
user chooses the game's target; never register the installer as the game.
Target setup can be skipped, or continued after a failed/canceled installer
whose cleanup resolved. Keep the environment and explain what completed.

### Prefix import

Fields: source folder, name, compatible runner, **Copy** or **Use in place**,
and destination for Copy. Copy is the default. Detect layout and architecture
from inspection; offer an architecture choice only when core cannot detect
it. Explain that a Proton `pfx` source imports the full compatdata root.

Review the normalized source and destination, runner, layout, architecture,
external links and game paths, and the instruction to stop external
launchers. Show an expandable full dependency list rather than hiding it
behind a verbose flag. Execution uses the exact inspected review, and
source changes require a fresh review.

After registration, offer target setup through the same controls used
after installation. If registration fails after a completed copy, report
the retained folder and offer to reopen Import with **Use in place** and
that path prefilled. Do not delete or automatically adopt the retained copy.

### Heroic import

Discover native and Flatpak Heroic sources and allow choosing a config
folder. List supported manually added games with title, platform, and
already-imported state. Store games and unsupported platforms explain why
they cannot be imported.

A single game opens an editable `GamePlan` or native plan: name, executable,
argument chips, working directory, desktop entry, imported launch settings,
and Copy/Use in place for Windows. Resolve with guided defaults and present
all omissions and warnings. Unparsed arguments require explicit input;
blocking findings disable Import. Use core's runner suggestion, including
copying a Heroic runner where supported. Heroic files are never written.

Also offer **Import all eligible games**, matching the CLI's library flow.
Review every ready, pending, skipped, and already-imported row, with one
confirmation. Run games sequentially, continue past individual failures,
and show per-game results. Cancel stops the active operation and the queue,
keeping completed imports. Do not start imported games.

Defer legacy Heroic-id backfills until the user confirms Import, so opening
or canceling a review is read-only. Revalidate plans before applying; changed
Heroic settings return the user to review instead of applying stale choices.

### Clone

Ask for the new name and, for Wine, an optional prefix destination. Use
core's Wine or native inspection for the review. Show target paths that
stay external, copied art, disabled desktop entries, cleared Heroic id,
reset playtime, and literal source paths that cannot be rewritten safely.

Before Wine clone inspection, wait for the source's Wine processes to exit
through `transfer::wait_idle`, as the CLI does, on a cancellable worker.
Explain that external launchers must also stay idle during the copy.

Native clone clearly says it copies config and art while sharing the game
files; it has no prefix destination. Wine clone uses the existing reviewed
copy and recovery behavior. Both finish with the new card selected.

### File picking, downloads, and art

Use an asynchronous XDG portal file/folder chooser on Linux, with typed
paths as a fallback when no portal is available. Add the picker to existing
target executable and working-directory controls too. Canceling a chooser
leaves the previous value intact. Preserve core `Paths` rules, including
portable paths, rather than implementing a second resolver.

Include contextual runner and component installation when creation or
Heroic import needs it, with an explicit download/copy confirmation and
progress. Successfully installed builds remain after a canceled flow.
Rebuild the review after installing them. General download management stays
in 0040.

Offer manual artwork in New and import flows, including each game's Heroic
review. Icon, Grid, Hero, and Logo each have **Browse...**, a preview, and
**Clear selection** before registration. Typed file paths remain available
when the portal is unavailable. Local selection does not require a
SteamGridDB key and remains visible when all automatic-fetch switches are
off. Users can mix local files and fetched kinds; a local selection wins
for its kind.

Validate selected files by contents using core's supported formats. Keep
selection and preview read-only until registration, then copy files into
the environment's art store with `art::set_manual`, preserving the source
files. Manual art failure keeps the registered environment and offers retry
or skip. Reconcile desktop entries after art is stored so local icons apply.
Clone continues copying the source's art rather than asking for replacement
art in this flow.

Include optional SteamGridDB fetching for New and Heroic import, honoring
the four global art-kind switches. With a configured key, single games
offer title search and match selection; batch import uses the same top-match
policy as the CLI. All-off hides automatic fetching. A missing key explains
why fetch is unavailable; key management and an artwork browser stay in 0040.
Art failure leaves registration successful, with a retry or skip choice.
For Heroic, retain `Registered` through manual art and fetching and call
`finish` even when art is skipped or fails, so desktop entries are reconciled
afterwards.
Do not claim immediate cancellation for requests that lack token support.

### Failure, cancellation, and closing

Review and Back write nothing. Explicitly confirmed prerequisite downloads
are separate operations and identify the files they will retain.

The progress page distinguishes work before registration, a registered
environment, and a completed copy awaiting registration. Refresh library
state after errors and cancellation; an error does not imply nothing changed.
Reuse core cleanup and recovery rules rather than attempting GUI rollback.

Intercept closing the window while a mutation is active. Offer **Keep
working** or **Cancel and close**, and wait for the worker's result before
closing; allow Force stop while waiting. Already-running games keep their
existing detached behavior. Do not abandon a worker holding an installer or
copy operation when the GUI exits.

## Decisions for the maintainer

All five recommendations were approved on 2026-09-29, with the opt-in launch
checkbox above added to the new environment finish page.

1. **Page shape and entry points:** approve the full-page flows, Add menu,
   and shared card/panel actions menus, or prefer a different arrangement?
2. **Heroic scope:** include single-game and batch import in 0038 as
   proposed, or split batch import into a follow-up plan?
3. **Downloads and art:** include contextual prerequisite installation and
   post-registration art fetching now, alongside the confirmed manual file
   selection, while keeping their general managers
   in 0040?
4. **After registration:** open target setup when needed, otherwise offer
   Settings or return to the selected new card, without launching a game?
5. **Window close:** cancel and wait for an active mutation, rather than
   allowing it to outlive the window? Backfills also wait for confirmation.

## Implementation sequence

1. Page routing, Add and actions menus, draft state, keyboard handling, and
   portal file/folder picking, including existing target controls.
2. New Windows/native flows, template/default resolution, review, worker
   progress, partial-success reporting, and existing-config refusal.
3. Installer continuation with retained guard, logs, cleanup, and shared
   post-registration target setup.
4. Prefix import and Wine/native clone using reviewed core operations.
5. Heroic sources, editable single-game plans, batch review, and queue.
6. Contextual prerequisites, manual art selection, and art fetching; extract
   shared orchestration into core if it would otherwise duplicate CLI policy. Keep CLI wording
   and behavior unchanged.
7. Closing/cancellation verification and docs: `docs/gui.md`, features,
   README, and core API docs for any extracted operations.

Before implementation, identify any core gaps found while connecting these
flows. Small behavior-preserving extractions fit this plan; new policy or
storage behavior requires an amendment approved by the maintainer.

## Testing and acceptance

- GUI state tests cover backtracking, validation, draft discard, stale
  chooser/worker replies, review invalidation, duplicate submissions,
  native/Wine field visibility, and selecting a result hidden by search.
- Worker tests use temporary XDG roots, fake runners/installers, and local
  prefix/Heroic fixtures. Cover create-lock continuity, MSI handling,
  installer outcomes, partial creation, copy recovery, and source changes.
- Cover batch mixed success, skipped/already-imported rows, canceling the
  queue, and backfills only after confirmation. Heroic source files remain
  byte-for-byte unchanged.
- Test download/art orchestration with fake transports, including declined
  downloads, retained completed builds, missing keys, all-off art, and art
  failure after successful registration. No real network is needed.
- Cover manual art without a key/network, all automatic-fetch kinds off,
  mixed local/fetched kinds, chooser cancellation, invalid file contents,
  preserved source files, stored manual provenance, previews, and desktop
  icon reconciliation.
- Verify comment preservation and portable path handling when reusing
  existing configs and target settings. CLI integration tests stay green.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test`,
  both release builds, and the frozen serial workspace packaging gate pass.
- Isolated GUI smoke tests exercise both palettes, chooser cancel/fallback,
  keyboard-only navigation, progress/log views, and window-close handling.
  Agents use fake programs; real games and installers are maintainer tests.
- Maintainer acceptance: create from an installer and an existing exe,
  register a native game, import a prefix, import single and batch Heroic
  games, clone Wine and native environments, and try cancellation during
  copying and installation. Verify the results in both GUI and CLI.

## Implementation verification

Implemented on 2026-09-29. Maintainer acceptance with real games and
installers passed on 2026-09-30.

- Formatting and workspace clippy with warnings denied passed.
- The full suite and frozen serial workspace gate passed. The latter ran
  1,518 tests, with 16 prerequisite-dependent tests ignored.
- Both CLI and GUI release builds passed. Follow-up GUI checks passed all
  160 GUI tests after the final artwork and review-display adjustments.
- Independent code review found no remaining actionable defects after the
  reviewed-adoption, desktop-lock, draft-discard, and stale-reply fixes.
- Isolated GUI smoke checks used temporary storage and fake programs:
  Windows creation, manual artwork previews/storage, Wine clone, native
  Heroic import, keyboard creation, light/dark rendering, unavailable and
  canceled file choosers, opt-in launch once, and installer cancel-and-close.
  No real games, installers, or network downloads were used.

Maintainer acceptance passed:

- 2026-09-29: registering a native game, single and batch Heroic import,
  Wine and native clone, and cancellation during copying and installation.
- 2026-09-30: creating from a real installer, creating from an existing
  exe, and importing a prefix.

Released as v0.19.0 after maintainer acceptance passed.

## Scope limits

- No changes to the config schema, import ownership, copying semantics, or
  supported Heroic platforms and libraries.
- No remove/trash/validate, general downloads or art management, snapshots,
  portable copies, arbitrary exec, or standalone winetricks UI.
- No copying external game folders or editing Heroic settings. Launch after
  creation requires the unchecked-by-default finish-page checkbox; imports
  and clones do not launch automatically.
- No gamepad navigation, alternate library layouts, or visual-polish redesign.
