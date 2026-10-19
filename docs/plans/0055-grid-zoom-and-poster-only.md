Status: draft

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
- Whether the overlay shows playtime is open (question 3): it may be too
  busy.

## Existing foundation

- **Geometry.** Cards are `CARD_WIDTH` 160 px (`main.rs:104`) with a
  240 px poster (`grid.rs:25`), 16 px gaps (`main.rs:105`), and 16 px grid
  padding (`grid_motion::PADDING`). The poster frame has 2 px padding and a
  2 px selection border (`grid.rs:217-227`), so the art's inner frame is
  156 x 236. Under it sit a two-line 14 px semibold title, a muted 12 px
  identifier when it differs, and a meta row with the state dot, state
  word, and playtime (`grid.rs:236-277`, heights in `grid.rs:39-67`).
- **Columns.** `grid_motion::columns` divides the viewport width by card
  width plus gap, and `columns_for_viewport` takes the docked panel off
  first (`grid_motion.rs:158-166`). The app keeps `viewport_width` from
  the scrollable's bounds and the window size. The grid's own layout reads
  its limits instead; whether a scrollbar makes the two differ is
  unverified and step 2 checks it.
- **Motion.** `GridMotion` springs each card between its closed and open
  slots when the panel docks (about 300 ms, up to 35 ms stagger), driven
  by a `window::frames()` subscription only while it runs. It only
  interpolates between those two slot sets, so a zoom, re-sort, or filter
  change snaps today. Other code that hardcodes the 160 x 240 poster (hit
  tests and per-card motion) has to follow the zoom too.
- **Art.** Grid thumbnails decode to at most 320 x 480, cover fit, and
  never upscale (`artwork.rs:55-68, 347-360`), cached on disk by size in
  the file name and in a 96 MiB memory cache keyed without size
  (`art_cache.rs`). Two decodes run at once, for the visible rows plus two
  rows of margin.
- **Hover and selection.** Hovering a card dims its poster to 85%
  (`grid.rs:130-151`). The selected card has a 2 px primary border; arrows
  move by index or by `columns`, and the grid scrolls the selected card
  just into view (`keys.rs:282-322`).
- **Art-less cards** show the title centered on a muted text tile, and the
  same title again under it.
- **Wheel.** The grid's scrollable takes the wheel through `wheel::scale`
  (0.7 of iced's 60 px per line). Nothing reads Ctrl on the wheel today,
  so **Ctrl+wheel scrolls the grid now**, and this plan changes that.
- **Shortcuts.** `Ctrl+=`, `Ctrl++`, `Ctrl+-`, and `Ctrl+0` are free.
  Taken: Ctrl+F, `/`, `F`, Ctrl+1 to 5, Ctrl+`,`, Ctrl+Z, Ctrl+N.
- **`library.toml`.** `[view]` holds `sort`, `descending`, and
  `favorites_first` (`library.rs:62-66`), written one field at a time
  under the lock, with bool and sort setters. `validate` flags `[view]`
  keys it doesn't know (`library/check.rs:89-115`), so new keys join that
  list.
- **Benchmarks.** `scripts/bench-gui-idle.sh` measures idle CPU, RSS, and
  scroll smoothness in Xvfb, and `library/bench.rs` times the rescan and
  order rebuilds. The `0054-step0` worktree keeps a warm build and the
  generated 300-environment library for them.

## Questions for the maintainer

### 1. Zoom range and steps

The art's inner frame at 200% is 312 x 472, still inside the 320 x 480
decode cap, so a range that stops at 200% needs no art changes. Past it,
posters would upscale unless the cap grows, and a 640 x 960 cap means new
thumbnail files and a memory cache that holds about 40 posters instead of
about 160.

- A: 60% to 200% (posters 96 to 320 px wide). The slider moves freely;
  `Ctrl+=`, `Ctrl+-`, and each wheel notch step by 10 points; `Ctrl+0`
  goes back to 100%.
- B: 60% to 300%, with a bigger decode cap above 200%.
- C: a few fixed sizes (like 75, 100, 125, 150, 200%) that the slider
  snaps to.

Recommendation: **A**. 200% already fits about three posters across a
1080p window beside the panel, and it keeps the art pipeline as it is.

### 2. Where the slider lives

The toolbar today: search (it stretches), **Sort**, the direction arrow,
**Filters**, and **Add**.

- A: in the toolbar after **Filters**, a 96 px slider with a small
  poster-only toggle button beside it. Always one drag away, but it costs
  about 150 px, so the search box narrows in a small window.
- B: a **View** button after **Filters**, opening a popup like Sort's
  with the slider and a **Poster only** check row. The toolbar barely
  grows, and part 3's list view switch can join the same popup, but the
  slider is one click further away.
- C: a slim footer under the grid with the slider and toggle at the
  right, like a file manager's status bar. Leaves the toolbar alone, but
  takes about 36 px of height everywhere.

Recommendation: **A**, since dragging while watching the grid change is
the point of a slider; Ctrl+wheel covers small windows.

### 3. What the overlay shows in poster-only mode

- A: the title only, up to two lines, on a dark gradient at the poster's
  bottom.
- B: the title plus one muted line with the state and playtime, like the
  meta row under a poster today.
- C: the title, plus the playtime only on the selected card.

Recommendation: **A**. The panel has the playtime one click away, and the
overlay stays quiet when the mouse sweeps across the grid.

### 4. A running game in poster-only mode

Today the state dot and word sit under the poster. With titles hidden, a
running, starting, or updating game has nothing to say so until you point
at it.

- A: a small status chip in the poster's top-left corner, always shown
  for any state but idle (the state dot, like the poster marks on the
  right), and nothing for an idle game.
- B: the state shows only inside the overlay, on hover or selection.

Recommendation: **A**, so a game left running is visible at a glance.

### 5. The `[view]` keys

```toml
# A
[view]
zoom = 125            # percent
poster_only = true

# B
[view]
zoom = 1.25           # scale
titles = "over"       # "below" or "over"
```

Recommendation: **A**. Whole percents never round-trip oddly, and the key
reads like the control. Both default to off (100, `false`) and write
nothing at their defaults. An older wineo's `validate` reports these as
unknown keys, a warning only, since `library.toml` is lenient.

### 6. Cards that come and go on a filter change

- A: cards that stay spring to their new places; new ones fade in where
  they land and leaving ones fade out, over about 150 ms.
- B: cards that stay spring; the rest appear and vanish at once.

Recommendation: **A**.

## Proposed design

Details here assume the recommendations; the answers amend them.

### Zoom

- A zoom of Z percent scales the card and poster: card width
  `160 * Z / 100`, poster height 1.5 times that, rounded to whole pixels.
  Gaps and the grid's padding stay 16 px. Text sizes stay as they are,
  except the text tile's title, which scales with the poster between 12
  and 18 px so it stays readable at both ends. Poster marks keep their
  24 px size.
- One `Layout` value carries the zoomed geometry and replaces the
  constants wherever they're read: card size, columns, slots, card
  height, hit tests, art loading margins, and per-card motion. At 100%
  everything matches 0.36.0 to the pixel, and tests pin it.
- The slider changes the zoom as it moves; the grid follows without a
  tween. Keys and wheel notches tween the zoom over about 160 ms, and
  steps taken during a tween retarget it. Trackpad wheels (pixel deltas)
  add up until they make a step.
- `Ctrl+=` and `Ctrl++` (with or without Shift, and the keypad's `+`) zoom
  in, `Ctrl+-` zooms out, `Ctrl+0` resets. They work while the grid or
  the search box has focus, and not while a menu, popup, or dialog takes
  the keys.
- Ctrl+wheel zooms only over the grid; over the panel, a menu, or the
  filter tab it does what the wheel does there without Ctrl.
- **Keeping the place.** The anchor is the selected card if it's on
  screen, otherwise the first card whose top is in view. Through the zoom
  the anchor keeps its top at the same height in the viewport; if the
  selected card was off screen, the selection still scrolls into view at
  the end like an arrow key would.
- **Saving.** The zoom saves like the sort: off the UI thread, the last
  value wins, and the close wait covers it. A drag saves once, after it
  ends. A window keeps its own zoom while it's open, as it does the sort.

### Motion

- `GridMotion` generalizes from "closed to open" into "from these slots to
  those", per card by name. The dock, a zoom that changes the column
  count, a re-sort, and a filter or search change all start a transition
  from where each card is now to its new slot, with the existing spring,
  stagger, and settle. A transition that starts during another continues
  from the current positions.
- Within a column count, a zoom just scales: every card's slot is a
  function of the zoom, so there's nothing to spring. Only the cards that
  change row or column when the count changes spring.
- Cards entering on a filter change fade in at their slot; cards leaving
  fade out where they were and take no input. The 2 s rescan's changes
  (a game added or removed outside the app) snap as today.
- Animation runs only while something moves; idle CPU stays where 0.36.0
  has it. A reduced-motion setting belongs to the "GUI accessibility
  settings" row, not this plan.

### Poster-only

- A toggle (question 2 places it). With it on, a card is just its poster,
  plus marks: the favorite and hidden marks on the right, and the status
  chip on the left for any state but idle (question 4).
- Hovering a card, or selecting it, shows its title over the bottom of
  the poster on a dark gradient, in white semibold, up to two lines. The
  identifier isn't shown; the panel has it.
- Art-less cards already show their title on the tile, so they show no
  overlay; their tile keeps working as the label.
- A hidden card shown with Show hidden keeps its 45% art; its overlay
  title is muted like its title under the poster.
- Rows are shorter without the text, so more fit, and keyboard Up and
  Down still move by columns.
- `gui-style.md` gets the amendment: titles beneath the art by default,
  over it in poster-only.

### Saved view

- `[view]` gains `zoom` (whole percent, 60 to 200, default 100) and
  `poster_only` (default `false`), read leniently like the other keys: a
  value out of range or of the wrong type counts as unset. Core gets an
  integer setter beside the bool and sort ones, and `validate`'s list of
  `[view]` keys grows by two.

## Implementation sequence

0. **Benchmark baseline.** Add a zoom measurement to
   `scripts/bench-gui-idle.sh`: `Ctrl+=` and `Ctrl+-` presses captured
   like the scroll run, reporting presented frames and the longest gap.
   Record 0.36.0's scroll and idle numbers at 100% as the baseline, in
   the `0054-step0` worktree.
1. **Core `[view]` keys.** `zoom` and `poster_only` in `View`, lenient
   reads, the integer setter, the `validate` key list, and
   `config-files.md`.
2. **Geometry as a function of zoom.** The `Layout` value, columns, slots,
   heights, hit tests, art margins, and every hardcoded poster size;
   identical at 100% (tests pin it). Confirms whether the scrollbar width
   makes `viewport_width` and the layout width disagree.
3. **Zoom controls.** The slider and toggle where question 2 puts them,
   the keys, Ctrl+wheel, tweened steps, the anchor, and the save.
4. **Motion.** Generalized slot-to-slot transitions for zoom, re-sort,
   filter, and search changes, fades for cards coming and going, and the
   dock moved onto the same path.
5. **Poster-only.** The overlay, the status chip, art-less tiles, hidden
   cards, and the style guide amendment.
6. **Benchmark again** against step 0, at 100% and at both ends of the
   range.
7. **Docs.** `gui.md` (zoom, keys, poster-only), `gui-style.md`,
   `config-files.md`, and `features.md`.

## Testing and acceptance

- Layout probe tests: geometry at 60, 100, and 200%, with 100% matching
  0.36.0's numbers; column counts with and without the panel; the anchor
  staying put through a zoom; overlay shown on hover and selection only.
- Key tests for every zoom shortcut, including Shift and keypad forms,
  and for keys that must not zoom while a menu or dialog takes them.
- Ctrl+wheel over the grid zooms and over the panel or filter tab
  doesn't; a wheel without Ctrl still scrolls everywhere.
- Motion tests drive the frame clock: transitions settle at their slots,
  retarget mid-flight, and stop the frame subscription when done.
- Core tests for `[view]` reads, writes, no-op writes at defaults, and
  `validate`.
- The acceptance checklist is written once the work lands.

## Scope limits

- No list view, presets, grouping, or coverflow: parts 3 to 5.
- No zoom past 200% and no bigger art decodes (question 1).
- No reduced-motion setting: the "GUI accessibility settings" row.
- No change to the art-less tile's repeated title in the normal view.
- No zoom in other pages' grids (Art, imports); only the Library.
