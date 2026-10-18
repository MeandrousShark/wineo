Status: draft

# 0054: Library sorting, filters, and favorites

## Goal

Let the desktop app's library be browsed the way that fits the moment: sort
it by name, playtime, last played, date added, or source, in either
direction; narrow it by source and tag; star favorites; and hide what you
don't want to see. The choice of sort is remembered across launches.

This is part 1 of the library views work in
[features.md](../features.md) (priority 1). It also builds `library.toml`,
the lenient file [0053](0053-config-contract.md) question 8 decided library
state lives in, so the later parts (zoom, presets) have somewhere to save.

## Series

The maintainer split library views into parts on 2026-10-17. Each later
part gets its own numbered plan when its turn comes; this section only
keeps their ideas in one place.

1. **Sorting, filtering, favorites, and hidden games** (this plan).
2. **Grid zoom and poster-only mode.** A zoom slider, remembered across
   launches, relaunches, and reinstalls that keep the configs (it lives in
   `library.toml`'s `[view]` table). `Ctrl+=`, `Ctrl+-`, and `Ctrl` with
   the mouse wheel change it. The grid animates smoothly between sizes and
   keeps the selected game in view. Poster-only (titles hidden until hover
   or keyboard focus, then shown wrapped over the poster, like Heroic) is a
   toggle on the grid, not a separate view; it amends the "titles beneath
   the art" rule in `gui-style.md` on purpose, and art-less environments
   need their text tiles to stay readable. Grid thumbnails decode at most
   320 x 480 (`artwork.rs:59`), so zoom past about 2x needs bigger decodes.
3. **List view.** An alternative to the grid, with the same sort and
   filters.
4. **Saved filter presets and grouping.** Named presets of sort and
   filters. Grouping by tag, source, or state, which is where sections like
   "Recently played" or a hidden-games section would come from.
5. **Coverflow with controller navigation**, possibly as a whole
   fullscreen mode like Steam's Big Picture (the "Controller fullscreen
   mode" row). iced 0.14 has no gamepad events and the app has no
   fullscreen handling today, so this needs a gamepad crate and a plan of
   its own.

Sorting by metadata (genre, release date, developer) waits for a metadata
feature, but this plan's sort keys are built so adding one is one new key
(see "Sort keys").

## Settled by the maintainer

- **Library state lives in a lenient `library.toml`** (0053 question 8,
  2026-10-16). Env configs say how to run something; `library.toml` says
  how you see it. Tags stay in the env configs.
- **The split into five parts** (2026-10-17), as in "Series".
- **Poster-only is a grid toggle**, not a separate view (2026-10-17).
- **Sorts go both ways**, and include playtime and library source
  (2026-10-17).

## Existing foundation

- **Cards.** `Card` (`crates/wineo-gui/src/library.rs:77-100`) is built in
  `load_library` (`library.rs:213-408`) at boot and on every focused 2 s
  rescan (`rescan.rs:19`). It carries `display_title`, `name`, `kind`
  (`Wine`, `Native`, `Proxied`), `gog`, `tags`, and the state. Playtime,
  sessions, and last played are only formatted strings (`library.rs:90-92`,
  built at 240-262); the raw values come from `stats::Summary`
  (`crates/wineo-core/src/stats.rs:210-229`: `seconds`, `last_played`,
  `first_played`, `source`) and are thrown away. The rescan diffs every
  card field (`rescan/tests.rs:85`), so new fields join change detection.
- **Order and search.** The grid sorts by identifier
  (`library.rs:405`), not by the title it shows. `App::filtered()`
  (`main.rs:1152-1161`) matches search case-insensitively against the
  title and identifier, not tags. Every consumer (the grid, keys, art
  loading, panel motion, resize) goes through `filtered()`, so it's the one
  place sort and filters plug in.
- **Source.** Steam is `CardKind::Proxied`. A GOG game has
  `EnvironmentConfig.gog_id` (`config.rs:266`), also on the card. A Heroic
  import has `heroic_id` (`config.rs:263`) and usually a `store` hint
  (`config.rs:339`), neither on the card yet; `load_library` already holds
  the loaded `Environment` (`library.rs:282`), so copying them costs
  nothing. Native is `CardKind::Native`. Everything else is a Wine or
  Proton prefix.
- **Date added.** Nothing records when an environment was created. Configs
  are written by temp file and rename (`files.rs:147`), so their times mean
  "last edited".
- **Motion.** Cards move between old and new slots with a damped spring
  (`grid_motion.rs:19-25`), and `gui-style.md` already requires filtering
  and changed contents to settle or retarget safely, so a re-sort animates
  with what exists. Only the collapse honors reduced motion today
  (`main.rs:1114`).
- **Context menu and panel.** The right-click menu and the docked panel
  (`panel.rs:734`, detail rows at 974-978) are where per-game actions live.
- **No saved GUI state.** Plan [0036](0036-gui-library-launcher.md) chose
  no settings file and no remembered window size (0036:175-179). This plan
  reverses that for library view state only, through core. `wineo-gui` has
  no `toml` or `serde` dependency, so file I/O stays in core.
- **Lenient files.** 0053 made internal files tolerate unknown keys with
  `serde(default)` (trash and snapshot manifests, run state, GOG folder
  state), while user configs keep `deny_unknown_fields`. Edits go through
  `toml_edit`. Trash payload kinds are a strict enum (`trash.rs:132`), so a
  `library.toml` entry rides in a trash entry as an additive manifest field
  (0053:413-416).

## Questions for the maintainer

The scenarios use the same library in every option: **hades** (Steam,
40 h, played yesterday), **papers-please** (GOG, 6 h, played last month),
**notepad-plus-plus** (Wine, never played), **balatro** (native, 20 h,
played today), and **old-demo** (Wine, 1 h, played last year). hades and
balatro are favorites; old-demo is hidden.

### 1. Where favorites go

**A. First within the current sort (recommended).** Favorites lead the
grid in whatever sort is active, then everyone else in the same sort. A
star on the poster marks them; no heading, so no vertical space is spent.

```
Sort: playtime, most first
  ★ hades 40 h   ★ balatro 20 h   papers-please 6 h   notepad-plus-plus
```

**B. A Favorites section above the rest.** A "Favorites" heading and its
own row or rows, then an "All games" heading with everything else
(favorites again or not).

```
Favorites
  hades 40 h   balatro 20 h
All
  papers-please 6 h   notepad-plus-plus
```

**C. A toggle between A and plain sorting.** "Favorites first" on or off
in the sort menu; off, favorites sort like everything else and only the
star shows.

Recommendation: **A**, with C's toggle if you want the plain order
sometimes. Sections are grouping, part 4, and a "Recently played" section
fits there too; until then, sorting by last played gives the same view.

### 2. Where hidden games go

**A. Gone unless "Show hidden" is on (recommended).** Hidden games leave
the grid and search. A "Show hidden" switch in the filters brings them
back dimmed, with a hidden mark, so they can be unhidden.

```
Default:      hades  balatro  papers-please  notepad-plus-plus
Show hidden:  hades  balatro  papers-please  notepad-plus-plus  old-demo (dimmed)
```

**B. A Hidden section at the bottom, collapsed.** Always one "Hidden (1)"
heading below the grid that expands.

Recommendation: **A**. It needs no sections, which wait for part 4, and a
count beside the switch ("Show hidden (1)") keeps them findable. Search
skipping hidden games matches what hiding is for; with the switch on,
search finds them.

### 3. Sort keys

Proposed: **Name**, **Last played**, **Playtime**, **Date added**, and
**Source**, each ascending or descending. Never-played games sort last for
Last played and Playtime in both directions, so "least played" doesn't
start with every game you've never opened. Ties, and everything within one
source, fall back to Name.

- **Name sorts by the title the card shows**, not the identifier, so
  "The Witcher 3" files under T, not under its identifier. Today the grid
  sorts by identifier (`library.rs:405`), so a library whose titles and
  identifiers differ changes order. Recommended, since search and the card
  both lead with the title. Leading "The" and "A" are not skipped, to keep
  it predictable.
- **"Sort by tags"** reads to me as filtering by tag now and grouping by tag
  in part 4, since a game has many tags and there's no single value to
  order by. Please confirm.

### 4. Date added

Nothing records when an environment was made.

**A. Record it from now on; older ones are unknown (recommended).** `new`,
imports, GOG installs, and `clone` stamp `added` in `library.toml`. Older
environments have no date and sort after every dated one in both
directions, by name.

```
Date added, newest first, right after upgrading:
  hades  balatro  papers-please  notepad-plus-plus   (all unknown, by name)
After adding celeste:
  celeste  hades  balatro  papers-please  notepad-plus-plus
```

**B. Backfill from the first session.** Unknown dates use the first played
time. Never-played games stay unknown, and a game played long after it was
added looks newer than it is.

**C. Backfill with the time the app first sees it.** Every existing game
gets the upgrade date, which looks real but isn't.

Recommendation: **A**. It never shows a date that isn't true, and the sort
becomes useful as the library grows. B is fine as a one-time seed if you'd
rather have something on day one; it would be stored as an estimate, so it
can be told apart later.

### 5. Sources

Proposed labels and order, for the Source sort and filter:

| Source | Which environments |
|---|---|
| GOG | Has a `gog_id` |
| Heroic | Has a `heroic_id` and no `gog_id` |
| Native | `CardKind::Native` |
| Steam | `CardKind::Proxied` |
| Windows | Every other Wine or Proton environment |

Sorted alphabetically by label, so ascending reads as the table and
descending reverses it. The open part is **Heroic**: show "Heroic", or the
store it came from (Epic, Amazon, GOG, sideloaded) from the import's
`store` hint? The hint is user-editable and meant for protonfixes, so it's
a decent label but not a fact. Recommendation: by store, falling back to
"Heroic" when there's no hint, since "source" reads as where the game is
from. A Heroic GOG import with a `gog_id` already counts as GOG.

### 6. What's remembered across launches

**A. Sort, direction, favorites-first, and Show hidden persist; source and
tag filters reset each launch (recommended).**

```
You filter to Steam, quit, and relaunch next week:
  A: the whole library, still sorted by last played
  B: only Steam games, and nothing says why the others are gone
```

**B. Everything persists**, including filters.

Recommendation: **A**. A filter left on is the classic "where did my games
go". Part 4's saved presets are the way to come back to a filter on
purpose. The filter button shows a count while filters are on, either way.

### 7. Controls

A toolbar row beside the existing search box:

```
[ Search...            ] [ Sort: Last played v ] [ ↓ ] [ Filters (2) v ]
```

- **Sort** is a dropdown of the keys; **↓ / ↑** flips the direction; the
  dropdown also holds "Favorites first" if question 1 picks C.
- **Filters** opens a popup with source checkboxes, tag chips (games
  matching any chosen tag), "Show hidden (N)", and "Clear filters".
- **Favorite** and **Hide** live in the card's context menu and as a star
  button in the panel's header. `F` toggles favorite on the selected card
  from the grid; hiding has no single key, since it's easy to do by
  accident and the game disappears.
- **Search also matches tags**, so typing `rpg` finds games tagged `rpg`.

Recommendation: as drawn. Say if you'd rather have the controls in a
collapsible bar or the sidebar.

### 8. Clones and portable copies

- A **clone** gets its own `added` time and starts unstarred and unhidden,
  since it's a new environment (recommended), or copies the source's star
  and hidden flag.
- A **portable copy** (`wineo portable`) carries the entry into the
  portable folder's `library.toml`, and copying back carries it home
  (recommended), or starts fresh.

### 9. The CLI

**A. GUI only, with the API in core (recommended).** Core owns
`library.toml` and exposes favorites, hidden, and added. The CLI doesn't
show or set them yet; a backlog row covers `list` markers, `--json`
fields, and commands like `wineo favorite`.

**B. CLI now too.** `list` marks favorites and skips hidden ones unless
`--all`, `list --json` adds `favorite`, `hidden`, and `added`, and
`wineo favorite NAME` / `wineo hide NAME` set them.

Recommendation: **A**. JSON fields and commands are contract after 1.0, so
they're worth designing once the GUI has settled what these mean. Adding
them later is a compatible change.

### 10. Which parts land before 1.0

The library views row is priority 1, and GUI themes wait on library views
so themes cover every palette role. Do all five parts gate 1.0, or only
some? Recommendation: parts 1 and 2 gate 1.0, since they're what people
meet every day and they settle the palette roles themes need; parts 3 to 5
can be 1.x, since none of them changes a contract.

## Proposed design

Assumes the recommended answers; amended once the questions are settled.

### `library.toml`

`Paths::library_file()` is `<config>/library.toml`, so it's backed up with
the configs and travels with a portable folder.

```toml
schema_version = 1

[view]
sort = "last-played"
descending = true
show_hidden = false

[environments.hades]
favorite = true
added = 2026-10-17T18:00:00Z

[environments.old-demo]
hidden = true
```

- Lenient like 0053's internal files: unknown keys and unknown sort names
  are ignored with `serde(default)`, and a broken file loads as empty with
  a warning rather than refusing anything. `validate` reports unknown keys,
  unknown sort names, and entries for environments that don't exist, with
  `--fix` removing stale entries.
- Writes go through `toml_edit` under the config lock pattern core already
  uses, so a newer release's keys survive an older one's edit. Reads and
  no-op edits never rewrite the file (AGENTS.md).
- `rm` moves the entry into the trash manifest as an additive field;
  `restore` puts it back; pruning drops it. `clone` and portable copies
  follow question 8.
- The `[environments.<name>]` table leaves room for completion status and
  notes (their backlog row), which fit the same "how you see it" rule.

### Sort keys

A `SortKey` enum in core with a stable string name per key (`name`,
`last-played`, `playtime`, `added`, `source`) and a comparator over a
small `SortFields` struct the GUI fills from each card. An unknown name in
the file falls back to the default (Name, ascending). A metadata key later
is a variant, a name, and a match arm.

### Cards

`Card` gains raw `seconds`, `last_played`, `added`, `favorite`, `hidden`,
and `source`, filled in `load_library` from `stats::Summary`, the loaded
`Environment`, and `library.toml` (read once per rescan). The formatted
strings stay for display. Since every field joins the rescan diff, the
values are stable ones, not "today" labels.

### Filtering and order

`filtered()` becomes: drop hidden unless Show hidden; keep sources and tags
that match the active filters; match search against title, identifier,
and tags; then sort with favorites first. It's still the single hook, so
keys, art loading, and motion follow. A change of sort or filter retargets
the grid motion like a filter change does today and keeps the selected
card selected when it's still shown.

## Implementation sequence

1. **Core `library.toml`.** `Paths::library_file()`, the lenient types,
   load and `toml_edit` writes, the core API (set favorite, set hidden, set
   view, stamp added), stamping `added` in `new`, imports, GOG installs,
   and `clone`, the trash manifest field with restore, portable copies, and
   `validate` checks. Upgrade fixtures: a missing file and an older
   `library.toml` load.
2. **Card fields and source.** Raw sort fields and `source` on `Card`, from
   the summary, config, and library file. Rescan diff tests.
3. **Sort.** `SortKey`, the comparator, favorites first, and the sort
   control with direction. Persist through `[view]`.
4. **Filters and search.** Source and tag filters, Show hidden with its
   count, tag matching in search, and the filter popup.
5. **Favorite and hide actions.** Context menu rows, the panel star, the
   `F` key, the poster star mark, and the dimmed hidden treatment.
6. **Docs.** `gui.md`, `config-files.md` (its `library.toml` placeholder at
   323-326 becomes the real schema), `features.md` rows, and `gui-style.md`
   for the toolbar and the star and hidden marks.

## Testing and acceptance

- Core: `library.toml` round-trips, ignores unknown keys, keeps a newer
  release's keys through an edit, never rewrites on read, survives `rm`
  and `restore`, and `validate` reports stale entries.
- GUI: `filtered()` order for every key and direction with never-played
  and unknown-date games, favorites first, hidden with and without the
  switch, source and tag filters, tag search; the selection survives a
  re-sort; the grid motion retargets without stale bounds; keys move
  through the sorted order. Driven with `tests::app()` and fake ticks, like
  the existing grid tests.
- Acceptance: a checklist the maintainer runs in a real window on a
  throwaway library, written when the steps land.

## Scope limits

- No zoom, poster-only, list view, presets, grouping, sections, coverflow,
  or gamepad: parts 2 to 5.
- No install-size sort: Wine prefixes would need a tree walk the 2 s
  rescan can't afford.
- No metadata sorts until there's metadata.
- No CLI surface (question 9).
- Reduced motion still only covers the collapse; the grid and panel ignore
  it today, and a re-sort uses the same motion. A separate fix if wanted.
