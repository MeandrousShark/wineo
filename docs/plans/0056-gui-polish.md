Status: approved

# 0056: GUI polish

## Goal

Small touches to the desktop app that were too small for a plan each: star
a game from its poster on hover, slide a card's details in and out when the
zoom crosses 90%, make the zoom slider's 100% snap easier to hit, center
the grid so the leftover width isn't all on the right, fade the hover
effects, let a game have a title of its own on its card, and reach the
choice lists from the keyboard. More touches can join before approval.

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

Answers to the questions (2026-10-22), numbered as asked:

1. **A favorite's star toggles.** A favorite's filled star is always
   shown, and clicking it unfavorites, exactly as the hover star favorites.
2. **Reserve the star's room on every card.** The corner keeps the 36 px
   the marks ask for whether or not a mark shows, so nothing shifts on
   hover. A narrow unmarked card's GOG pill turns into the round badge a
   little earlier than today.
3. **No tooltip on the star.** The maintainer thinks it's obvious. It
   wouldn't help a screen reader, since iced exposes no accessibility
   tree; the "GUI accessibility settings" row is where that belongs.
4. **Fade and slide.** The details fade and drift up about 8 px into place
   (down and out when they leave) on the zoom's 160 ms tween. Cards still
   spring to their new places as today, and a throwaway spike checks the
   outgoing details draw cleanly while they do (see Details motion).
5. **Widen the snap** in this plan. The first numbers, 8 in and 11 out,
   were amended by answer 11.
6. **Center the grid** (A), so the leftover width splits evenly left and
   right and the cards keep their exact zoom sizes.
7. **A `title` key on the environment.** The maintainer expects it to be
   replaced or filled later by IGDB or another metadata integration, so
   the key stays the person's own word: anything a source fills in later
   must not overwrite a title that is already set.
8. **Search and sort use the title.** Search matches the title and the
   environment name, and Sort by name sorts by what the card shows, with
   ties falling back to the environment name.
9. **Up and Down move focus between rows on the choice lists**, Enter
   chooses, and Tab still walks the rows.
10. **A Steam game keeps its live name.** The `title` key stays a fallback
    there, as today: Steam's own name wins, and Settings shows the key
    read-only for a Steam game. The key means the card's title for every
    other kind.

From the plan review (2026-10-22): `qa-gpt` and an outside read of the
draft, with each point checked against the code.

11. **Snap 8 points in and 10 out.** With 11 out, dragging from 100%
    straight to 90% or 110% stays held until 89 or 111, so those sizes
    needed an overshoot and a return. At 10 out they show on the way.
12. **The star's slot belongs to the star.** A hidden card always leaves
    the top mark slot for the star, so its eye sits in the second slot,
    at rest and on hover, and never moves when the star appears or the
    card is favorited. Today a hidden card that isn't a favorite shows the
    eye in the top slot, so this moves it down one slot.
13. **`title` is only the person's.** Nothing writes it automatically: no
    GOG fill, and a later metadata source reads or fills its own place,
    not this key. The card shows, in order, the person's `title`, a Steam
    game's live name, the default target's desktop-entry name, then the
    environment name. Rules for what the person types: trimmed, not empty,
    at most 120 characters, no control characters, any Unicode.
14. **All five list pages get keyboard focus,** and Up and Down stop at
    the ends instead of wrapping. No row is focused when a list is empty.
15. **The review findings are folded in** (this revision).

From step 3 (2026-10-22):

16. **Keep the star's room on every card, as built.** Measured: a GOG card
    with an update pill and a status chip shows the round badge at 80%
    where it showed a wrapped pill, and in poster-only at 100% the "Finish
    update" pill wraps where it fit on one line. Every GOG pill has up to
    28 px less width in its row. The maintainer accepted it, since nothing
    shifts on hover.

From steps 6 and 7 (2026-10-22):

17. **`wineo clone` keeps copying the title,** like its other keys. The
    clone's card shows the same title until it is changed in Settings.
18. **Ctrl+Enter on other focused controls stays as it is.** Only the new
    list rows ignore Enter while Ctrl is held. Other focusable controls in
    flows, like the runner offer card, still press and also submit the flow.
    That was true before this plan and is parked in the backlog.
19. **Up and Down scroll the focused row into view,** just enough to show
    it, like the Sort menu does.

Added to the plan by the maintainer (2026-10-22), from the polish
candidates:

- Fade the hover effects: the poster star and the poster-only title
  overlay.
- A title of its own on a card, from the "Display titles for every card"
  backlog row.
- Keyboard focus for the choice lists, from the "Keyboard focus for list
  rows" backlog row.
- The zoom slider stays where it is in the toolbar.

## Existing foundation

Mapped on 2026-10-22; line numbers are in `crates/wineo-gui/src` unless a
path says otherwise. Points marked (review) came from the plan review and
were checked against the code, or are confirmed again by the step.

- **Marks.** `poster_marks` (`library_actions.rs:416-436`) stacks a star for
  a favorite and a crossed-out eye for a hidden game in the top-right
  corner, 24 px chips on a 55% black backing, in a column with the star
  first. It returns nothing unless the card is a favorite or hidden, so a
  hidden card that isn't a favorite has its eye in the top slot. The corner
  room the GOG pill leaves (`MARKS_ROOM`, `grid.rs:302-306`) keys off the
  same test and is 8 px of margin, the 24 px chip, and a 4 px gap: 36 px
  of room, not a 36 px target. Marks are a plain container, so they aren't
  clickable today.
- **Hover.** There's no per-card hover widget. Each card is one
  `mouse_area` (`grid.rs:1241-1251`) with press, double-click, right-press,
  and enter and exit handlers, and the app keeps a single
  `hovered: Option<String>` (`main.rs:168`). Hover is ignored while a flow
  or dialog is open (`main.rs:812`). The poster-only title overlay already
  uses `hovered || selected` (`grid.rs:213-214`). iced's `mouse_area`
  decides hover from the cursor over its own bounds, so a button inside it
  doesn't change it.
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
  tween is heading to, so a card switches the moment the zoom sets out
  (`main.rs:1368-1376`). A `Title` card doesn't build the identifier or the
  meta row at all (`grid.rs:446`, review). Only positions spring
  (`grid_motion.rs:353`, review): a card's height changes at once to the
  new layout, and `zoom_to` springs the slots when the text or the column
  count changes (`grid_motion.rs:525-538`). `caption_alpha`
  (`grid.rs:195-196`, `411-453`) fades the title along with the
  identifier and meta row, so it can't drive the details alone. No
  per-card scalar for size or height exists.
- **Snap.** `SNAP_IN` 5 and `SNAP_OUT` 7 (`zoom.rs:18-21`), tested in
  `zoom/tests.rs:256-293` and quoted in `gui-style.md:108-109`.
- **Slots.** Cards are placed left to right from the 16 px padding.
  `columns` (`grid_motion.rs:656-660`) floors the columns that fit, and
  `slots` places them from the padding, so the leftover width always sits
  on the right: up to about one card wide, 312 px at 200% in a 1000 px
  area. The widget's layout (`grid_motion.rs:935-975`) and the places the
  motion owner springs to (`grid_places`, `app_grid.rs:64-96`) both derive
  from the same sizes and column count. `PlacesKey` holds the order, the
  layout, and the column count, but not the width (`grid_motion.rs:305-312`,
  review), so a width change that leaves the columns alone can reuse stale
  places. The dock spring and the resize snap paths themselves are sound
  (review).
- **Hover fades.** The poster-only overlay is a plain boolean,
  `PosterOnly && art && (hovered || selected)` (`grid.rs:213-214`), so it
  appears and disappears at once. `GridMotion` already has a 150 ms `FADE`
  for cards entering and leaving (`grid_motion.rs:22-23`), and a leaving
  card takes no input (`grid_motion.rs:1040`, review).
- **Titles.** A card's title is the default target's desktop-entry name
  when the entry is on and the name is set, otherwise the environment name
  (`display_title`, `library.rs:249-261`). A Steam environment's card
  shows Steam's live name instead (`library.rs:424-433`). `title` is
  already a proxied-only key (`PROXIED_ONLY_KEYS`, `config.rs:529`), written
  at import from the Steam name (`steam_import.rs:310`) as a fallback for
  when Steam forgets the game, and every other kind refuses it
  (`config.rs:389-392`), as every released wineo does. Settings tracks a
  title only for Steam environments (`settings/form.rs:772`, `1144`,
  review). Search and the name sort already read `display_title` and the
  identifier (`order.rs:208`, review). Templates refuse a `title`
  (`templates.rs:38`), and JSON output already carries a Steam title
  (`report.rs:73`). GOG migration adds only `gog_id` to a config
  (`gog/migrate.rs:170`, review). A native environment named `rimworld`
  with no desktop entry shows `rimworld`.
- **List rows.** The quiet rows (`ui::list_row`) take hover and press but
  never keyboard focus, on five pages: Add (`flow/mod.rs:31`), New's
  program list (`flow/new.rs:1552`), Heroic's source list
  (`flow/heroic/view.rs:92`), the GOG install program list
  (`flow/gog_install.rs:1228`), and the version picker
  (`version_picker.rs:745`), whose builds change with the branch. Add has
  its 1 to 5 keys (`app_keys.rs:31`) and Ctrl+Enter submits the flow
  (`app_keys.rs:42`). `focus_button.rs` has the focusable control, its
  focus ring, and the cycling Tab uses, which wraps; a focused control
  captures Enter and Space whatever modifiers are held
  (`focus_button.rs:267`, review).

## Proposed design

The shape, with the answers above. No questions are open.

### Poster star

- A non-favorite's card shows an outline star chip in the top mark slot
  while it's hovered. A favorite's filled star is always shown. Both are
  buttons: a left press toggles the favorite through the existing
  `Event::Toggle` and doesn't select the card, start a double-click, or
  open the panel. A right press still opens the context menu.
- The button's target is a 28 px square centered on the 24 px chip, inside
  the corner's margin. Moving the pointer onto it keeps the card hovered.
- The eye mark sits in the second slot on every hidden card (answer 12),
  so it doesn't move when the star appears, fades, or is toggled.
- The hover star is also shown in poster-only mode, above the title
  overlay's layer order.
- It follows the card's fade (`alpha`) like the other marks and takes no
  input while the card is fading out or a flow or dialog is open.
- Clicking it can re-sort the grid (Favorites first); the card then springs
  away from under the pointer through the existing path.
- `gui-style.md` is amended: marks are no longer all non-interactive, the
  star is, and a hidden card's eye is in the second slot.

### Hover fades

- The hover star and the poster-only overlay fade in and out over the
  grid's 150 ms `FADE` instead of switching at once. The star follows
  `hovered`, and the overlay follows `hovered` or `selected`, so a selected
  card keeps its overlay. One per-card progress value for each, retargeted
  from where it is, so a quick pass across cards never jumps. Values for
  cards that are removed, and values at rest, are dropped, so an idle grid
  has no frame subscription.

### Details motion

- A single details progress for the grid, tweened between 0 and 1 over the
  zoom's 160 ms when the zoom crosses 90%, drives the identifier and meta
  row's alpha and their 8 px offset. It is separate from the title's
  alpha, which stays at the card's own. A card entering during the tween,
  after a filter or a sort, reads the same value, so it joins mid-way.
- The layout still decides the card's height from the target zoom, so the
  slots spring as today. The drawn text is separate: a card keeps building
  its identifier and meta row while the progress is above 0 on the way
  out, and starts building them on the way in, so the exit is a fade and
  not a cut.
- On the way out the card's own box has already shrunk to its `Title`
  height, so the details draw below it, over the gap and the top of the row
  that is still springing up. The spike (step 5) decides how to draw them
  cleanly: in a clipped band under the card, in the grid's overlay layer,
  or by delaying the height change until the fade ends. If none looks
  clean, the fallback is a fade without the slide, and the maintainer is
  told before the step goes on.

### Centered grid

- The leftover width is split evenly: every slot shifts right by half of
  `available - used`, where `available` is the viewport beside the docked
  panel. One function computes the shift, and the widget's layout, the
  places the motion owner springs to, the zoom anchor, and the visible-row
  test for art loading all read it, so they can't disagree.
- `PlacesKey` gains the available width or the shift, so places measured at
  one width aren't reused at another with the same column count.
- A window resize snaps as it does today. A dock or an undock springs the
  cards to their new places even when the column count stays the same,
  through the existing path.

### Card titles

- `title` joins the environment config for every kind, optional: for Wine
  and native environments it is the person's own title, and for a Steam
  environment it stays the import's fallback (answer 10). It needs no
  `schema_version` bump, but a released wineo already knows `title` only
  as a proxied-only key and refuses it on a Wine or native config, so
  downgrading means removing the key first. `config-files.md` says so next
  to its versioning rules.
- The card, the panel, and the poster-only overlay show, in order: the
  person's `title`, a Steam game's live name, the default target's
  desktop-entry name, then the environment name (answer 13). A card title
  can differ from the desktop entry's name, and `config-files.md` says so.
- It's set on the environment's Settings page and with `wineo set
  title=...`, and cleared the same ways. A Steam environment's Settings
  shows its key read-only. The value is trimmed and must be non-empty, at
  most 120 characters, and free of control characters. `wineo validate`
  reports a title that breaks those rules as a warning, a Steam fallback
  included, and never rewrites it. The Steam import's own write is
  unchanged.
- Nothing writes `title`. GOG migration, imports, and art lookups leave it
  alone, and a later metadata source gets its own place in the display
  order (answer 7).
- Templates keep refusing `title`, since one template serves many games.
  JSON output gains no field for a Wine or native title in this plan, and
  the Steam title it already carries is unchanged. Adding one later is a
  compatible change. `wineo list` and the other CLI output keep the
  environment name, which is what commands take.
- Search matches the title and the environment name, and Sort by name uses
  the shown title with the environment name as the tie-break (answer 8).
  The existing code already reads `display_title` for both; the step
  confirms the new key reaches them.

### Choice lists

- The rows on the five pages become `focus_button` controls with the usual
  primary focus ring. Up and Down move focus between rows and stop at the
  ends. Enter chooses, and Tab still walks the rows as it does now
  (answers 9 and 14). Focus lands on the first row when a page opens, which
  changes the focus order on those pages.
- A focused row captures Enter and Space as the control does, but ignores
  Enter while Ctrl is held, so Ctrl+Enter keeps submitting the flow and
  never also chooses the row. Add's 1 to 5 keys keep working with a row
  focused.
- When a list changes under focus, like the version picker's builds when
  the branch changes or a program list that fills in late, focus stays on
  the same row if it is still there and moves to the nearest row otherwise.
  An empty list focuses no row and leaves focus on the page's own
  controls.

### Slider snap

- `SNAP_IN` 8 and `SNAP_OUT` 10, with the tests and the style guide.
  Dragging between 90% and 110% passes through 100% as before, and 95% and
  105% stay unreachable by drag, as they are today.

## Implementation sequence

1. **Snap.** The constants (8 and 10), `zoom/tests.rs`, `gui-style.md`, and
   `gui.md` if it quotes them. Tests: a drag from 100 reaches 90 and 110 on
   the way (raw 89.6 shows 90, raw 110.4 shows 110), rapid back and forth
   across the zone never sticks outside it, and a drag that starts at 120
   or 80 snaps in at 8 and lets go past 10.
2. **Centered grid.** The shared shift, `PlacesKey` with the width or
   shift, the layout, places, anchor, and art visibility reading it, and
   tests: leftover split evenly at several widths and zooms, the same
   positions in the widget and the places, the anchor staying put through
   a zoom with the shift changing, a resize with the same column count
   mid-motion, and a dock and undock springing without a column change.
   Re-pin the slot x positions that move.
3. **Poster star.** The hover-aware marks layer, the 28 px button and its
   click rules, the second slot for the eye, the corner room, the style
   amendment, and tests: a star click toggles and doesn't select or
   launch, a right click still opens the menu, the card stays hovered with
   the pointer on the star, hover is ignored during a flow, the star fades
   with its card, the GOG pill's room is the same at rest and on hover,
   and a hidden card's eye keeps its place at rest, through the fade, and
   after the star is toggled, including a click that re-sorts.
4. **Hover fades.** The per-card hover progress, the star and overlay
   reading it, and motion tests: fade in, reverse mid-fade, a selected
   card keeping its overlay, a card removed mid-fade, a flow opening or
   closing during a fade, and no frame subscription once everything has
   settled.
5. **Details motion.** A spike first: a throwaway worktree with Xvfb shots
   of 80% to 90% and back, trying the three ways of drawing the outgoing
   details and checking clipping and overlap. The maintainer sees the
   shots before the step goes on. Then the progress value, the separate
   alpha and offset, the retained drawing, and motion tests that drive the
   frame clock through a crossing in both directions, mid-flight reversal,
   a card entering mid-crossing after a filter or a sort, and the title
   staying at full strength throughout.
6. **Card titles.** The `title` key in core for every kind with its rules,
   `wineo set`, Settings (change tracking beyond Steam environments, a
   read-only field for a Steam environment, and a concurrent edit), the
   display order in the card, panel, and overlay, search and sort,
   `validate`, and `config-files.md`. Tests: set and unset on Wine and
   native, a legacy file with no `title` untouched by a read or a no-op
   edit, every rule, the display order with and without a desktop-entry
   name, a Steam environment keeping its live name and fallback, search
   and sort, templates still refusing it, and an older-wineo-shaped
   refusal documented rather than tested.
7. **Choice lists.** `focus_button` rows on the five pages, the arrow
   keys with stops at the ends, the focus ring, the changed focus order,
   the Ctrl+Enter rule, and key tests: Up and Down on each page, Tab
   unchanged, Add's 1 to 5 keys with a row focused, Ctrl+Enter submitting
   once without choosing the row, focus kept or moved when the version
   picker's list changes, and an empty list.
8. **Docs.** `gui.md`, `gui-style.md`, `features.md` (the two backlog rows
   leave it), and the changelog at release.

## Testing and acceptance

- Unit and layout tests as listed in the steps. Motion tests drive the
  frame clock like 0055's.
- A benchmark is not needed as long as the hover and details motion stop
  their frame subscriptions when settled, which the tests check. Idle CPU
  is measured once at the end with 0055's script.
- The acceptance checklist is added after implementation, as the
  workflow says. It will include one scenario that mixes rapid hover, a
  zoom, a sort change, and the panel docking in succession, since they
  share motion and layout state.

## Scope limits

- No reduced-motion setting: the "GUI accessibility settings" row.
- The poster-only toggle and the dock keep their springs; only the 90%
  crossing gets the new motion.
- No other marks become clickable. The hidden eye stays a plain mark.
- No new poster hover effects beyond the star and the overlay's fade.
- No percentage or label for the slider, and the slider stays in the
  toolbar where it is.
- No automatic titles and no metadata from the stores: descriptions,
  genres, and titles from a source stay in the "Game metadata from the
  stores" row.
- No JSON field or template support for `title` in this plan.
- Lists other than the five named, and dropdowns like `pick_list`, keep
  their current keyboard behavior.
