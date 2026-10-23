# GUI style guide

This is Wineo's approved visual direction from
[plan 0042](plans/0042-gui-visual-polish.md). Use it when extending the desktop
app so new pages feel like part of the same application. The maintainer approved
the Grouped HTML preview on 2026-10-03, with smaller posters for implementation.
The [original HTML preview](design/0042-ui-polish-preview.html) is saved unchanged
as a visual reference. Its Grouped variant is approved; the Cards alternative
and oversized preview posters are not implementation requirements.

## Layout and hierarchy

- Keep the Library at the top of the sidebar and management destinations at
  the bottom. Use a restrained tinted selection, readable text, and small
  status badges.
- Library posters are 160 x 240 px at 100% zoom with a 2:3 aspect ratio and
  16 px gaps. The zoom runs from 60% to 200% and scales the posters, never
  the gaps or the text (plan
  [0055](plans/0055-grid-zoom-and-poster-only.md)). Titles and compact
  state/playtime metadata sit beneath the art by default; below 90% only
  the title does, and in poster-only the title moves over the art (see
  Poster corners and overlay). Artwork should support browsing without
  dominating the window.
- Use the named default target's desktop title when available, with the
  environment identifier as the fallback. Keep the identifier visible as
  muted context when the title differs. Search matches both, and tags;
  operations and selection always use the identifier.
- Use a 420 px docked detail panel with a distinct surface, thin border, and
  subtle shadow. Keep hero art bounded and actions easy to reach.
- Page titles use 22 px semibold text. Section headings use 16 px semibold;
  body and controls use 14 px; supporting labels use 12-13 px muted text.
- Text uses the bundled Noto Sans, which has only regular and semibold.
  Don't ask for other weights such as bold or medium: they would fall back
  to whatever the system has. The wordmark is semibold too.
- Use 24 px outer page padding, 16 px inside grouped surfaces, and 8-12 px
  between related controls. Related rows share one surface with subtle
  separators rather than becoming individual cards.
- Round grouped surfaces by 12 px and controls by 8 px. Use 1 px borders.
  Avoid large empty areas, repeated framing, or decoration that competes with
  the content.

## Colors and controls

`crates/wineo-gui/src/theme.rs` owns the semantic palette. Reuse its tokens;
do not copy literal preview colors into page implementations.

| Token | Dark | Light | Role |
|---|---|---|---|
| background | `#16171b` | `#f6f7f9` | Window and page backdrop |
| surface | `#1f2126` | `#ffffff` | Grouped sections and panel |
| border | `#2c2f36` | `#d9dde4` | Quiet separation |
| foreground | `#e6e6e9` | `#20242b` | Main text |
| muted | `#8b8f98` | `#59616d` | Supporting text |
| primary | `#6d8cff` | `#3158ca` | Main action and focus |
| success | `#3ecf6e` | `#198044` | Healthy and completed states |
| warning | `#e5a34a` | `#a65d12` | Attention and recovery |
| danger | `#e5534b` | `#b93431` | Stop, destructive actions, errors |

Buttons are compact rounded rectangles, 32 px high with 8 px corners and
consistent horizontal padding. Play, Stop, and Log share an 82 px width.
Play/Launch and Stop occupy the first panel action row. Log, Settings, Art,
and Actions sit on a quieter row below. Use Play for the exact `game` tag and
Launch otherwise. The target picker has a visible **Launch target:** label.
Primary actions use a filled accent; secondary actions use a neutral surface
and border. Destructive actions use danger only where their consequence calls
for it. Keep text labels visible. Every text action button in the app uses
these controls through the helpers in `ui.rs`, never iced's stock button
style. Give each step or action row at most one accent button, for its main
forward action (Next, Review, Import, Create); Back, Cancel, Browse, and
Discard stay neutral. A disabled main action keeps its accent and dims. A
dialog that confirms Remove uses danger, like the context menu row. Back
buttons say **Back**, with no arrow. Lists of choices, like **Add**'s or a
program list, use quiet full-width rows in one grouped surface, not
buttons.

Scrollbars use a thin rail: 6 px wide with a 4 px thumb in the border color
and no visible track. Outside the Library the thumb is always shown, so a
short window still signals more content. The Library hides its thumb while
idle and shows it on hover or scroll.

Hover, press, disabled, and keyboard focus must be visibly distinct in both
palettes. Use a clear focus outline without changing button geometry. Disabled
controls keep readable labels and never accept input. Preserve existing action
availability, focus order, shortcuts, and operation ownership.

Dropdown fields use 14 px text, compact padding, 8 px corners, a neutral
surface, and a thin border. Hover and opened states emphasize the primary
color. Their popups use the same surface and corners, a subtle shadow, and a
readable selected row. Reuse the native chooser behavior and preserve open
and close callbacks. Context menus use quiet, full-width rows, separators
between action groups, and a danger treatment for Remove. Bound long content
to the window and keep the Close control reachable. A card's context menu
grows to 600 px before it scrolls; the window bounds it below that.

## Library toolbar and marks

Plan [0054](plans/0054-library-sorting-and-favorites.md) added these.

- The toolbar reads the wordmark, the accent **Add**, search, **Sort:
  <key>** with a chevron, a 32 px square direction button, **Filters** (or
  **Filters (N)** with N chosen stores, platforms, and tags), the zoom
  slider, and the poster-only toggle. All are 32 px high, and Tab follows
  that order, with the filter tab's chips right after **Filters**. Its
  bottom padding is 15 px, room for the filter tab. Search is 320 px wide
  and shrinks first as the window narrows, down to 160 px; then the right
  end clips.
- The zoom slider is 96 px with no label or readout, a small muted tick at
  100%, and the usual focus ring; its tooltip says "Zoom". It snaps to 100%
  within 8 points and lets go past 10. The poster-only toggle is a 32 px
  square icon button, an outline poster over a short title line, with the
  tag chips' selected look while on (15% primary tint, primary border and
  icon); its tooltip says "Hide titles" or "Show titles".
- Icons that Noto Sans lacks (arrows, chevron, check, star, crossed-out
  eye, search) are inline SVGs tinted with palette tokens, never glyphs.
- A tooltip is 13 px text padded 4 by 8 on the surface color, with a 1 px
  border, 8 px corners, and a 6 px gap. Toolbar tooltips sit below their
  button. The direction button's names the current order ("Most played
  first", "A to Z").
- The Sort menu and the Filters popup share the context menu's chrome,
  248 px wide. Only one of them or the context menu is open at a time. A
  click, Enter, or Space opens it under its button.
  It opens with focus on the current choice and takes every key while open:
  Tab and the arrows move between rows, scrolling the menu just enough to
  show the focused row (to the top for the first row), and Esc closes it and
  returns focus to its button. The Filters popup grows with its content up
  to the window's edge, then scrolls.
- The filter tab floats 3 px under **Filters**, left edges aligned, over
  the gap above the grid: 26 px high, the surface color, a 1 px border,
  8 px corners, and 4 px between items. It leaves 3 px above the first
  poster, and the grid sits in the same place with or without it. A chip
  is a pill with a 1 px border, a 13 px label, and a 12 px x in a 16 px
  round target; **+N** is the same pill as a button. The clear-all x is
  14 px in a 24 px round target. The x's are quiet: no fill until hovered,
  and a primary border while pressed. The tab keeps 16 px from the
  window's edge or the docked panel. While a menu is open over it, it
  shows but takes no input.
- Check rows ("Favorites first", stores, platforms, "Show hidden (N)")
  use a drawn 16 px check box: a bordered square with 4 px corners, filled
  with the primary color and a check in its ink color when on.
- Tag chips are pills that wrap. A selected chip shows a check before its
  label, a 15% primary tint, and a primary border, so selection never rests
  on color alone. The keyboard focus ring stays a separate outline outside
  the chip.
- An empty grid shows one grouped surface over the grid area, at most
  440 px wide and clear of the docked panel: a muted SVG icon, a 16 px
  semibold heading, one muted line, and one accent action. A secondary
  action, like **Show hidden** beside "2 matches are hidden", stays neutral.
  Copy stays neutral ("matches", not "games").
- A file problem that affects the whole library shows a warning-tinted,
  rounded strip under the toolbar with the message and at most one neutral
  action (**Open Checkup**). It clears itself when the problem does.
- Poster marks sit in the poster's top-right corner: a 24 px round chip in
  black at 55% opacity with a 14 px white icon, a star for a favorite and a
  crossed-out eye for a hidden game, stacked when both apply. They aren't
  clickable, and they don't change the card's size.
- A hidden card shown with Show hidden on keeps its color art at 45%
  opacity with a muted title; its marks stay at full strength. Grayscale
  stays the not-installed signal.
- The panel's star is a 32 px neutral button at the right of the title
  (through `ends`): filled in the primary color for a favorite, an outline
  in the foreground color otherwise, with a tooltip naming the action.

## Poster corners and overlay

Plan [0055](plans/0055-grid-zoom-and-poster-only.md) added these. Marks,
chips, and badges keep their sizes at every zoom.

- Poster-only shows each card as its poster alone. Hovering or selecting a
  card shows its title over the poster's bottom: a black gradient fading in
  over 24 px to 75% behind the text and 85% at the edge, then 14 px
  semibold white text, two lines at most with an ellipsis, 8 px from the
  left and bottom. It's dark in both themes, sits under the selection
  border and the corner layers, isn't a pointer target, and leaves 32 px
  on the right when the Steam mark shows. A hidden or not-installed card's
  overlay title is white at 60%, which measures at least 4.5:1 over bright
  and dark art in both themes. Cards with no art keep the title on their
  tile and get no overlay; art that's loading or failed follows the art
  cards.
- In poster-only, and below 90% with titles shown, a card that isn't idle
  gets a status chip at the art's top-left, 8 px in: a 24 px circle on the
  marks' backing with a 10 px dot in the state row's dot color. Idle means
  inactive, exited, stopped, or not installed; Steam's install and update
  states aren't idle.
- The GOG pill sits 4 px right of the chip, or in the corner without one.
  The corner keeps 36 px clear on the right while a mark shows. When the
  pill's longest word can't fit on one line in that room, it becomes a
  24 px badge instead: a circle in the pill's color with a 14 px down arrow
  in its ink color. The panel keeps the words. At 60 to 62% with a chip,
  a badge, and a mark, the badge sits flush against the mark.
- Every poster layer fades with its card, including the marks, the pill
  or badge, and the install bar's track.

## Panel motion

Opening and closing use a restrained slide from the right with a fade over
about 160 ms. Library cards move between their old and final slots with a
continuous damped spring over about 300 ms. A small stagger and bounded
overshoot of up to 8 px add a playful settle; posters stay upright and readable.
Ease to zero velocity at the end, including after a reversal. Compute
final geometry once per layout change, without rewrapping on every frame.
Keep the scrollable tree stable and move each card's hit area with its image.
The docked panel covers and shields the cards beneath it. Animate only while
a transition is active. Keep its 420 px content width and leave a narrow
library scrollbar rail accessible beside it.

Selecting another environment while open replaces content without replaying
the entrance. Rapid reversals continue from the current position. Leaving the
Library finishes motion immediately. Logical close and log cleanup happen
immediately; a closing or invisible panel must not receive input.
Filtering, resizing, and changed library contents settle or retarget safely.
Keep scroll position, bottom rows, keyboard columns, and visible art loading
correct through a transition.
Clicking unused Library space closes the panel. Card clicks, scrollbars,
menus, and the panel itself keep their own input handling.

## Grid motion

One owner moves the Library's cards, matching each card by name, so its
hover and state follow it. Every layout change springs each card from where
it's drawn now, with the panel's spring, so a re-sort during the dock or a
column change halfway through a zoom never jumps.

- A re-sort, a filter, search, or Show hidden change, and a favorite that
  moves a card spring the cards that stay. New cards fade in where they
  land and leaving ones fade out where they were, over about 150 ms,
  taking no input. The grid keeps its height until the fades end, and the
  empty message waits for the last one.
- A zoom step tweens over about 160 ms; a slider drag follows directly.
  Within one column count a zoom only scales; a column change, the
  poster-only toggle, and crossing 90% spring.
- A window resize and the background rescan snap. A rescan that lands
  mid-transition settles it first.
- A zoom or the toggle keeps the place: the selected card if any of it is
  visible, otherwise the first visible card, holds its top where it was.
  A manual scroll, arrow key, or click ends that.
- After a new order, the selection scrolls once, straight to where it
  lands. A scroll waiting on motion runs once when the last motion ends,
  or is dropped.

## Reviews and management pages

New-environment reviews use one grouped summary with Application, Source,
and Runtime sections as applicable. Render the frozen reviewed data directly.
Warnings and missing prerequisites stay prominent below the summary. Long
paths stay complete and wrap or scroll. Keep Back/Create outside the scrolling
summary so they remain reachable in a short window. Import pages work the
same way: Back stays at the top, the content scrolls (on grid pages the title
and notes scroll with the posters), and each step's main action row is pinned
at the bottom. On grid pages the main action sits at the right of that row
(`ends`), and posters still decoding half a second after the page opens show
a progress bar on the left. The art picker matches: Back and Filters stay at
the top, the title, filters, and images scroll, and only progress and **Use**
are pinned.

Determinate progress uses `progress::bar` (in `progress.rs`): a thin bar with
its state in words beside it, which changes only when its counts do. Work that
is often quick uses `progress::Delayed`, which shows the bar only if the work
is still running after 500 ms, so a fast load never flashes it.

A header or card with a left group and a right group uses `ends` (in
`ends.rs`), not a row with a fill spacer. It keeps the right group at the
right edge when both fit, gives the left side the rest of the line, and
stacks the right group underneath in a narrow window instead of squeezing
its buttons. Rows of buttons that can run out of room wrap.

Settings uses grouped sections and a fixed Save/Revert footer with the edit
count. Snapshots uses compact rows and bounded, complete reviews. Downloads
groups builds by component and keeps uses and progress visible. Checkup groups
issues and fix choices, with concise healthy and result states. Trash keeps
prefix inclusion, retained paths, recovery notices, and confirmations readable.

Keep populated, empty, busy, and error states consistent. Status includes text,
not just color. Preserve confirmations, recovery rules, conflicts, and ongoing
operations when navigating. Sample names and operations from the HTML mockup
are examples; the actual application's existing data and behavior take priority.

## Review checklist

Check light and dark palettes, mouse and keyboard focus, long paths, and a
small window. Preserve scroll position and stable widget-tree paths. Ensure
footers stay reachable, grouped content scrolls, and animations stop when idle.
New visual directions need maintainer approval before replacing these rules.
