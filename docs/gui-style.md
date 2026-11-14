# GUI style guide

This is Wineo's approved visual direction from
plan 0042. Use it when extending the desktop
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
  0055). The columns are
  centered in the width beside the panel: whatever doesn't fit another
  column splits evenly between the left and right margins, rounded down to
  a whole pixel, so a full row is laid out exactly as before (plan
  0056). Titles and compact
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

`crates/wineo-gui/src/theme/` owns the semantic palette and the built-in
themes (plan 0057). Reuse its roles; do not copy
literal preview colors into page implementations.

| Role | Use |
|---|---|
| background | Window and page backdrop |
| surface | Grouped sections and panel |
| border | Quiet separation |
| foreground | Main text |
| muted | Supporting text |
| primary | Main action and focus |
| success | Healthy and completed states |
| warning | Attention and recovery |
| danger | Stop, destructive actions, errors |

Each built-in theme sets all nine, in this order:

| Theme | Kind | background, surface, border, foreground, muted, primary, success, warning, danger |
|---|---|---|
| Midnight | dark | `#16171b` `#1f2126` `#2c2f36` `#e6e6e9` `#8b8f98` `#6d8cff` `#3ecf6e` `#e5a34a` `#ec625a` |
| Daylight | light | `#f6f7f9` `#ffffff` `#d9dde4` `#20242b` `#59616d` `#3158ca` `#198044` `#a65d12` `#b93431` |
| Cellar | dark | `#191513` `#231d1a` `#3a302a` `#eee6df` `#a8988c` `#e08060` `#5fcb8c` `#e9c25a` `#f26d6d` |
| Cellar Light | light | `#f3ede6` `#fbf8f4` `#ddd2c6` `#2a211b` `#6b5b4f` `#b44a2f` `#2f7a45` `#8f6400` `#b0233f` |
| High contrast | dark | `#000000` `#0e0f11` `#7a7f8a` `#ffffff` `#c9ccd3` `#94b4ff` `#62e48f` `#ffc46a` `#ff8277` |
| Dracula | dark | `#21222c` `#282a36` `#44475a` `#f8f8f2` `#6272a4` `#bd93f9` `#50fa7b` `#ffb86c` `#ff5555` |
| Catppuccin Macchiato | dark | `#1e2030` `#24273a` `#363a4f` `#cad3f5` `#a5adcb` `#c6a0f6` `#a6da95` `#eed49f` `#ed8796` |
| Catppuccin Latte | light | `#e6e9ef` `#eff1f5` `#ccd0da` `#4c4f69` `#6c6f85` `#8839ef` `#40a02b` `#df8e1d` `#d20f39` |

Midnight and Daylight are the defaults for dark and light. Every text role
(foreground, muted, primary, success, warning, danger) in a wineo theme
reaches 4.5:1 against both background and surface, and a new wineo theme
must too. Dracula and Catppuccin Latte ship their published colors and miss
it in places; Improve readability raises them. Accent labels need 4.5:1
active, hovered, and pressed, and 3:1 disabled, in every theme.

A few colors stay fixed in every theme, because they sit over artwork whose
colors no theme controls, or read as depth on any ground:

- the poster marks' black backings and white icons, the star on the poster,
  and white text over art;
- the poster-only title gradient, the panel hero's gradient and close
  button, and the art flow's gradient;
- scrims, dialog and Settings, in black;
- shadows, black at 16 to 20%;
- accent hover and press, which blend toward white and black, with the
  label picked again for the result.

Anything else takes its color from a role.

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
buttons. The rows take keyboard focus with the usual outline, on Add, New's
choice page and program list, Heroic's and GOG's lists, and the version
picker (plan 0056): the first row has focus when a list appears, or the row
that opened the next page when you come back to it, Up and Down move between rows and stop at the ends, Enter or Space
chooses, and Ctrl+Enter still submits the dialog without choosing the row,
except on a page that has no forward action, like New's choice page, where
it opens the focused row.
A runner offer's No and Yes leave Ctrl+Enter alone the same way, but this
isn't every control's default: left uncaptured outside a flow, Ctrl+Enter
reaches the Library's Enter binding. When a list changes, focus stays on the same row if it is still there and
moves to the nearest otherwise.

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

Plan 0054 added these.

- The toolbar reads the wordmark, the accent **Add**, search, **Sort:
  <key>** with a chevron, a 32 px square direction button, **Filters** (or
  **Filters (N)** with N chosen or excluded stores, platforms, and tags,
  or **Filters (Name)** while the view matches a preset, its name cut at
  120 px),
  the 32 px square dice, the layout switch, the zoom slider, and the
  poster-only toggle (the last two in the grid only). All
  are 32 px high, and Tab follows that order, with the filter tab's chips
  right after **Filters**. Its
  bottom padding is 15 px, room for the filter tab. Search is 320 px wide
  and shrinks first as the window narrows, down to 160 px.
- When the row still doesn't fit (plan 0063), controls fold from the right
  into a 32 px square **More** button with a three-dot icon (tooltip
  "More"): the poster-only toggle, zoom, the layout switch, the dice, then
  Filters, each only when the row still doesn't fit. Search stays at 160 px
  once anything folds. With everything folded, search becomes a 32 px icon
  button (tooltip "Search", or the typed text). Folds come from the row's
  real width and each control's measured width at layout, never from fixed
  breakpoints, so the sidebar, the interface size, and Filters' label all
  count. Every control stays in the widget tree while folded, unshown, so
  a fold never drops focus, text, or a drag: focus on a control that folds
  moves to More, and focus on More or the search icon moves to what comes
  back (the field, for search). The filter tab hides while Filters is
  folded.
- More opens a menu in the Sort menu's chrome under it, right-aligned, with
  the folded controls in toolbar order: **Filters** with its count or
  preset name and a right chevron (it opens the Filters popup under More),
  **Pick one at random** with the die, **Layout** with the segment at the
  row's right, **Zoom** with the slider at the row's right, and **Hide
  titles**, a check row. Each row sends what its toolbar control sends. The
  menu takes keys like the Sort menu, and More is one Tab stop where the
  first folded control was. More shows the selected look while open. The
  search icon opens the field in a drop-down in the same chrome; Esc, a
  click outside, or Tab, Shift+Tab, or Down out of it closes it and keeps
  the text, and the icon keeps the selected look while there's text.
- The zoom slider is 96 px with no label or readout, a small muted tick at
  100%, and the usual focus ring; its tooltip says "Zoom". It snaps to 100%
  within 8 points and lets go past 10. The poster-only toggle is a 32 px
  square icon button, an outline poster over a short title line, with the
  tag chips' selected look while on (15% primary tint, primary border and
  icon); its tooltip says "Hide titles" or "Show titles".
- The layout switch is a segment, 96 x 32 px: three 32 px sides, a list
  icon (dots and lines), a grid icon (2 x 2 squares), and a coverflow icon,
  each with a tooltip ("Show as list", "Show as grid", "Show as
  coverflow"). One thumb sits under
  the chosen side with the poster-only toggle's selected look, and the
  chosen side's icon is primary; hovering a side adds a faint foreground
  tint. The thumb slides with the panel's spring and snaps under reduced
  motion. The keyboard treats the segment as one control, like a radio
  group: it's a single Tab stop, its focus ring goes around the whole
  segment, and while it has focus Left moves to the side before and Right,
  Space, and Enter to the side after, stopping at the ends, so the arrows
  don't reach the library then. Ctrl+L wraps around. Held arrows don't repeat the
  flip. A click picks a side and never takes focus, since the thumb already
  shows the change. In the list, an empty slot holds the slider and
  toggle's place, so the toolbar keeps its shape. With the switch the toolbar's right end starts clipping
  about 72 px sooner.
- The dice shows a die face of five, in the foreground color, or muted
  while there's nothing to pick from; its tooltip says "Pick one at
  random (right-click for options)". Right-click, Shift+F10, or the Menu
  key while it has focus opens a 272 px menu in the Sort menu's chrome
  under it, with one check row, **Only games that can launch**. Focus
  moves to the row, the toggle leaves the menu open, and Esc closes it and
  returns focus to the dice. Only one toolbar menu or context menu is open
  at a time.
- Icons that Noto Sans lacks (arrows, chevron, check, star, crossed-out
  eye, search, die) are inline SVGs tinted with palette tokens, never
  glyphs.
- A tooltip is 13 px text padded 4 by 8 on the surface color, with a 1 px
  border, 8 px corners, and a 6 px gap. Toolbar tooltips sit below their
  button. The direction button's names the current order ("Most played
  first", "A to Z").
- **Group by** is a row in the Sort menu, between the sorts and Favorites
  first, showing the current grouping and a right chevron. Opening it shows
  a 160 px list beside the menu, on the left near the window's right edge,
  in the same chrome; Up, Down, and Tab move in it, Esc or Left returns to
  the row, and a pick closes both. It doesn't open on hover.
- A section header is a 32 px row across the grid's columns or the list:
  a chevron pointing down while open and right while collapsed, the name in
  16 px semibold, and the count muted, as "(12)". It tints like a list row
  on hover and press, takes Tab focus but not focus from a click, and sits
  the layout's usual gap from the cards above and below. Cards drawn over a
  header while they move take the pointer before it.
- The Sort menu and the Filters popup share the context menu's chrome,
  248 px wide. Only one of them or the context menu is open at a time. A
  click, Enter, or Space opens it under its button.
  It opens with focus on the current choice and takes every key while open:
  Tab and the arrows move between rows, scrolling the menu just enough to
  show the focused row (to the top for the first row), and Esc closes it and
  returns focus to its button. The Filters popup grows with its content up
  to the window's edge, then scrolls.
- The Filters popup starts with its presets: a muted 12 px **Presets**
  heading, one quiet 32 px row per preset, the active one tinted 12% primary
  while the view matches it, names cut with an ellipsis at 204 px, then
  **Update "X"** when shown and **Save as preset**, which becomes a name
  field, and a separator before the filters. It still opens on the first
  filter row. Rename and Delete show as rows indented 12 px under their
  preset, Delete in danger text, and deleting asks in a card with 8 px
  padding and a 1 px danger border, **Keep it** focused. Name problems show
  live under the field in 12 px danger text, and a failed save shows under
  the presets with **Open Checkup** when Checkup can repair the file.
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
  with the primary color and a check in its ink color when on. An excluded
  store or platform row shows the same square filled with the danger color,
  with a drawn x in its ink color.
- The Filters popup ends with a muted 12 px hint line, "Right-click or press
  X to exclude.", after **Clear filters**.
- Tag chips are pills that wrap. A selected chip shows a check before its
  label, a 15% primary tint, and a primary border, so selection never rests
  on color alone. An excluded chip shows a danger x before its label, a 15%
  danger tint, and a danger border. The keyboard focus ring stays a
  separate outline outside the chip.
- An excluded value's chip in the filter tab reads **Not <value>**, with
  the usual x, a danger border, and a faint danger tint.
- An empty grid shows one grouped surface over the grid area, at most
  440 px wide and clear of the docked panel: a muted SVG icon, a 16 px
  semibold heading, one muted line, and one accent action. A secondary
  action, like **Show hidden** beside "2 matches are hidden", stays neutral.
  Copy stays neutral ("matches", not "games").
- A file problem that affects the library, like a broken `library.toml` or
  an unreadable trash entry, shows a warning-tinted, rounded strip under the
  toolbar with the message and at most one neutral action
  (**Open Checkup**). Each problem gets its own strip, and it clears itself
  when the problem does.
- An offer, like new Steam games, uses the same strip in the primary tone,
  below any problem strips and nearest the grid: an accent action, and a
  neutral one to decline. Every strip sits in one always-present slot, so one
  coming or going doesn't reset the grid's scroll.
- Short-lived notes sit bottom-center on the surface with a thin border and
  stack 8 px apart instead of covering each other: an auto-import note on
  top, at most 480 px wide, then a feature notice, then the removal bar with
  **Undo**. A note that can act, like one naming a single imported game, is
  a neutral button with hover and focus states.
- The action bar for marked cards is the last item in that stack, 16 px
  above the window's bottom, and shows with no motion like the notes above
  it. It's 48 px high in the context menu's chrome: "N selected" in 14 px, a
  neutral **Tags** button with an up chevron, text buttons for **Favorite**
  and **Hide** in the same style, and a 32 px square x with a "Clear
  selection" tooltip above it, each a Tab stop in that order. The flag
  buttons read **Unfavorite** and **Show** once every marked game has the
  flag, so the bar grows about 22 px and, being centered, shifts about 11 px.
  While it shows, the grid keeps 72 px under its last row, and scrolling a
  card into view or holding one through a zoom treats that strip as covered.
- The Tags list opens above its button with left edges aligned, 248 px wide,
  in the Sort menu's chrome and keys. It grows up to the window's top edge,
  then scrolls, and ends with a separator and a "New tag" field whose
  problems show live under it in danger text. A tag some but not all marked
  games have shows the check box filled with the primary color and a drawn
  dash in its ink color.
- Poster marks are a 24 px round chip in black at 55% opacity with a 14 px
  white icon, 8 px in from their corner, each corner holding one so nothing
  moves when another appears. The star has the top-right corner and a
  hidden card's crossed-out eye the bottom-left. A favorite's filled
  star is always shown; a plain card shows an outline star there while it's
  hovered, fading in and out over about 150 ms. Unfavoriting under the
  pointer shows the outline star at full strength at once. The star is a
  button with a 28 px target centered on the chip: a left press toggles
  the favorite without selecting the card or opening the panel, and a
  right press still opens the context menu (plan
  0056). With the pointer on the star itself,
  its icon tints to the primary color and its chip darkens to about 80%
  black, fading over the same 150 ms. A click that favorites the card also
  pops it: the chip springs up to the 28 px target and settles over about
  240 ms while a thin primary ring ripples out once, clipped to the star's
  corner so it never reaches the top-left corner or a neighbor.
  Unfavoriting, F, the panel star, and the menu don't pop it, and reduced
  motion skips the pop. It has no tooltip and no keyboard focus, since F,
  the panel star, and the menu are the keyboard paths. The eye isn't
  clickable, sits above the install bar, and marks don't change the card's
  size.
- A hidden card shown with Show hidden on keeps its color art at 45%
  opacity with a muted title; its marks stay at full strength, apart from
  the hover star's fade. Grayscale
  stays the not-installed signal.
- The panel's star is a 32 px neutral button at the right of the title
  (through `ends`): filled in the primary color for a favorite, an outline
  in the foreground color otherwise, with a tooltip naming the action.

## Poster corners and overlay

Plan 0055 added these. Marks,
chips, and badges keep their sizes at every zoom.

- Poster-only shows each card as its poster alone. Hovering or selecting a
  card fades its title in over the poster's bottom in about 150 ms, and out
  the same way when neither holds: a black gradient fading in
  over 24 px to 75% behind the text and 85% at the edge, then 14 px
  semibold white text, two lines at most with an ellipsis, 8 px from the
  left and bottom. It's dark in both themes, sits under the selection
  border and the corner layers, isn't a pointer target, and leaves 32 px
  on the right when the Steam mark shows and 36 px on the left when the
  eye does. A hidden or not-installed card's
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
  The corner keeps 36 px clear on the right on every card, so nothing shifts
  when the hover star appears. When the
  pill's longest word can't fit on one line in that room, it becomes a
  24 px badge instead: a circle in the pill's color with a 14 px down arrow
  in its ink color. The panel keeps the words. At 60 to 62% with a chip,
  a badge, and a mark, the badge sits flush against the mark.
- A card marked for a bulk action (plan 0062) gets a 25% primary wash over
  its art, under the title overlay and the corner layers, and the selected
  card's border still draws over it. Its check leads the top-left corner: a
  24 px primary circle holding a 14 px check in the primary's ink, 8 px in.
  The status chip and the GOG pill or badge follow it in one row with 4 px
  gaps, jumping right at once when the mark comes or goes, and the pill's
  fit counts the check, so marking can turn it into its badge. The row may
  run flush against the star's circle, as the badge already could. When a
  marked card's check, chip, and badge can't all fit, below about 78%, the
  badge hides until the mark clears.
- Every poster layer fades with its card, including the marks, the pill
  or badge, the marked wash and check, and the install bar's track.

## Accessibility

Plan 0063 added these, under Settings, Appearance.

- **Interface size** feeds the window's scale factor, multiplied with the
  desktop's own, so every size in this guide grows together. Layout math
  stays in logical pixels: never scale a size by hand, and never read the
  physical window size for layout. The window keeps its size on screen, so
  a bigger interface shows less, and anything that can run out of room
  folds, wraps, or scrolls instead of clipping. Layouts can count on at
  least 400 by 300: the window's minimum size is that in interface pixels,
  so it grows on screen with the interface size and follows a change live.
  Some compositors, like Hyprland, don't enforce it.
- **Font**: text that names no font draws in the generic sans-serif family,
  which the chosen font is mapped to, so leave text on the default font
  unless it's monospace. Bold text goes through the semibold helper, which
  picks the chosen family's own semibold, then bold, then regular, so heavy
  text never comes from another family. A label drawn in a font for
  preview (like the Font list) falls back to the built-in font when that
  font can't draw it.
- **Motion**: with reduced motion (the setting, or the desktop's animation
  switch), everything that moves snaps and every fade stays. A snap is a
  transition that ends on its first update, so work that waits for motion
  to end still runs once. The busy spinner keeps turning, since it's the
  only sign that work is going on. New animation asks the motion setting
  whether to move.

## Tray menu

The tray menu reads **Open wineo**, **Add to library...**, a divider, up to
three **Play NAME** rows, a divider, up to eight **Stop NAME** rows then
**More in wineo...**, a divider, and **Quit**. A section with no rows hides
with its divider. Rows are plain text with no icons. Names escape `_` (dbusmenu reads it as a mnemonic) and are
cut at 40 characters with an ellipsis. A tray action never throws away a
page or a flow: over one, it shows the window as it is, with a one-line
notice when the action didn't happen. Anything with more to say than a menu
row, like Force stop, a hook prompt, or an error, belongs in the panel, so
a blocked or failed row shows the window on that card.

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

## List rows

Plan 0062 added these.

- Rows are 56 px and full width, one column through the same motion owner
  as cards, so re-sorts and filters animate the same way. A 32 x 48 px
  thumbnail of the grid art leads, grayscale when not installed and faded
  when hidden or not installed, like the poster.
- The title cell takes what's left after the other columns: the title over
  a muted environment name when they differ, then the row's marks at its
  right end: the poster's GOG pill, then the Steam mark, eye, and star as
  bare 16 px icons, without the poster's round backdrops.
  The star keeps its place when hidden and fades in on hover like the
  poster's. The install bar runs along the row's bottom edge, and a
  warning shows as the State column's dot.
- Columns are fixed: State 128, Playtime 96, Last played 112, Source 80,
  and Tags 176 px, with the title at least 200 px. They leave from the
  right as the width drops: Tags, Source, Last played, then Playtime. Text
  that doesn't fit is cut between graphemes and ends in an ellipsis.
- The header is 32 px and stays put above the scrolling rows. Every label
  but State sorts. Sortable labels are buttons that tint 6 px past their text on hover and press (10%
  and 22% primary, with a primary border pressed), with cells widened so
  labels line up with the row cells below. The active sort shows its
  direction arrow. The header is mouse-only; the Sort menu covers the
  keyboard.
- A selected row gets a 2 px primary border, rounded 6; a hovered row fades
  in a surface-colored background with the poster's hover timing. A marked
  row gets the marked poster's 25% primary wash across the row and a 22 px
  check centered on the thumbnail.
- Switching layouts snaps rather than morphing posters into rows.

## Grid motion

One owner moves the Library's cards, matching each card by its slot (its
section and game), so its hover and state follow it. When a game's copies
change sections, a copy that stays keeps its place, and the others pair up
by game, first with first in shown order, so they spring rather than fade.
Section headers spring and fade like cards. Every layout change springs each card from where
it's drawn now, with the panel's spring, so a re-sort during the dock or a
column change halfway through a zoom never jumps.

- A re-sort, a filter, search, or Show hidden change, and a favorite that
  moves a card spring the cards that stay. New cards fade in where they
  land and leaving ones fade out where they were, over about 150 ms,
  taking no input. The grid keeps its height until the fades end, and the
  empty message waits for the last one.
- A zoom step tweens over about 160 ms; a slider drag follows directly.
  Within one column count a zoom only scales and re-centers; a column
  change, the poster-only toggle, and crossing 90% spring. A panel dock or
  undock springs the cards even when the column count stays the same,
  since the centering shift changes with the width beside the panel.
- A window resize and the background rescan snap. A rescan that lands
  mid-transition settles it first.
- The hover star and the poster-only title fade over the same 150 ms,
  each from where it's drawn, so a quick pass across cards never jumps.
  They keep their own frames, so a fade that a flow or dialog opens over
  still finishes instead of snapping.
- Crossing 90% fades the identifier and state/playtime row over the zoom's
  160 ms, drifting 8 px up into place on the way in and down on the way
  out, while the title stays at full strength. They draw in a band clipped
  to their card's column, down to the top of the card below, so they never
  cover another card mid-spring. The row stays on one line while it fades
  and wraps again at rest. The fade keeps its own frames, so a stopped
  zoom, a drag, or a flow opening still finishes it; the poster-only
  toggle snaps it.
- A zoom, the toggle, or a panel dock or undock keeps the place: the
  selected card if any of it is visible, otherwise the first visible card,
  holds its top where it was.
  A manual scroll, arrow key, or click ends that.
- After a new order, the selection scrolls once, straight to where it
  lands. A scroll waiting on motion runs once when the last motion ends,
  or is dropped.

## Coverflow

The strip is one custom widget drawn back to front from a fractional
position, so every frame is a pure function of that position and the
Library's size.

- The middle cover is 55% of the Library's height, at most 40% of the
  strip's width, 2:3. Up to four neighbors per side, each 0.125 smaller
  than the one before and never below half the middle, and each 0.15 more
  dimmed. The nearest neighbor sits 0.68 of the middle's width from its
  center, so it tucks under the middle; the rest step 0.3.
- Side covers tilt toward the middle: 60% as wide as flat, the far edge
  inset 10% top and bottom, cropped to the 2:3 middle. The tilt is a warp
  made once per cover on the decode thread and cached left and right, never
  drawn per frame. A cover stays flat within a landing's overshoot (0.15 stop)
  of the middle, crossfades to tilted by 0.3 stop, and is all trapezoid
  beyond.
  Covers are square, since tiny-skia ignores image corner radius.
- Everything about a cover follows one outline each frame: the trapezoid
  once turned, warp or not. A cover waiting on its warp is a blank card
  with its title and a hairline edge, never its flat art; an art-less name
  card has the same edge. Name cards and
  section stacks turn with the strip. The trapezoid fills are SVGs made at
  each cover's own aspect, since both renderers scale an SVG evenly to its
  longer side and ignore `preserveAspectRatio`.
- Dimming isn't a layer over the cover: its colors mix toward the
  background and its art drops in opacity over an opaque background.
  Badges and the caption get their own layers, since both renderers draw a
  layer's quads before its images, and each cover's art draws in a layer
  its own size, since tiny-skia clips an image only to its layer. A cover
  carries only the mark check and the favorite star, drawn as on a grid
  card, with the grid's gray and fade for not installed or hidden.
- The caption is the title and one state line in a 64 px band, 12 px under
  the middle cover. In a short window it lifts above the marks bar and the
  controller hint bar, and the middle cover shrinks only when lifting isn't
  enough.
- A collapsed section's card is its first game's cover with two blank
  cards offset behind it, the section name and count in the caption; an
  art-less first game gives the name card. The open section's name sits
  above the strip as a button that collapses it.
- The middle cover and its two neighbors decode at the middle's size in
  physical pixels, rounded up to 240 px steps and at most 1440 px tall, with
  the grid poster shown until it arrives. That decode and its warps wait
  until the strip rests, and a result for a cover that has moved away is
  dropped. Grid-size warps are made during glides too, for every stop from
  six behind where the strip is drawn to six past its target, asked again
  at each stop crossed unless the strip moves faster than four stops a
  frame; a longer jump holds only what's on screen and six ahead, plus six
  around where it lands. A
  warp that arrives late for a cover on screen fades in over its blank card
  in 150 ms. At rest the middle's neighbors show their grid warp until the
  large one is here, which replaces it without a fade. A
  resize moves the covers at once but asks for the larger decode only after
  250 ms without another resize.
- Motion: the position follows the grid cards' spring, stepped by elapsed
  time so a dropped frame skips ahead, with the swing past a stop capped at
  0.15 of a cover. A held key or stick glides through without stopping. The
  middle title crossfades over 150 ms. Reduced motion, a covered strip, and
  switching into coverflow snap; the title still fades. Frames are asked for
  only while something moves or a title or warp fades.

## Controller hints and remapping

- The hint bar is 32 px at the bottom of the Library, under the marks bar:
  a 20 px button symbol in a pill and a word for Open (Play with the panel
  open), Back, Favorite, Mark, and Random, each only when bound, and
  without Favorite and Mark on a section header or collapsed card. Symbols
  follow the last controller pressed: letter rings for Xbox and Nintendo,
  the four shapes for PlayStation, and words like LB, L1, or ZL for the
  rest. It shows after a controller press and hides on any mouse move or
  click, with no timer.
- Each remap row in Global settings is a button showing the action and its
  symbol, or "Unbound" in the warning color. A waiting row says "Press a
  button..." for 5 s. Reset to defaults is enabled only while something is
  remapped.

## Reviews and management pages

New environment's Summary uses one grouped surface with Application, Source,
and Runtime sections as applicable, checked live as the draft settles
rather than frozen on a review page. While a check runs, keep the last
Summary in place, dimmed under "Checking...", so it never blanks or
reflows, and keep the forward action off with its reason beside it until
the check matches the draft. Notes that change what the forward action does
repeat beside it in the pinned strip, so they're seen without scrolling.
Other flows' review pages still render their frozen reviewed data directly.
Warnings and missing prerequisites stay prominent below the summary. Long
paths stay complete and wrap or scroll. Each step of an Add, import, or
clone page that can go back has the same shape: Back alone in a row at the
top, the content scrolling (on grid pages the title and notes scroll with
the posters), and the step's forward action pinned in a strip at the
bottom. Off the grid pages the action sits at the left of the strip, and a
recovery alternative like **Use retained copy in place** wraps in beside
it. Steps that can't go back, like working and finished, keep their
controls in the content. A discard prompt that isn't an overlay sits above
Back, outside the scroll, in a slot that is always there so the content
keeps its state. On grid pages the main action sits at the right of the
strip (`ends`), and posters still decoding half a second after the page
opens show a progress bar on the left. The art picker matches: Back and
Filters stay at the top, the title, filters, and images scroll, and only
progress and **Use** are pinned.

Determinate progress uses `progress::bar` (in `progress.rs`): a thin bar with
its state in words beside it, which changes only when its counts do. Work that
is often quick uses `progress::Delayed`, which shows the bar only if the work
is still running after 500 ms, so a fast load never flashes it.

A header or card with a left group and a right group uses `ends` (in
`ends.rs`), not a row with a fill spacer. It keeps the right group at the
right edge when both fit, gives the left side the rest of the line, and
stacks the right group underneath in a narrow window instead of squeezing
its buttons. Rows of buttons that can run out of room wrap.

A settings row is a label, a control 12 px after it, and sometimes a note,
laid out by `fold_row` (in `fold_row.rs`) through `setting_row`. A control
with a fixed width sits beside the label while it fits, and otherwise folds
under it and shrinks to the room. A field that fills folds once it would get
less than 260 px beside the label. A note stays beside its control while it
fits, and otherwise drops under the control, wherever the control is.
Controls that size to their content, like tag chips, keep a plain row. The
Font button cuts a long family name with "..." at the end and, when it's cut
at the button's full width and the list is closed, shows the whole name in a
tooltip.

Settings uses grouped sections and a fixed Save/Revert footer with the edit
count. The global page has General and Appearance tabs in its header,
between the title and the divider: text buttons 32 px high, 14 px regular
labels (foreground when selected, muted otherwise), hover and press from
`list_row_style`, and a 2 px primary underline on the selected tab that sits
on the divider. The underline is always laid out, transparent when
unselected, so switching never shifts anything. The tabs are the first Tab
stops on the page, Left and Right move between them and select, and each tab
keeps its own scroll position while the page is open. Snapshots uses compact rows and bounded, complete reviews. Downloads
groups builds by component and keeps uses and progress visible. Checkup groups
issues and fix choices, with concise healthy and result states. Trash keeps
prefix inclusion, retained paths, recovery notices, and confirmations readable.

Keep populated, empty, busy, and error states consistent. Status includes text,
not just color. Preserve confirmations, recovery rules, conflicts, and ongoing
operations when navigating. Sample names and operations from the HTML mockup
are examples; the actual application's existing data and behavior take priority.

## Review checklist

Check light and dark palettes, mouse and keyboard focus, long paths, and a
small window. Library changes are also checked in coverflow, in a short and a
narrow window, and with a controller. Preserve scroll position and stable widget-tree paths. Ensure
footers stay reachable, grouped content scrolls, and animations stop when idle.
New visual directions need maintainer approval before replacing these rules.
