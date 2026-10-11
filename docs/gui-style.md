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
- Library posters are 160 x 240 px with a 2:3 aspect ratio and 16 px gaps.
  Titles and compact state/playtime metadata sit beneath the art. Artwork
  should support browsing without dominating the window.
- Use the named default target's desktop title when available, with the
  environment identifier as the fallback. Keep the identifier visible as
  muted context when the title differs. Search matches both; operations and
  selection always use the identifier.
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
to the window and keep the Close control reachable.

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

## Reviews and management pages

New-environment reviews use one grouped summary with Application, Source,
and Runtime sections as applicable. Render the frozen reviewed data directly.
Warnings and missing prerequisites stay prominent below the summary. Long
paths stay complete and wrap or scroll. Keep Back/Create outside the scrolling
summary so they remain reachable in a short window. Import pages work the
same way: Back stays at the top, the content scrolls (on grid pages the title
and notes scroll with the posters), and each step's main action row is pinned
at the bottom. The art picker matches: Back and Filters stay at the top, the
title, filters, and images scroll, and only progress and **Use** are pinned.

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
