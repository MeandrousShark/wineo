Status: done

# 0042: GUI visual polish

## Goal

Make the existing desktop app feel finished: give the detail actions a
consistent shape and clear interaction states, add depth and motion to the
detail panel, make the new-environment review easy to scan, and carry the
same visual treatment through Settings, Snapshots, Downloads, Checkup,
and Trash.

## Settled by the maintainer

- On 2026-10-02, the maintainer picked UI polish as the next plan and asked
  to investigate the flaky crashed Proton installer test first.
- The maintainer picked compact rounded rectangles for the visual previews
  and requested interactive HTML mockups before implementing in Wineo.
- On 2026-10-02, the maintainer liked the previews and requested the same
  design for Settings, Snapshots, Downloads, Checkup, and Trash. The expanded
  previews use the Grouped direction as their starting point.
- On 2026-10-03, the maintainer approved implementation of the expanded
  Grouped design: compact rounded rectangles, short slide and fade, and
  grouped reviews and management sections. The HTML preview stays unchanged.
  Posters should be smaller in the actual app, and the design must be
  documented for future updates.

## Decisions during implementation

- Use 160 x 240 px library posters, with 16 px gaps, instead of the preview's
  oversized artwork. Preserve the 2:3 ratio and existing keyboard grid logic.
- The reusable visual specification lives in [GUI style guide](../gui-style.md).
  Mockup sample data is illustrative; existing core data and operations remain
  authoritative.

## Approved acceptance refinements

On 2026-10-03, the maintainer requested the following refinements and settled
the title and launch-label rules through follow-up answers:

- Use the default launch target's configured desktop entry title for Library
  cards. Fall back to the environment identifier if the default target or its
  desktop title is unavailable. Search matches both title and identifier.
  Keep identifiers as operation and selection keys; changing the selected
  launch target does not rename the card.
- Use **Play** when an environment has the `game` tag and **Launch** otherwise.
  The built-in `gaming` template adds that tag to future creations. Existing
  environments are not classified from runner or performance options; they
  can be tagged through their settings.
- Add **Launch target:** beside the panel chooser and give every dropdown
  and its menu the shared rounded, bordered, palette-aware treatment.
- Match the Library context menu and panel Actions menu to the same design,
  preserving their operations and enabled rules.
- Center Checkup button labels. Keep Play/Launch and Stop on the first panel
  action row, with Log and the secondary controls on the next row.
- Clicking unused Library space closes the detail panel. Card selection,
  scrolling, menus, keyboard navigation, and immediate cleanup retain their
  existing behavior.

These refinements extend the original scope to library display names and the
existing tag-based launch label. The approved amendment below adds motion
between grid positions during panel reflow.

## Approved grid-motion amendment

Status: approved

On 2026-10-03, the maintainer selected the playful settle preview with a
300 ms duration and approved implementation, asking for smoother movement
with a little more jiggle. Use a continuous damped spring with a small stagger
and bounded overshoot, instead of the preview's segmented keyframes. Keep
posters upright and readable, and stop all animation when the cards settle.

Animate cards between their previous and final grid slots when the panel
opens or closes. Compute the destination layout once, then interpolate card
positions over a bounded transition. Keep the scrollable widget tree stable
and move hit areas with their cards. Use an opaque docked panel overlay and
input shielding so cards cannot receive clicks through the panel.

Implementation keeps the panel content at 420 px and exposes a 10 px library
scrollbar strip beside it. The strip remains interactive, so the full-width
viewport's scrollbar stays accessible without a second scrolling mechanism.

The selected playful motion supports reversals from the current positions. Filters,
navigation, window resizing, and changed library contents finish or retarget
the motion safely. Preserve scroll position, reachable rows near the bottom,
keyboard column navigation, visible artwork loading, and immediate panel/log
cleanup. Verify these behaviors in both palettes and small windows before
landing the layout change. This replaces the original snap at the single
reflow; it does not resize or rewrap the grid on every animation frame.

## Existing foundation

- `theme.rs` owns light and dark palette tokens and follows the desktop.
- `panel.rs` has a 420 px docked detail panel, hero/logo art, target selection,
  launch actions, progress, errors, and a log view. Play and Stop use a custom
  32 px button with a 6 px radius; Log uses the built-in button style. The
  custom action style does not distinguish hover and pressed states.
- `main.rs` and `grid.rs` account for the panel width when laying out the
  library. Selection, panel visibility, log visibility, and live operations
  already have separate state.
- `flow/new.rs` renders the reviewed environment as a sequence of text rows,
  followed by prerequisite downloads and Create/Back. Core preparation and
  frozen reviews already own validation and creation decisions.
- Settings already has grouped sections, unsaved edits, Save/Revert, and
  conflict handling. Snapshots, Downloads, Checkup, and Trash own their
  review, confirmation, progress, recovery, and result states separately.

## Approved visual direction

### 1. Detail action shape

Both options retain visible text labels and the current action availability.
The examples use the same inactive and running environment.

| Scenario | A: compact rounded rectangles (recommended) | B: pill buttons |
|---|---|---|
| Inactive: Play, Stop, Log | Shared height, 8 px corners, and padding; Play uses the primary fill, disabled Stop is muted, Log is a neutral surface | The same roles and height, with fully rounded ends |
| Running: Play, Stop, Log | Disabled Play is muted, Stop uses the danger fill, Log keeps the same neutral surface | The same running states in the pill shape |
| Hover, press, keyboard focus | Explicit hover/pressed colors and a visible focus treatment within the same shape | The same feedback within the pill shape |

Selected: A, using the existing palette. Carry the shared geometry to
the panel's Settings, Art, Actions, Cancel, Force stop, and Clean up controls
where they appear. Secondary actions stay visually quieter than Play/Stop.
Check both palettes, including disabled text and focus visibility.

### 2. Panel motion

Both options retain the current docked panel and add a subtle shadow and
surface separation that works in light and dark mode.

| Scenario | A: short slide and fade (recommended) | B: short fade |
|---|---|---|
| Open the same environment | About 160 ms of restrained motion from the right, with a fade | About 120 ms of fading into its final position |
| Close the same environment | Reverse the transition; the closing panel stops accepting input immediately | Fade out; the closing panel stops accepting input immediately |
| Select another card while open | Update the selected content without replaying the open transition | Update the selected content without replaying the open transition |
| Rapid open/close or navigation | Reverse from the current position, or finish immediately when leaving Library | Reverse from the current opacity, or finish immediately when leaving Library |

Selected: A. The approved grid-motion amendment replaces the initial width
reservation and snap with a stable full-width viewport and docked panel overlay.
Compute final grid geometry once per open/close and animate card positions
without rewrapping on every animation frame. Tick only while motion is active. Logical close,
log cancellation, and selection changes take effect immediately; animation
must not delay cleanup or keep hidden controls interactive.

### 3. New-environment review layout

Both options show the same existing reviewed fields and warnings. Long
paths wrap or scroll without hiding their full text, and the confirmation
controls remain easy to reach in a short window.

| Scenario | A: grouped summary (recommended) | B: separate summary cards |
|---|---|---|
| Windows install: name, source, program, runner, architecture, prefix | One summary surface with labeled Environment, Source, and Runtime sections | The same fields in three separate cards |
| Native registration: name, source, program | The same layout, omitting Windows-only rows | The same cards, omitting the Runtime card |
| Existing prefix or missing prerequisite | A separate warning/prerequisite area below the summary, above Create/Back | The same warning/prerequisite area below the cards |
| Long program and prefix paths | A consistent label/value layout with wrapping and a bounded details area | Wrapping inside the matching card and a bounded details area |

Selected: A. Use spacing, labels, and muted secondary text for
hierarchy. Keep warning and prerequisite controls prominent. Rearrange the
existing review data without recomputing creation decisions in the view.

## Management pages

Apply the selected geometry, palette, surface depth, spacing, and focus
treatment to the existing pages. The previews keep their current operations
and use simulated data and actions.

- **Settings:** grouped launch defaults, new-environment defaults, storage,
  run limits, art switches, environment variables, and key source. Keep the
  unsaved-edit count and Save/Revert visible. Apply the same treatment to
  environment settings during implementation.
- **Snapshots:** environment and label controls, a readable snapshot list,
  bounded create/restore reviews, config comparison, deletion confirmation,
  progress, and completion results.
- **Downloads:** grouped runner and graphics-component lists with their
  uses, unused-build deletion, release selection, and install progress.
- **Checkup:** grouped problems and fix choices, check/fix progress, batch
  results, and a concise healthy state.
- **Trash:** removed entries, prefix inclusion and retained paths, recovery
  notices, restore results, and the existing deletion confirmations.

Show populated, empty, busy, and error examples where they apply. Keep long
values readable and actions reachable at small window sizes. Presentation
changes must preserve review validity, recovery rules, operation ownership,
and results when navigating between pages.

## Implementation sequence

1. **Settle the visual direction.** Record the maintainer's answers above.
   Prepare matching light/dark previews of the chosen button, panel, and
   review treatments for feedback before applying them throughout the app.
2. **Shared detail actions.** Add scoped styles/tokens for geometry and
   interaction states, then apply them to the detail panel. Preserve each
   action's messages, enabled rules, and keyboard behavior.
3. **Panel depth and motion.** Add presentation state and bounded animation
   updates around existing open/close behavior. Preserve library scrolling,
   target selection, art, log handling, and operations during rapid changes.
4. **Review layout.** Group the existing reviewed fields and warnings,
   handle optional/native fields and long values, and keep Create/Back
   accessible without changing the frozen review or prerequisite flow.
5. **Management pages.** Apply the shared treatment to Settings, Snapshots,
   Downloads, Checkup, and Trash. Preserve forms, confirmations, progress,
   empty/error states, and ongoing work across navigation.
6. **Review, docs, and acceptance.** Independently review the interactions,
   run the required checks, update `docs/gui.md` and the feature backlog,
   and prepare maintainer acceptance. Release only after acceptance.

## Testing and acceptance

- Unit tests cover animation transitions and reversals, stale ticks after
  navigation, and logical close taking effect before its animation ends.
- Existing action-enabled, keyboard, selection, log, navigation, and creation
  tests continue to pass. Add behavior tests only where the polish introduces
  state transitions or layout decisions needing protection.
- Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo test`. Verify visuals in both palettes at ordinary and small window
  sizes. Ask before opening a GUI window for a smoke test.
- Maintainer acceptance uses throwaway Wine/Proton and native environments:
  compare inactive/running/busy/error action states; exercise mouse and
  keyboard focus; rapidly open, close, and switch cards; open/close logs
  during a run; and review an installer, an existing prefix, and a native
  registration with long paths and missing prerequisites.
- Confirm the library keeps its scroll position, actions remain responsive,
  animation stops when idle, and no warning or review detail is lost.
- Check Settings edits, Save/Revert, and conflicts; snapshot create/restore
  reviews and config comparison; used/unused download rows and installation;
  Checkup selection and results; and Trash restore, recovery, and deletion
  confirmations. Verify populated, empty, busy, and error states in both
  palettes, including long paths and a small window.

## Scope limits

Theme customization, new library views, mouse Back/Forward navigation,
store integration, and runner discovery keep
their own backlog entries. This plan does not change creation, launch, or
process ownership policy, add management operations, or redesign pages
beyond the existing library, review, and five requested management pages.

## Implementation and verification

- Implemented compact 160 x 240 posters and matching decode sizes; shared
  32 px rounded controls with hover, pressed, disabled, and focus treatments;
  docked panel depth and bounded slide/fade transitions; grouped frozen
  creation reviews with a fixed footer; and all five management pages.
- Added regression coverage for reversals, stale ticks, navigation, immediate
  close/log cleanup, selection without replay, and Checkup opening a closed
  panel. Accent label contrast covers both palettes and disabled states.
- Independent QA passed after correcting disabled accent label contrast.
  The full suite passed: 2,018 tests, 0 failures, 16 ignored. Formatting,
  Clippy with warnings denied, and the release GUI build passed.
- A hidden X11 workspace exercised the actual GUI in light and dark mode at
  1200 x 800 and 800 x 600, including poster density, panel actions, grouped
  management pages, empty/error states, and Windows/native review footers.
  The temporary fixture filesystem does not support reflinks, so real snapshot
  copying remains part of maintainer acceptance.
- Visual review removed duplicate Checkup padding and kept longer secondary
  panel labels at their natural width. The original HTML preview is unchanged
  and saved under `docs/design/` beside the reusable style guide.

## Fixes during acceptance

- Investigated the installed app's "invalid or unsupported trash manifest"
  error. Existing entries had matching device, inode, and creation time but
  different current Linux mount IDs. Trash identity checks now accept that
  change while retaining replacement detection and current mount-boundary
  checks. The same error appeared in the isolated GUI fixture and should have
  been investigated before implementation was handed over for acceptance.
- Five regressions cover changed mount IDs in listing, snapshot restore,
  prune, portable marker recovery, and copy-stage recovery, plus conservative
  refusal when creation time is absent. Independent QA and all 43 Trash tests
  passed. The full suite passed with 2,023 tests, 0 failures, and 16 ignored;
  formatting and Clippy with warnings denied passed.
- Release CLI and GUI builds passed. A read-only check with the new CLI listed
  all four existing entries from the affected installation and verified that
  their manifest bytes were unchanged.
- Implemented the approved acceptance refinements: desktop display titles and
  search, explicit game-tag launch labels, shared dropdown and popup styles,
  a bounded context menu, centered Checkup labels, separate primary/secondary
  action rows, and closing the panel from unused Library space.
- Independent QA caught and verified a fix for long busy reasons pushing the
  context menu outside a small window. The reason and actions now scroll while
  its title and Close control stay visible. All 506 GUI tests and the full suite
  passed: 2,028 tests, 0 failures, 16 ignored. Formatting, Clippy with warnings
  denied, and release CLI/GUI builds passed.
- A hidden desktop with throwaway registrations checked friendly card/panel
  titles, Play versus Launch, the labeled chooser and opened popup, compact
  context menus, Checkup label alignment, and blank-space closing at 1200 x 800
  and 800 x 600. User registrations were not changed.
- Implemented the approved playful grid motion with a continuous 300 ms
  spring, a small stagger, an 8 px overshoot limit, and a smooth final settle.
  Reversals preserve position and velocity. Measured slot layouts are cached,
  card hit areas follow the moving artwork, and overlapping cards receive input
  in paint order. The library keeps one scroll tree and an accessible rail
  beside the panel. Keyboard scrolling uses actual card bounds and checks again
  after settling; unchanged background refreshes leave active motion intact.
- Independent QA passed after fixes for scroll tree replacement and keyboard
  scrolling during motion. All 512 GUI tests and the full suite passed: 2,034
  tests, 0 failures, 16 ignored. Formatting, Clippy with warnings denied, and
  release CLI/GUI builds passed.
- Hidden window checks used 30 numbered poster fixtures in both palettes,
  including 1200 x 800 and 800 x 600 windows. Checked rapid reversals, moving-card
  selection, scrolled opening/closing, reachable bottom rows, scrollbar dragging,
  keyboard selection during motion, filtering, resizing, navigation, and menus.
  These checks used throwaway storage and private appearance settings.

## Maintainer acceptance

The maintainer confirmed acceptance passed on 2026-10-04, including the
approved refinements, the grid-motion amendment, and snapshots on a
reflink-capable filesystem. The plan is done and released as 0.23.0.

- [x] Browse a populated library in both palettes and a short window. Check
  poster size, title/state text, search, keyboard navigation, and scroll position.
- [x] Check a named default desktop target, no desktop title, and several
  targets. Search by title and environment name; changing the launch target
  should keep the card title stable.
- [x] Check Play for a `game` tag and Launch without it on Wine and native
  entries. Future built-in gaming creations should show the tag in review.
- [x] Exercise Play/Launch, Stop, Log, and secondary actions with mouse and Tab/Shift+Tab.
  Check inactive, running, busy, error, and disabled states.
- [x] Open dropdowns and both action menus, including long names and busy
  reasons in a small window. Check every action, full reason text, and Close.
- [x] Click unused Library space to close the panel without changing keyboard
  selection. Card selection, scrollbars, and menus keep their own behavior.
- [x] Rapidly open/close/switch cards, leave Library mid-transition, and open
  and close logs during a run. Confirm immediate cleanup and idle animation.
- [x] Open and close the panel in a populated, scrolled library. Check smooth
  playful card movement, rapid reversals, clicks on moving cards, bottom rows,
  keyboard navigation, and resizing or filtering during the transition.
- [x] Review Windows/native creation, existing prefixes, long paths, and missing
  prerequisites. Check complete details and reachable Back/Create controls.
- [x] Edit global/environment Settings, save/revert, and test a config conflict.
- [x] Create/restore/delete a snapshot on a reflink-capable filesystem. Expand
  config comparison and confirm reports, results, and controls remain reachable.
- [x] Inspect used/unused Downloads, install progress, Checkup fixes/results,
  and Trash restore/recovery/deletion confirmations. Check navigation during work.
