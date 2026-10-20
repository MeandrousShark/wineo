Status: approved

# 0055: Grid zoom and poster-only view

## Goal

Let the desktop app's library grid be as big or as small as the moment
wants: a zoom slider you can drag, the usual zoom keys, and Ctrl with the
mouse wheel, remembered across launches. Add a poster-only toggle that
hides titles until you point at a poster or select it, then shows the
title over the art. Cards move to their new places when the zoom, the
sort, or the filters change, instead of snapping.

This is part 2 of the library views work (see
[0054](0054-library-sorting-and-favorites.md)'s Series section and the
"GUI library views" row in [features.md](../features.md)). Parts 1 and 2
land before 1.0.

## Settled by the maintainer

From 0054's Series section (2026-10-17) and the maintainer's sketch for
this plan (2026-10-19):

- A draggable zoom slider. `Ctrl+=` and `Ctrl+-` change it, and so does
  Ctrl with the mouse wheel.
- The zoom is remembered across launches, relaunches, and reinstalls that
  keep the configs: it lives in `library.toml`'s `[view]` table.
- The grid animates smoothly between sizes and keeps the selected game in
  view.
- Poster-only is a toggle on the grid, not a separate view. Titles are
  hidden until the mouse is over a poster or the keyboard selects it, then
  shown over the poster, like Heroic's.
- Per-card motion: cards animate to their new places on a zoom, a
  re-sort, or a filter change (0054 snaps them).
- This amends `gui-style.md`'s "titles beneath the art" rule on purpose.
- Environments without art keep readable text tiles.

## Existing foundation

- **Geometry.** Cards are `CARD_WIDTH` 160 px (`main.rs:104`) with a
  240 px poster (`grid.rs:25`), 16 px gaps (`main.rs:105`), and 16 px grid
  padding (`grid_motion::PADDING`). The poster frame has 2 px padding and a
  2 px selection border (`grid.rs:217-227`), so the art's inner frame is
  156 x 236. Under it sit a two-line 14 px semibold title, a muted 12 px
  identifier when it differs, and a meta row with the state dot, state
  word, and playtime (`grid.rs:236-277`, heights in `grid.rs:39-67`).
- **Code that reads the card size.** Besides the constants: meta row
  wrapping (`grid.rs:58`), title measuring (`grid.rs:233`), the card's
  width and identifier truncation (`grid.rs:269, 272`), columns, slots,
  and poster hit bounds (`grid_motion.rs:159, 198, 296`), and art loading
  (`app_art.rs:66-104`). The import and Art page previews have their own
  fixed sizes and stay as they are.
- **Columns.** `floor((width - 2 * padding + gap) / (card width + gap))`,
  at least one (`grid_motion.rs:158-162`); `columns_for_viewport` takes
  the docked panel off first. The app keeps `viewport_width` from the
  scrollable's bounds and the window size. The grid's own layout reads
  its limits instead; whether a scrollbar makes the two differ is
  unverified and step 2 checks it.
- **Motion.** `GridMotion` springs each card between its closed and open
  slots when the panel docks (about 300 ms, up to 35 ms stagger), driven
  by a `window::frames()` subscription only while it runs. It only
  interpolates between those two slot sets, so a zoom, re-sort, or filter
  change snaps today, and so does a window resize (`app_window.rs:76-83`).
  Its widget matches children to cards by index (`grid_motion.rs:325-331`).
- **Art.** Grid thumbnails decode to a cover-fit target of 320 x 480,
  keeping the aspect ratio and never upscaling, so a wide source can come
  out wider than 320 (`artwork.rs:55-68, 347-360`). They're cached on disk
  by size in the file name and in memory keyed without size
  (`art_cache.rs`), under a 96 MiB soft budget that art in use may exceed
  (`art_cache.rs:135-151`). Loading (`app_art.rs:66-104`) covers the
  visible rows of both the docked and undocked layouts plus two rows of
  margin, and while the dock moves, every index between them. Two decodes
  run at once.
- **Poster layers.** Over the art: the GOG pill top-left (Update
  available, Finish update, Finish install; `grid.rs:182-204`), the
  favorite and hidden marks top-right, the Steam mark bottom-right
  (`grid.rs:159-180`), and Steam's install bar along the bottom edge
  (`grid.rs:209-215`).
- **Hover and selection.** Hovering an ordinary card dims its poster to
  85%; hidden and uninstalled cards keep their stronger dimming
  (`grid.rs:119-151`). The selected card has a 2 px primary border; arrows
  move by index or by `columns` (`keys.rs:282-322`), and the grid scrolls
  the selected card just into view (`app_grid.rs:12-40`).
- **Art-less cards** show the title centered on a muted text tile, and the
  same title again under it.
- **Wheel.** The grid's scrollable takes the wheel through `wheel::scale`
  (0.7 of iced's 60 px per line). Nothing reads Ctrl on the wheel today,
  so **Ctrl+wheel scrolls the grid now**, and this plan changes that.
- **Shortcuts.** `Ctrl+=`, `Ctrl++`, `Ctrl+-`, and `Ctrl+0` are free.
  Taken: Ctrl+F, `/`, `F`, Ctrl+1 to 5, Ctrl+`,`, Ctrl+Z, Ctrl+N.
- **Toolbar.** The "wineo" wordmark, then a fixed 320 px search box,
  **Sort** with the direction button, **Filters**, and **Add**
  (`grid.rs:895-1000`). Nothing shrinks and the window has no minimum
  size, so a narrow window clips the right end. Tab goes from **Filters**
  into the filter tab's chips (`filter_tab.rs:490-539`).
- **iced's slider** can't take focus: it steps on the arrow keys only
  while the pointer is over it, and steps on Ctrl+wheel
  (`iced_widget-0.14.2/src/slider.rs:375-407`).
- **`library.toml`.** `[view]` holds `sort`, `descending`, and
  `favorites_first` (`library.rs:62-66`), written one field at a time
  under the lock, with bool and sort setters. `validate` flags `[view]`
  keys it doesn't know (`library/check.rs:89-115`), and repair keeps only
  `[view]` values it recognizes as valid (`library/repair.rs:737-742`), so
  new keys join both lists.
- **Benchmarks.** `scripts/bench-gui-idle.sh` measures idle CPU, RSS, and
  scroll smoothness in Xvfb, counting sampled frames whose image changed
  (`bench-gui-idle.sh:210-224, 265-275`), and `library/bench.rs` times
  the rescan and order rebuilds. The `0054-step0` worktree keeps the
  generated 300-environment library; its build is 0.35.1.

## Answered by the maintainer (2026-10-19)

1. **Range: A.** 60% to 200% (posters 96 to 320 px wide), inside today's
   320 x 480 decode cap, so the art pipeline stays as it is. `Ctrl+=`,
   `Ctrl+-`, and each wheel notch step by 10 points; `Ctrl+0` goes back
   to 100%. The UI never shows a percentage, and the slider snaps to
   100% when dragged near it.
2. **Slider: A, for now.** In the toolbar after **Filters**, a 96 px
   slider with a small poster-only toggle beside it. **Add** moves to the
   toolbar's far left, before search, so it stands apart from the
   display controls. The maintainer is open to a better home for either
   later.
3. **Overlay: A.** The title only, up to two lines, on a dark gradient at
   the poster's bottom. No playtime: it gets busy when the mouse sweeps
   across the grid, and the panel has it one click away.
4. **Running games: A.** A small status chip in the poster's top-left
   corner for any state but idle, mirroring the marks on the right.
5. **Keys: A.** `zoom = 125` (whole percent) and `poster_only = true`.
   Both write nothing at their defaults (100, `false`). An older wineo's
   `validate` reports them as unknown keys, a warning only.
6. **Filter changes: A.** Cards that stay spring to their new places;
   new ones fade in where they land and leaving ones fade out, over about
   150 ms.

From the plan review, the same day:

7. **Status chip and GOG pill: A.** One row: the chip sits in the corner
   and the pill moves to its right, 4 px apart.
8. **Narrow window: A.** Search shrinks first, from 320 px down to 160 px,
   then the right end clips as today.

From the step 2 screenshots, the same day:

9. **Text under small posters: A.** Below 90% the identifier and the
   state and playtime row drop out, leaving the title, and a running
   game gets the status chip from poster-only (answer 4) instead. At 80%
   (128 px cards) a playtime like "250 h 57 m" already wraps, while 90%
   (144 px) fits it. Every card switches together, so rows keep one
   shape.

The maintainer approved the plan on 2026-10-19.

## Proposed design

### Zoom

- A zoom of Z percent scales the card and poster: card width
  `160 * Z / 100`, poster height 1.5 times that, rounded to whole pixels.
  Gaps and the grid's padding stay 16 px. Text sizes stay as they are,
  except the text tile's title, which scales with the poster between 12
  and 18 px so it stays readable at both ends. Below 90% the text under
  the poster is the title alone, with the status chip on the poster
  (answer 9). Poster marks, the GOG pill,
  and the Steam mark keep their sizes.
- One `Layout` value carries the zoomed geometry and replaces the
  constants everywhere the foundation lists. At 100% everything matches
  0.36.0 to the pixel, and tests pin it. The decode target, its disk
  file names, and the cache keys don't change.
- **Toolbar.** It reads the wordmark, **Add**, search, **Sort**, the
  direction button, **Filters**, the slider, and the poster-only toggle.
  Tab follows that order, with the filter tab's chips right after
  **Filters** as today, so they stay next to the button they hang from.
  The slider has no label or readout; its tooltip says "Zoom", and the
  toggle's says "Show titles" or "Hide titles". Both are 32 px high like
  the rest, and the slider shows a small tick at 100%.
- **The slider** is wineo's own widget, since iced's can't take focus. It
  takes focus with Tab, shows the usual focus ring, and steps on the left
  and right arrows like `Ctrl+-` and `Ctrl+=`. Ctrl+wheel over it zooms
  like Ctrl+wheel over the grid; the plain wheel over it does nothing.
- The slider changes the zoom as it moves; the grid follows without a
  tween, and a drag takes over at once from a tween in progress. It snaps
  to 100% once within 5 points of it and lets go past 7, so it doesn't
  flicker at the edge. Key steps and wheel notches go to the next
  multiple of 10 in their direction (a dragged 87% steps up to 90%), so
  they always pass through 100%. Repeated steps count from the zoom
  they're heading to, not from the tween's current frame, and tween there
  over about 160 ms.
- Trackpad wheels (pixel deltas) add up until they make a step. The sum
  resets when Ctrl is released, the direction reverses, or the pointer
  leaves the grid. A zoom gesture at either end of the range is still
  taken, so it never scrolls instead.
- `Ctrl+=` and `Ctrl++` (with or without Shift, and the keypad's `+`) zoom
  in, `Ctrl+-` zooms out, `Ctrl+0` resets. They work while the grid, the
  search box, or a toolbar control has focus, leave that focus where it
  is, and do nothing while a menu, popup, or dialog takes the keys.
- Ctrl+wheel zooms over the grid and the slider; over the panel, a menu,
  or the filter tab it does what the wheel does there without Ctrl.
- **Keeping the place.** At the start of a gesture (a drag, or a run of
  steps), the anchor is the selected card if any of it is on screen,
  otherwise the first card that intersects the viewport. The anchor's name
  and its top's offset in the viewport are stored once for the gesture,
  and every frame puts its top back at that offset, clamped to the scroll
  range; a card taller than the viewport aligns its top. A manual scroll,
  arrow key, or click ends the gesture's anchoring, and replies about an
  older layout are ignored. If the selected card was off screen, it
  scrolls into view at the end like an arrow key would.
- **Saving.** The saved value is the integer the zoom is heading to, never
  a tween frame. It saves like the sort: off the UI thread, one field at a
  time under the lock, the last value wins, and a failed save shows the
  sort's notice. A drag saves once when it ends; a drag still unsaved when
  the window closes is saved by the close wait. Like the other `[view]`
  keys, a key that's absent isn't written while it holds its default, and
  one already in the file stays and is updated. A window keeps its own
  zoom and poster-only state while it's open, as it does the sort.

### Motion

- **One owner.** `GridMotion` becomes the only thing that moves cards. For
  each card, by name, it keeps the displayed position, size, opacity, and
  velocity. A layout change of any kind sets new targets and the springs
  carry each card from where it's drawn now, so a column change halfway
  through a zoom tween, or a re-sort during a dock, never jumps. A slider
  drag sets sizes directly; steps tween them.
- **What moves.** The dock, a zoom that changes the column count, the
  poster-only toggle (row heights change; the anchor holds the place), a
  re-sort, a favorite that moves a card while favorites come first, and a
  search, filter, or Show hidden change. Within a column count, a zoom
  just scales: every slot is a function of the zoom, so there's nothing to
  spring.
- **What snaps.** A window resize (every frame of a drag would restart the
  springs) and the 2 s rescan's changes, as today. A rescan that lands
  mid-transition settles it first.
- **Cards that leave** stay in a short-lived outgoing set: drawn where
  they were, fading out over about 150 ms, taking no input. A card that
  comes back mid-fade fades in again from where it is. The grid keeps its
  scroll height until the fades end, so the scrollbar doesn't jump. The
  widget matches children to cards by name instead of index, so a card's
  hover and state follow it. When a filter leaves nothing, the empty
  message shows once the last card has faded.
- Search and filter changes keep today's scroll behavior; selection that
  a filter hides behaves as today.
- Animation runs only while something moves; idle CPU stays where 0.36.0
  has it. A reduced-motion setting belongs to the "GUI accessibility
  settings" row, not this plan.

### Art loading

- Art loading reads the same `Layout` as the grid. It loads what's
  visible first, then where moving cards are headed, then cards fading
  out that are still on screen. The current sweep of every index between
  two layouts only fits the dock; for a re-sort it could cover the whole
  library, so it's replaced by these sets.
- At 60%, and more so in poster-only, many more posters fit on screen.
  Step 6 checks that the memory cache's soft budget holds there.

### Poster-only

- The toolbar toggle. With it on, a card is just its poster with its
  layers: the marks top-right, the Steam mark bottom-right, the install
  bar along the bottom, and top-left the status chip for any state but
  idle (the state dot on the marks' backing, 24 px like them), with the
  GOG pill to its right (answer 7).
- Hovering a card, or selecting it, shows its title over the bottom of the
  poster on a dark gradient, in white semibold, up to two lines. It's
  drawn inside the poster's clipped frame and under the selection border,
  and it's not a pointer target. It sits above the install bar and leaves
  room on the right for the Steam mark. The identifier isn't shown; the
  panel has it.
- The gradient is dark in both themes, so white text works in both; a
  hidden card shown with Show hidden keeps its 45% art and a muted title,
  and step 5 checks that title's contrast in both themes.
- Only cards with no art at all show their title on the tile, and they
  show no overlay. A card whose art is still loading or failed to decode
  follows the art cards' rules, so its title doesn't vanish when the art
  arrives.
- Rows are shorter without the text, so more fit, and keyboard Up and
  Down still move by columns.
- Below 90% with titles shown (answer 9), cards keep the title under the
  poster and drop the identifier and the state row; the status chip and
  the GOG pill sit on the poster exactly as in poster-only.
- `gui-style.md` gets the amendment: titles beneath the art by default,
  over it in poster-only.

### Saved view

- `[view]` gains `zoom` (whole percent, 60 to 200, default 100) and
  `poster_only` (default `false`), read leniently like the other keys: a
  value out of range or of the wrong type counts as unset. Core gets an
  integer setter beside the bool and sort ones. `validate`'s list of
  `[view]` keys and repair's list of valid `[view]` values each grow by
  two, so repairing an unrelated broken line keeps a saved zoom. Edits keep
  the file's comments, like every other `library.toml` write.

## Implementation sequence

0. **Benchmark baseline.** Build 0.36.0 (the `v0.36.0` tag) in the
   `0054-step0` worktree, which keeps the generated library. Add zoom runs
   to `scripts/bench-gui-idle.sh` (see Testing) and record 0.36.0's scroll,
   idle, and dock numbers at 100%.
1. **Core `[view]` keys.** `zoom` and `poster_only` in `View`, lenient
   reads, the integer setter, the `validate` and repair lists, and
   `config-files.md`.
2. **Geometry as a function of zoom.** The `Layout` value through every
   place the foundation lists, identical at 100% (tests pin it). Confirms
   whether the scrollbar width makes `viewport_width` and the layout width
   disagree.
3. **Zoom controls.** **Add** moved left, the search box that shrinks
   first (answer 8), the focusable slider and the toggle, the keys,
   Ctrl+wheel, tweened steps, the anchor, and the save.
4. **Motion.** `GridMotion` as the one owner, name-keyed children, the
   outgoing set and fades, every trigger above, the dock on the same path,
   and art loading on the shared layout.
5. **Poster-only.** The overlay, the status chip with the GOG pill, the
   compact text below 90% (answer 9), the art-less and loading cases,
   hidden cards, and the style guide amendment.
6. **Benchmark again** against step 0.
7. **Docs.** `gui.md` (zoom, keys, poster-only, Tab order),
   `gui-style.md`, `config-files.md`, and `features.md`.

## Testing and acceptance

- Layout probe tests: geometry at 60, 100, and 200%, with 100% matching
  0.36.0's numbers; column counts with and without the panel; the anchor
  staying put through a zoom, at the scroll limits, and with an oversized
  card; overlay shown on hover and selection only; the chip and the GOG
  pill together; card heights switching at 90% with titles shown.
- Key tests for every zoom shortcut, including Shift and keypad forms, for
  focus staying put, for the slider's arrows and Tab order, and for keys
  that must not zoom while a menu or dialog takes them.
- Ctrl+wheel over the grid and slider zooms and over the panel or filter
  tab doesn't; a wheel without Ctrl still scrolls everywhere; a gesture at
  the range's ends doesn't scroll; pixel deltas add up and reset.
- Snap tests: entering at 5 points, leaving past 7, steps from a dragged
  value landing on multiples of 10.
- Motion tests drive the frame clock: transitions settle at their slots,
  retarget mid-flight from displayed positions, fade cards out and back
  in, hold the scroll height through fades, and stop the frame
  subscription when done.
- Core tests for `[view]` reads, writes, no write while absent at the default, comment
  preservation, `validate`, and repair keeping valid values.
- Benchmarks in Xvfb with tiny-skia: idle CPU and RSS after motion stops;
  frame gaps (p90 and max) during single steps, held-down steps, slider
  drags across column changes, sort and filter fades, and the dock; cold
  and warm art at 60, 100, and 200%, with titles and poster-only. Xvfb
  measures CPU rendering, so acceptance also checks the feel on the
  maintainer's GPU.
- The acceptance checklist is written once the work lands.

## Scope limits

- No list view, presets, grouping, or coverflow: parts 3 to 5.
- No zoom past 200% and no bigger art decodes (answer 1).
- No percentage readout anywhere (answer 1).
- No reduced-motion setting: the "GUI accessibility settings" row.
- No change to the art-less tile's repeated title in the normal view.
- No zoom in other pages' grids (Art, imports); only the Library.
- No animated window resizes.

## Decisions during implementation

- **Defaults aren't removed** (2026-10-19, step 1). The draft said setting
  a key back to its default removes it; the existing `[view]` setters
  instead skip the write while the key is absent and keep a written key,
  and answer 5 only asks for nothing written at the defaults. The new keys
  follow the existing ones.

## Results

### Step 0 baseline (0.36.0, 2026-10-19)

`scripts/bench-gui-idle.sh` (e2835caa) gained dock, resort, and zoom runs:
each is a timed list of actions captured at 60 fps, reporting CPU, changed
frames per second of motion, frame gaps, and `active_ms`, the average time
from an action to its last changed frame. The baseline ran the `v0.36.0`
tag's `release` build in the `0054-step0` worktree, on its generated
300-environment library, in Xvfb with tiny-skia, 1200 x 800, 5 fresh
launches, with `ZOOM=0` since 0.36.0 has no zoom. Ryzen 7 5800X.

| Measurement | min | median | p90 | max |
|---|---:|---:|---:|---:|
| Idle CPU, % of one core | 1.75 | 1.80 | 1.85 | 1.85 |
| RSS, MiB | 91 | 92 | 93 | 93 |
| Scroll CPU, % | 59.7 | 60.7 | 62.0 | 62.0 |
| Scroll frames/s | 8.8 | 10.3 | 10.6 | 10.6 |
| Scroll gap p90 / max, ms | 133 / 133 | 150 / 167 | 150 / 167 | 150 / 167 |
| Dock CPU, % | 38.3 | 38.8 | 39.8 | 39.8 |
| Dock frames/s | 18.6 | 18.9 | 20.7 | 20.7 |
| Dock gap p90 / max, ms | 83 / 89 | 83 / 100 | 83 / 100 | 83 / 100 |
| Dock active, ms | 530 | 539 | 555 | 555 |
| Resort CPU, % | 16.0 | 16.6 | 17.1 | 17.1 |
| Resort frames/s | 8.4 | 8.9 | 9.0 | 9.0 |
| Resort gap p90 / max, ms | 133 / 133 | 133 / 133 | 133 / 181 | 133 / 181 |
| Resort active, ms | 209 | 223 | 226 | 226 |

Main-thread wakeups stayed at 0 while idle, with 21 threads. The dock is
the only run with motion today: its spring draws at about 19 frames/s on
the CPU renderer and settles in about 540 ms. A re-sort snaps, and that
one re-laid-out frame takes about 130 ms to draw, so the resort numbers
are the cost of a full-grid repaint, which step 4's springs will repeat
every frame. As in 0054, these are relative numbers for comparing steps
in Xvfb, not what a GPU feels.

The runs park the mouse at fixed points: card 1 at (236, 190), the
direction button at (697, 24), and an empty header spot at (900, 24) for
zoom. Step 3 moves **Add** to the left and adds the slider, which shifts
the toolbar, so step 3 updates those coordinates before step 6 compares.
