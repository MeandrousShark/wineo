Status: draft

# 0056: GUI polish

## Goal

Three small touches to the desktop app's library that were too small for a
plan each: star a game from its poster on hover, slide a card's details in
and out when the zoom crosses 90%, and make the zoom slider's 100% snap
easier to hit. More touches can join before approval.

This is queue item 1 in [features.md](../features.md), after
[0055](0055-grid-zoom-and-poster-only.md).

## Settled by the maintainer

- Hovering a poster shows a clickable star, saving the trip through the
  panel or the right-click menu (2026-10-18). It sits where the favorite
  mark already is. F, the panel star, and the menu stay for keyboards,
  touch, and controllers.
- The environment name, status, and playtime slide into view when the zoom
  reaches 90% and out when it drops below, instead of appearing at once
  (2026-10-22, 0055 acceptance).
- The slider's 100% snap is hard to hit; widening it is a trivial patch
  (0055 answer 15, 2026-10-22).

Answers to the first five questions (2026-10-22), numbered as asked:

1. **A favorite's star toggles.** A favorite's filled star is always
   shown, and clicking it unfavorites, exactly as the hover star favorites.
2. **Reserve the star's room on every card.** The corner keeps the 36 px
   the marks ask for whether or not a mark shows, so nothing shifts on
   hover. A narrow unmarked card's GOG pill turns into the round badge a
   little earlier than today.
3. **No tooltip on the star.** The maintainer thinks it's obvious. It
   wouldn't help a screen reader, since iced exposes no accessibility
   tree; the "GUI accessibility settings" row is where that belongs.
4. **Fade and slide.** The details fade as they do now and drift up about
   8 px into place (down and out when they leave) on the zoom's 160 ms
   tween. Heights keep springing as today, and a throwaway spike checks
   the text clips cleanly while they do.
5. **Widen the snap to 8 points in and 11 out** in this plan.

## Existing foundation

Mapped on 2026-10-22; line numbers are in `crates/wineo-gui/src`.

- **Marks.** `poster_marks` (`library_actions.rs:416-436`) stacks a star for
  a favorite and a crossed-out eye for a hidden game in the top-right
  corner, 24 px chips on a 55% black backing. It returns nothing unless the
  card is a favorite or hidden, and the corner room the GOG pill leaves
  (`MARKS_ROOM`, `grid.rs:302-306`) keys off the same test. Marks are a
  plain container, so they aren't clickable today.
- **Hover.** There's no per-card hover widget. Each card is one
  `mouse_area` (`grid.rs:1241-1251`) with press, double-click, right-press,
  and enter and exit handlers, and the app keeps a single
  `hovered: Option<String>` (`main.rs:168`). Hover is ignored while a flow
  or dialog is open (`main.rs:812`). The poster-only title overlay already
  uses `hovered || selected` (`grid.rs:213-214`).
- **Clicks.** Today a click on the marks selects the card, since they sit
  inside the card's mouse area. A button in the marks layer takes a left
  press first: iced's `button` captures it, and `mouse_area` returns
  before its own press or double-click fires. A right press isn't
  captured, so the context menu still opens.
- **Favorite toggling.** The F key, the panel star, and the right-click
  menu all send `Message::Mark(Event::Toggle(name, Flag::Favorite))`. The
  handler flips the card at once, writes `library.toml`, and re-sorts when
  the order changes (`library_actions.rs:194-385`). A poster star reuses
  it.
- **Details.** `COMPACT_BELOW = 90` (`grid_motion.rs:41-43`). `CardText`
  is `PosterOnly`, `Title` below 90%, or `Full`, decided from the zoom the
  tween is heading to, while geometry follows the tween's frame
  (`main.rs:1368-1376`). Crossing the threshold changes the card's height,
  so `zoom_to` springs the cards instead of scaling them
  (`grid_motion.rs:525-538`). The title, identifier, and meta row already
  fade with `caption_alpha` (`grid.rs:195-196`, `411-453`), but no per-card
  scalar for size or height exists. Position springs, fade opacity, and
  the collapse easter egg's sweep are the only per-card motion.
- **Snap.** `SNAP_IN` 5 and `SNAP_OUT` 7 (`zoom.rs:18-21`), tested in
  `zoom/tests.rs:256-293` and quoted in `gui-style.md:108-109`.

## Open questions

6. **Dead space on the right when the width doesn't fit another column.**
   Cards sit at a fixed width for the zoom, left-aligned from the 16 px
   padding, so the leftover is always on the right: up to about one card
   wide. At 200% in a 1000 px area, 2 columns use 688 px and leave 312 px
   empty on the right, against 16 px on the left.
   ```
   today (200%):   |[card][card]                  |
   A. center:      |      [card][card]      |       (156 px each side)
   B. stretch:     |[ card    ][ card    ]|         (cards grow to fill)
   C. leave it
   ```
   - A. **Center the grid (recommended).** The leftover splits evenly, so
     the margins match. Cards keep their exact zoom sizes. Cost: cards
     slide sideways while the window resizes and when the panel docks,
     even when the column count stays the same; the motion owner already
     springs a dock, and a resize snaps. Tests that pin slot x positions
     change.
   - B. Stretch the cards a little so each row fills. The slider sets a
     minimum card width, so 100% would no longer mean exactly 160 px, and
     poster sizes would change as the window resizes. This breaks the
     "identical at 100%" rule that 0055's tests pin.
   - C. Leave it as it is.

## Proposed design

The shape, with the answers above. Question 6 is still open and will add
a section.

### Poster star

- A non-favorite's card shows an outline star chip in the marks' slot
  while it's hovered. A favorite's filled star is always shown. Both are
  buttons: a left press toggles the favorite through the existing
  `Event::Toggle` and doesn't select the card, start a double-click, or
  open the panel. A right press still opens the context menu.
- The eye mark stacks below the star as it does now. The hover star is
  also shown in poster-only mode, above the title overlay's layer order.
- It follows the card's fade (`alpha`) like the other marks and takes no
  input while the card is fading out or a flow or dialog is open.
- `gui-style.md` is amended: marks are no longer all non-interactive, and
  the star is.

### Details motion

- A single details progress for the grid, tweened between 0 and 1 when the
  zoom crosses 90%, drives the details' alpha and offset through the
  existing `caption_alpha` path. The text decision stays on the target
  zoom, so a Full card keeps drawing its details while they fade out.
- The details are clipped to their card, so they never draw over the next
  row while the heights spring.

### Slider snap

- `SNAP_IN` 8 and `SNAP_OUT` 11, with the tests and the style guide.

## Implementation sequence

1. **Snap.** The constants (8 and 11), `zoom/tests.rs`, `gui-style.md`, and `gui.md`
   if it quotes them.
2. **Poster star.** The hover-aware marks layer, the button and its click
   rules, the corner room, the style amendment, and tests:
   a star click toggles and doesn't select or launch, a right click still
   opens the menu, hover is ignored during a flow, the star fades with its
   card, and the GOG pill's room is the same at rest and on hover.
3. **Details motion.** A spike first: a throwaway worktree with Xvfb
   shots of 80% to 90% and back, checking clipping and overlap. Then the
   progress value, the alpha and offset, and motion tests that drive the
   frame clock through a crossing in both directions, mid-flight reversal,
   and a crossing during a sort.
4. **Docs.** `gui.md`, `gui-style.md`, and `features.md`.

## Testing and acceptance

- Unit and layout tests as listed in the steps. Motion tests drive the
  frame clock like 0055's.
- A benchmark is not needed: no new frame subscriptions outside a crossing.
- The acceptance checklist is added after implementation, as the
  workflow says.

## Scope limits

- No reduced-motion setting: the "GUI accessibility settings" row.
- The poster-only toggle and the dock keep their springs; only the 90%
  crossing gets the new motion.
- No other marks become clickable. The hidden eye stays a plain mark.
- No new poster hover effects beyond the star.
- No percentage or label for the slider.
