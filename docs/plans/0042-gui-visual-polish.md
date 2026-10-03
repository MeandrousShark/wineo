Status: draft

# 0042: GUI visual polish

## Goal

Make the existing desktop app feel finished: give the detail actions a
consistent shape and clear interaction states, add depth and motion to the
detail panel, and make the new-environment review easy to scan.

## Settled by the maintainer

- On 2026-10-02, the maintainer picked UI polish as the next plan and asked
  to investigate the flaky crashed Proton installer test first.
- The maintainer picked compact rounded rectangles for the visual previews
  and requested interactive HTML mockups before implementing in Wineo.
- Panel motion and review layout await visual feedback; the plan is not
  approved for implementation yet.

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

## Choices to settle

### 1. Detail action shape

Both options retain visible text labels and the current action availability.
The examples use the same inactive and running environment.

| Scenario | A: compact rounded rectangles (recommended) | B: pill buttons |
|---|---|---|
| Inactive: Play, Stop, Log | Shared height, 8 px corners, and padding; Play uses the primary fill, disabled Stop is muted, Log is a neutral surface | The same roles and height, with fully rounded ends |
| Running: Play, Stop, Log | Disabled Play is muted, Stop uses the danger fill, Log keeps the same neutral surface | The same running states in the pill shape |
| Hover, press, keyboard focus | Explicit hover/pressed colors and a visible focus treatment within the same shape | The same feedback within the pill shape |

Selected for previews: A, using the existing palette. Carry the shared geometry to
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

Recommendation: A. Keep layout changes bounded: reserve the docked width
during a transition and reflow the library once per open/close rather than
on every animation frame. Tick only while motion is active. Logical close,
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

Recommendation: A. Use spacing, labels, and muted secondary text for
hierarchy. Keep warning and prerequisite controls prominent. Rearrange the
existing review data without recomputing creation decisions in the view.

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
5. **Review, docs, and acceptance.** Independently review the interactions,
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

## Scope limits

Theme customization, new library views, library names from desktop entries,
mouse Back/Forward navigation, store integration, and runner discovery keep
their own backlog entries. This plan does not change creation, launch, or
process ownership policy, or redesign every management page.
