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

Each has a recommendation. The star is drawn as a 24 px chip like the
other marks.

1. **A favorite's star: does clicking it unfavorite?**
   - A. **Yes (recommended).** A favorite's filled star is always visible
     and clickable; clicking it toggles it off, exactly like the hover
     star toggles on. One rule: the star toggles.
   - B. No. Only non-favorites get a clickable hover star; to unfavorite,
     use the panel, the menu, or F.
2. **Corner room: what happens to a GOG pill or badge when the star shows
   on hover?**
   - A. **Reserve the star's room on every card (recommended).** The
     corner keeps the 36 px the marks already ask for whether or not a
     mark shows, so nothing shifts on hover. Cost: on a narrow poster, the
     GOG pill of an unmarked card becomes the round badge a little
     earlier than today.
   - B. The star floats over the pill. Nothing changes for unmarked
     cards at rest, but on a small poster the hovered star can cover part
     of the pill or badge.
3. **Does the star have a tooltip?**
   - A. **Yes (recommended).** "Add to favorites" or "Remove from
     favorites", like the panel star's tooltip, in the usual 13 px
     tooltip.
   - B. No. The star is self-evident, and a tooltip on a 24 px target
     appears over the art.
4. **How do the details move?**
   - A. **Fade and slide (recommended).** The identifier and meta row fade
     as they do now, and also drift up about 8 px into place when they
     appear (down and out when they leave), on the zoom's 160 ms tween.
     The cards' heights keep springing as they do today. A throwaway spike
     checks that the text clips cleanly inside the card while its height
     is still changing.
   - B. Fade only. The cheapest: tie `caption_alpha` to the crossing and
     leave position alone.
   - C. Full reveal. The card's height grows continuously with the
     details, so the rows below are pushed smoothly instead of springing
     as a block. This needs a per-card height scalar through `GridLayout`,
     `card_height`, and `PlacesKey`, and a new rule for when `zoom_to`
     springs. Biggest change, and it touches the code that 0055's motion
     tests pin.
5. **Do we widen the slider's snap in this plan?**
   - A. **Yes (recommended).** 8 points in and 11 out: about a 9 px catch
     zone on the 96 px slider instead of 6, and 90% and 110% stay
     reachable when you drag toward them from outside. A two-constant
     change plus the snap test and the style guide's numbers.
   - B. Leave it for a later patch.

## Proposed design

Filled in after the questions are settled. The shape, assuming the
recommendations:

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

- `SNAP_IN` and `SNAP_OUT` change, with the tests and the style guide.

## Implementation sequence

1. **Snap.** The constants, `zoom/tests.rs`, `gui-style.md`, and `gui.md`
   if it quotes them.
2. **Poster star.** The hover-aware marks layer, the button and its click
   rules, the corner room, the tooltip, the style amendment, and tests:
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
