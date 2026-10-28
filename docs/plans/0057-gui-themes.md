Status: approved

# 0057: GUI themes

## Goal

Let people make the desktop app their own: pick from built-in color themes,
keep following the desktop's light or dark setting with a theme for each,
copy a theme and change its colors, and turn on a readability setting that
raises contrast where a theme needs it. Settings gets tabs so all of this
lives on its own Appearance tab.

This is queue item 1 in [features.md](../features.md), the "GUI themes and
customization" row. The palette tokens from 0036 are the starting point.

## Settled by the maintainer

The mockups are on a private design canvas,
<https://claude.ai/artifact/LzPLBLDtK1Rq2MGukKKnXd>: the Library screen in
every preset, and an interactive Appearance tab. The canvas still calls
Cellar "Ember" in its file names and preset ids (`ember`, `emberLight`); its
labels say Cellar.

Answers to the canvas questions (2026-10-27), numbered as asked:

1. **Colors over art stay fixed; the rest come from the roles.** The poster
   marks' black backing, the poster-only and hero gradients, scrims, and
   shadows stay the same in every theme, since they sit over artwork whose
   colors no theme controls. Everything else derives from the nine roles.
2. **Follow the system, with a dark theme and a light theme picked
   separately.** Following stays on by default. Turning it off picks one
   theme for both.
3. **Picking a theme previews it right away.** Save keeps it, Revert puts
   back what was saved. Settings gets tabs so Appearance has its own.
4. **Built-in themes are read-only.** "Copy to a custom theme" makes an
   editable copy with a field per role.
5. **Fix Midnight's danger color in this plan.** `#e5534b` is 4.35:1 on
   `surface`, under 4.5:1 for text; it becomes `#ec625a` (4.96:1).
6. **Two tabs: General and Appearance.** Every existing global section moves
   under General unchanged.
7. **Borrowed themes ship their exact published colors,** plus a new
   setting, **Improve readability**, off by default, that adjusts colors for
   better contrast in themes that need it.
8. **Improve readability is automatic.** It nudges each failing role's
   lightness, keeping its hue, until the role passes, so custom themes get it
   too. Tests pin the result for each built-in.
9. **The warm preset is ours, renamed Cellar.** The canvas called it Ember,
   which collides with the [Ember](https://embertheme.com/) theme. Cellar
   keeps its own colors but borrows Ember's coral, `#e08060`, for its accent
   (MIT, see Notices).

Answers to the plan's questions (2026-10-28):

10. **Appearance settings live in `library.toml`,** in an `[appearance]`
    table beside `[view]`. It ignores keys it doesn't know, so an older
    wineo just loses the preference.
11. **Custom themes are a file each** in the config root's `themes/`
    folder, read leniently, with `based_on` filling any gaps.
12. **Custom colors are a hex field beside a swatch.** A hue, saturation,
    and lightness picker could come later on top of it.
13. **The per-environment Settings page stays one page,** without tabs.
14. **One Save for both tabs.** The footer counts edits on General and
    Appearance and saves each file it touched.

A qa-gpt review of the plan (2026-10-28) found seven gaps. The maintainer
took all seven recommended fixes, which are folded into the sections below:
readability's best effort, the disabled label bar, fixed custom theme ids,
save order, the `based_on` chain, warnings without a fix, and the
immediate key and login actions.

## Existing foundation

- `crates/wineo-gui/src/theme.rs` owns `Palette`: nine colors (`background`,
  `surface`, `border`, `foreground`, `muted`, `primary`, `success`,
  `warning`, `danger`), the `DARK` and `LIGHT` constants, `from_mode`, and
  `faded`. `Palette::theme()` hands iced only six of them through
  `Theme::custom`, so stock iced widgets see no surface, border, or muted.
- `main.rs` reads the system mode at startup (`iced::system::theme()`) and
  follows `iced::system::theme_changes()`; `Message::ThemeChanged` replaces
  `self.palette` with `Palette::from_mode(mode)`. Reduced motion is read once
  at startup through ashpd.
- `ui.rs` derives control states from the palette: `secondary_style` hovers
  toward `foreground` and presses toward `primary`; `accent_style` hovers 10%
  toward white, presses 18% toward black, and picks its text with
  `accent_ink`, black or white by luminance at 0.179. The test at
  `ui.rs:395` checks that accent labels reach 4.5:1 in every state,
  disabled included, for the two modes only. Nothing tests that states
  stay distinct.
- Literal colors outside `theme.rs`, about a dozen files: poster marks and
  chips on black at 55 to 60% (`grid.rs`, `library_actions.rs`), the white
  star on the poster, the poster-only title gradient and white overlay text
  (`grid.rs`), the panel hero's gradient and close button (`panel.rs`), the
  art flow's gradient (`flow/artwork.rs`), the settings scrim at 50%
  (`settings/view.rs`), and shadows at 16 to 20% black (`ui.rs`, `panel.rs`,
  `grid.rs`, `library_menu.rs`). Already derived: the 15% primary selection
  tint, the scrollbar thumb (`border`), the log view's selection (`primary`
  at 40%), and the zoom slider's states.
- Tests construct `Palette::DARK` directly in at least seven files.
- `wineo-core/src/library.rs` reads and writes `library.toml`'s `[view]`
  table field by field (`set_poster_only` and friends), `library/check.rs`
  inspects it for `validate`, and `library/repair.rs` keeps each line it can
  still read. [config-files.md](../config-files.md) routes settings that only
  change how the app looks to `library.toml`, which ignores keys it doesn't
  know, so an older wineo just loses the preference. That avoids the
  unknown-key refusal every real config has.
- `Paths` resolves `library_file()` under the config root, which the
  portable folder overrides.
- The global Settings page (`settings/view.rs`, `global_view`) is a list of
  grouped sections with a fixed Save and Revert footer and an edit count.
  Edits collect on the page and only Save writes them; leaving with unsaved
  edits asks first ([gui.md](../gui.md), Settings). The key and login
  actions are the exception: they apply right away, outside the Save bar,
  and stay that way on the General tab.
- `crates/wineo-gui/THIRD_PARTY.md` holds the font and icon notices, and the
  `PKGBUILD` installs it beside the license.

## Proposed design

### Presets and roles

`Palette` stays the nine colors. A new `Preset` holds an id, a display name,
a kind (dark or light), a short description, its `Palette`, and where its
colors come from. Built-in ids are the config spelling:

| Id | Name | Kind | Source |
|---|---|---|---|
| `midnight` | Midnight | dark | wineo, today's dark with the danger fix |
| `daylight` | Daylight | light | wineo, today's light |
| `cellar` | Cellar | dark | wineo, accent from Ember |
| `cellar-light` | Cellar Light | light | wineo, accent from Ember's light coral |
| `high-contrast` | High contrast | dark | wineo |
| `dracula` | Dracula | dark | Dracula spec |
| `catppuccin-macchiato` | Catppuccin Macchiato | dark | Catppuccin palette |
| `catppuccin-latte` | Catppuccin Latte | light | Catppuccin palette |

The colors, in role order (background, surface, border, foreground, muted,
primary, success, warning, danger):

| Id | Colors |
|---|---|
| `midnight` | `#16171b` `#1f2126` `#2c2f36` `#e6e6e9` `#8b8f98` `#6d8cff` `#3ecf6e` `#e5a34a` `#ec625a` |
| `daylight` | `#f6f7f9` `#ffffff` `#d9dde4` `#20242b` `#59616d` `#3158ca` `#198044` `#a65d12` `#b93431` |
| `cellar` | `#191513` `#231d1a` `#3a302a` `#eee6df` `#a8988c` `#e08060` `#5fcb8c` `#e9c25a` `#f26d6d` |
| `cellar-light` | `#f3ede6` `#fbf8f4` `#ddd2c6` `#2a211b` `#6b5b4f` `#b44a2f` `#2f7a45` `#8f6400` `#b0233f` |
| `high-contrast` | `#000000` `#0e0f11` `#7a7f8a` `#ffffff` `#c9ccd3` `#94b4ff` `#62e48f` `#ffc46a` `#ff8277` |
| `dracula` | `#21222c` `#282a36` `#44475a` `#f8f8f2` `#6272a4` `#bd93f9` `#50fa7b` `#ffb86c` `#ff5555` |
| `catppuccin-macchiato` | `#1e2030` `#24273a` `#363a4f` `#cad3f5` `#a5adcb` `#c6a0f6` `#a6da95` `#eed49f` `#ed8796` |
| `catppuccin-latte` | `#e6e9ef` `#eff1f5` `#ccd0da` `#4c4f69` `#6c6f85` `#8839ef` `#40a02b` `#df8e1d` `#d20f39` |

Mapping borrowed colors onto nine roles is a choice no port avoids, so it is
recorded here. wineo puts panels one step lighter than the window in dark
themes and one step whiter in light ones.

- **Dracula** ([spec](https://github.com/dracula/spec)): background is the
  spec's AnsiBlack `#21222C`, since the spec has a single Background;
  surface is Background `#282A36`; border is Selection `#44475A`;
  foreground, muted, primary, success, warning, and danger are Foreground,
  Comment, Purple, Green, Orange, and Red.
- **Catppuccin** ([palette.json](https://github.com/catppuccin/palette)):
  background is mantle, surface is base, border is surface0, foreground is
  text, muted is subtext0, and the rest are mauve, green, yellow, and red,
  in each flavor.
- **Cellar's accents** come from Ember's
  [palette.json](https://github.com/ember-theme/ember): `accents.coral`
  `#e08060` as is, and `accents_light.coral` `#b84c30` darkened to
  `#b44a2f` so Cellar Light's accent passes 4.5:1 on its ground (4.56:1;
  the original is 4.39:1).

Every wineo preset passes 4.5:1 for each text role (foreground, muted,
primary, success, warning, danger) against both background and surface.
The borrowed Dracula and Catppuccin Latte don't (Dracula's muted is 3.03:1;
Latte's green 2.75:1 and yellow 2.15:1, among others) and ship as published
by answer 7; Improve readability is their remedy. Macchiato passes as
published.

`Palette::from_mode` gives way to a resolve step: the appearance settings
plus the system mode in, a `Palette` out, with Improve readability applied
last. `Palette::DARK` stays as Midnight's palette so existing tests keep
compiling. Tests that loop over palettes loop over every preset, with and
without readability.

Fixed colors follow answer 1. Everything over art keeps its literal values
(marks, the star and overlay text, gradients, scrims), and shadows keep
their black alphas, which read as depth on every ground. The audit step
routes anything else through a role, and `gui-style.md` gains the rule and
the allowlist. Accent hover and press keep blending toward white and black,
since `accent_ink` re-picks the text after the blend; a new state test
checks they stay distinct in every preset.

Accent labels need 4.5:1 when active, hovered, or pressed, and 3:1 when
disabled, since inactive controls are exempt from the 4.5:1 text rule.
Today's test holds disabled labels to 4.5:1, which Catppuccin Latte misses
(3.68 to 4.14:1), so step 0 lowers that bar before the test widens.

### Choosing a theme

- **Follow the system** is on by default. With it on, the Appearance tab
  shows a **Dark theme** list (the dark presets and dark custom themes) and
  a **Light theme** list, and the app uses whichever matches the system
  mode, switching live as it does today.
- With it off, one **Theme** list holds every preset, each row labeled Dark
  or Light, and the system mode is ignored.
- Defaults are Midnight for dark and Daylight for light, so nothing changes
  for anyone who never opens the tab, apart from the danger fix.
- Rows are the quiet full-width choice rows from `gui-style.md`, keyboard
  reachable like the other choice lists, each with a small swatch (ground,
  surface with an accent chip, text), the name, and a one-line description.

### Improve readability

A check row on the Appearance tab, "Improve readability", with the line
"Raises contrast in themes that need it." It's off by default and applies
to whatever theme is showing, built-in or custom.

The transform runs on a `Palette` and returns one:

- **Roles:** foreground, muted, primary, success, warning, and danger. Each
  must reach 4.5:1 against background and against surface; the lower of the
  two decides.
- **Direction:** both ways are tried for each role, lighter and darker.
  The one that reaches 4.5:1 wins, the smaller change if both do, and the
  theme kind's way (darker in a light theme) breaks a tie. Judging by the
  result rather than the kind keeps a custom theme with unusual grounds
  from being pushed the wrong way.
- **How:** lightness moves in small steps in OKLCH, holding hue and clamping
  chroma to what sRGB can show, until the role passes or reaches black or
  white.
- **Best effort:** some grounds leave no passing color. With a black window
  and a `#777777` panel, white is 4.48:1 on the panel and anything darker
  fails on the window. Then the role keeps the step with the highest
  contrast, which is never lower than where it started, and nothing in the
  UI says so.
- **Left alone:** background and surface, so the theme still looks like
  itself, and border, which is quiet by design; High contrast is the theme
  for visible borders.
- **After:** accent text is picked again with `accent_ink`, and every
  derived color (tints, hover, press) follows the adjusted roles.
- A role that already passes is untouched, so the transform does nothing to
  the wineo presets or Macchiato, and running it twice changes nothing.

The canvas shows hand-picked values for Latte and Dracula that are close to
what this produces; the tests pin the real output.

### Settings tabs

The global Settings page gets a tab bar under its title: **General** and
**Appearance**. General holds every existing section in today's order.
Tabs are a row of text buttons with a 2 px primary underline on the
selected one, in the Tab order after the title; Left and Right move between
them. The page opens on General. The per-environment Settings page keeps
its single scrolling page (answer 13).

### Live preview and saving

Appearance edits join the page's edit count like any other, with one
difference: the app repaints with them before Save.

- **Save** writes General's edits to `config.toml` and Appearance's to
  `library.toml`, each only if it has edits (answer 14).
- **Revert**, and **Discard** in the leave prompt, restore the saved
  appearance and repaint.
- A system light or dark change during a preview picks from the previewed
  lists, not the saved ones.
- Save writes in this order: `config.toml`, then theme files (new, edited,
  and renamed), then `library.toml`, then theme deletes, so a file is never
  deleted while `library.toml` still points at it.
- A failure stops the rest and works like today's partial save: the page
  reads the files back as the new base and keeps the edits that didn't
  land, and the preview keeps showing them until Save succeeds or Revert.
  When the failure is `library.toml` refusing writes (it doesn't parse or
  is from a newer wineo), the error carries the usual Open Checkup
  action.
- A hand edit to `library.toml` while the tab has no edits reloads it, as
  the page already does for `config.toml`.

### Custom themes

- **Copy to a custom theme** on the Colors section makes a copy of the theme
  showing: named "<Name> copy" (then "copy 2" and so on), same kind, and
  selected in place of its source. Built-in rows are read-only and say so.
- A custom theme's Colors section has a field per role: a swatch and a hex
  entry (answer 12). A bad value shows "Invalid color: use #rrggbb." and
  isn't applied; the swatch keeps the last good one.
- **Rename** and **Delete** sit beside the name. Rename changes only the
  name, never the id, so `library.toml` keeps pointing at it; a name
  another custom theme already uses, ignoring case, says "Name taken."
  Delete asks first; deleting a theme in use falls back to the default for
  its kind.
- Each custom theme is a file in the config root's `themes/` folder (answer
  11), written on Save:

```toml
schema_version = 1
name = "My Cellar"
kind = "dark"
based_on = "cellar"

[colors]
background = "#191513"
surface = "#231d1a"
# ... one key per role
```

  The id is set once, when the theme is created: a slug of its first name
  (lowercase ASCII letters and digits, anything else becoming a single
  `-`), with `-2`, `-3`, and so on added when that file exists. A copy's
  first name always ends in "copy", so the slug is never empty. The file
  is `<id>.toml`, and a theme file added by hand uses its own file name as
  its id. `library.toml` refers to it as `custom:<id>`, so a custom theme
  can never shadow a built-in:

```text
copy Cellar                         -> "Cellar copy", custom:cellar-copy
rename it "Mine"                    -> "Mine", still custom:cellar-copy
copy Cellar again                   -> "Cellar copy", custom:cellar-copy-2
```

  Like `library.toml`, it's read leniently, so a theme file never stops
  the app:

  - An unknown key is ignored.
  - `based_on` must be a built-in id. A copy of a custom theme keeps that
    theme's `based_on`. A missing or unknown one means the default for the
    kind, Midnight or Daylight.
  - A missing or unknown `kind` means dark.
  - A missing or bad color falls back to `based_on`'s.
  - A file that doesn't parse is left out of the lists, and anything
    pointing at it falls back as if it were gone. `validate` warns about
    it.

  `Paths::themes_dir()` resolves the folder, so a portable folder carries
  its themes.

### Stored settings

`library.toml` gains an `[appearance]` table (answer 10), written field by
field like `[view]`:

```toml
[appearance]
follow_system = true
dark = "midnight"
light = "daylight"
theme = "midnight"            # used when follow_system is false
improve_readability = false
```

A missing table means the defaults. An unknown id, or a custom theme whose
file is gone, falls back to the default for that slot; `validate` reports
it as a warning. Like a bad `[view]` value, the warning has no fix: the
app falls back on its own, and the next Appearance save writes a good
value. A wrong kind in a slot (a light theme in `dark`) is treated the
same way. `check.rs` inspects the table and
`repair.rs` keeps its readable lines, as for `[view]`.

### Notices

`crates/wineo-gui/THIRD_PARTY.md` gains entries for the borrowed colors,
each with its source, the colors used, and its MIT text:

- Dracula, `Copyright (c) 2020 Dracula Theme`, from `dracula/spec`.
- Catppuccin, `Copyright (c) 2021 Catppuccin`, from `catppuccin/palette`.
- Ember, `Copyright (c) 2026 Hossam Saraya`, from `ember-theme/ember`, for
  the two coral values.

The `PKGBUILD` already installs the file.

## Implementation sequence

0. **Palette groundwork.** `Preset`, the built-in table, and the resolve
   step, with Midnight and Daylight as the only reachable presets so the
   app looks the same apart from Midnight's danger fix. Audit the literal
   colors against the allowlist, route stragglers through roles, lower the
   disabled accent label bar to 3:1, and widen the palette-iterating tests
   to every preset.
1. **Core storage.** `[appearance]` in `library.toml`: reading, the field
   setters, `check.rs`, and `repair.rs`. `Paths::themes_dir()` and custom
   theme files: id assignment, list, read, write, rename, delete, and the
   `validate` warning for a file that doesn't parse. Tests for older files
   without the table, unknown keys, bad values, and repair.
2. **Improve readability.** The transform and its tests, including the
   pinned output for Dracula and Catppuccin Latte.
3. **Settings tabs.** General and Appearance on the global page, with
   General holding today's sections unchanged.
4. **Appearance tab.** Follow the system, the theme lists, Improve
   readability, live preview, and Save, Revert, and Discard across both
   tabs.
5. **Custom themes.** Copy, the role fields, rename, and delete.
6. **Notices and docs.** `THIRD_PARTY.md`; `gui-style.md` (the preset table
   replaces the two-column palette table, the fixed-color allowlist, and the
   4.5:1 rule with the borrowed-theme exception); `gui.md` (the Appearance
   tab); `config-files.md` (`[appearance]` and theme files).

Steps 1 and 2 can run in parallel after step 0; 3 can start any time.

## Testing and acceptance

- Every wineo preset and Macchiato pass 4.5:1 for each text role on both
  grounds; with Improve readability on, every preset does, and so does
  every custom theme in a generated sample where passing is possible.
  Where it isn't, no role ends lower than it started.
- The transform leaves a passing palette unchanged and is idempotent; its
  output for Dracula and Latte is pinned.
- Hover, press, disabled, and focus stay distinct in every preset, with and
  without readability. Accent labels reach 4.5:1 active, hovered, and
  pressed, and 3:1 disabled, in every preset.
- Layout and poster tests that switch modes run in at least one light and
  one dark borrowed preset.
- Core: a `library.toml` without `[appearance]` reads as the defaults;
  unknown keys and ids are tolerated; a broken file refuses writes and the
  GUI reports it; repair keeps `[appearance]`; a bad slot warns without a
  fix. Theme files round-trip; ids get `-2` on a clash; rename keeps the
  id; a bad color, `based_on`, or `kind` falls back
  as described; a file that doesn't parse is skipped with a warning.
- Settings: preview, Save, Revert, and Discard restore the right palette; a
  `ThemeChanged` mid-preview uses the previewed lists; Save writes in the
  stated order, and a failure at each point (a theme file, a refused
  `library.toml`, a delete) keeps what landed, keeps the rest as edits, and
  never leaves `library.toml` pointing at a deleted file.
- An acceptance checklist for a real window is added when the
  implementation lands.

## Decisions during implementation

- **Steps 0 and 1 ran in parallel** (2026-10-28), since step 1 touches only
  `wineo-core` and step 0 only `wineo-gui`. Step 2 started from step 0's
  branch.
- **Step 0 routed no colors.** Every literal outside `theme.rs` is over art,
  a scrim, a black shadow, an accent blend, or a test fixture.
- **Disabled secondary buttons keep their fill.** `secondary_style` paints
  disabled and active alike and marks disabled with a muted label, so the
  distinct-states test checks active, hovered, and pressed fills plus the
  disabled label for secondary buttons. Changing the fill would change
  Midnight and Daylight.
- **The hero's close button stays literal,** white on black, though it sits
  over a plain `surface` block when there's no hero image. Routing it would
  change today's look.
- **Preset descriptions are placeholders** until the maintainer reviews them
  before step 4.

## Scope limits

- No fonts, text size, or reduced-motion toggle; those stay in the "GUI
  accessibility settings" row, which loses its high-contrast idea to this
  plan.
- No per-environment themes, no theme for the CLI.
- No light High contrast, no Ember Soft, no Dracula or Catppuccin flavors
  beyond the two listed.
- No importing other apps' theme files.
- Colors over art stay fixed (answer 1); Improve readability doesn't touch
  borders.
