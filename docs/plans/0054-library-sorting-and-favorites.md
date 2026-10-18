Status: approved

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
   toggle on the grid, not a separate view. Part 2 also brings per-card
   motion, so cards animate to new positions on a zoom, a re-sort, or a
   filter change (0054 snaps them); it amends the "titles beneath
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

On 2026-10-17 the maintainer answered the questions below (the plan is not
approved yet):

- **1: C.** Favorites can lead the current sort through a "Favorites
  first" toggle, off by default. Sections stay for part 4.
- **2: A.** Hidden games leave the grid and search unless "Show hidden" is
  on.
- **3: as proposed, with playtime's ends swapped.** Most played puts
  never-played games at the bottom; least played puts them at the top.
  Name sorts by the displayed title. Tags are a filter now and grouping in
  part 4.
- **4: A.** `added` is recorded from now on; older environments are
  unknown.
- **5: store and integration are separate internally.** See "Sources".
- **6: A, but Show hidden resets too.** Only the sort, its direction, and
  "Favorites first" are remembered.
- **7: as proposed.**
- **8: as recommended.** Clones start fresh; portable copies carry the
  entry both ways.
- **9: A.** GUI only, with the core API built now.
- **10: parts 1 and 2 gate 1.0**; parts 3 to 5 can land in 1.x.
- **Empty results must explain themselves** (2026-10-17): a filter that
  matches nothing, or a library where everything is hidden, must never look
  like an empty or deleted library. See "Empty states".
- **11: A**, neutral wording. **12: A**, store else platform.
- **13: A, narrowly.** An existing bottleneck the benchmark finds is fixed
  here only if the fix is small, isolated, and measurably faster;
  otherwise it gets its own plan. Stamp caches document and test their
  blind spot (see "Performance").
- **A broken `library.toml` is never silently replaced** (2026-10-17).
  Keep the original, refuse writes with a useful error, and back it up
  before any repair.
- **14: A, with field-level recovery and honest reporting.** A bad value
  falls back to its default for that field only; the entry's other fields
  stay usable. An explicit repair salvages field by field and says exactly
  how much it recovered, since not every corruption can be undone. See
  "Corruption and recovery".
- **Performance gets one manual check on real hardware** (2026-10-17), not
  only Xvfb with software rendering.
- **The 2 s rescan avoids unnecessary work** (2026-10-17): parsing
  `library.toml`, sorting, and redrawing do no work when nothing changed,
  and the plan measures a library of a few hundred games with art. Some
  overhead from the new features is expected, so the bar is no material
  regression outside normal measurement variance: the numbers are
  recorded, meaningful regressions are investigated, and the app stays
  responsive. See "Performance".
- **The cache is never stale after a failed write** (2026-10-17).

A cross-family review on 2026-10-17 raised points the maintainer settled
the same day:

- **Lifecycle writes never block on `library.toml`, with safeguards.**
  `new`, imports, `rm`, `restore`, and `clone` succeed even when the
  library update can't be written; they warn, naming what wasn't saved.
  A reused name clears its old entry whenever `library.toml` is readable.
  See "Lifecycle".
- **Re-sorts and filter changes snap in this plan**, like search does
  today. Animating cards to new positions comes with part 2, which needs
  the same per-card motion for zoom.
- **Portable copies carry the entry into the portable folder only.**
  wineo has no copy back out of a portable folder (`portable.rs:102`);
  that's a backlog row, and amends question 8's "both ways".
- **Unknown playtime sorts last in both directions**, like an unknown
  added date. Steam playtime that can't be read is unknown, not never
  played.
- **Hiding the selected card selects the one that takes its place.** See
  "Selection".
- **An open window keeps its own view.** Another window saving a sort
  doesn't change an open one, and favoriting or hiding in a window never
  writes that window's view back: every write sets only the field it
  changes. Banners for a broken file do update.

A second review round on 2026-10-17 narrowed to the pending-clears journal
for reused names (crash ordering, portable folders, reads and trash
capture, growth). The maintainer settled it the same day:

- **No journal; the stale-entry case is a known limit.** Keeping a
  reused name clean when `library.toml` was unreadable at every chance to
  clean it would need a small transaction system to protect a star or a
  hidden flag. The normal case is covered by clearing on reuse, and the
  rare chain left over is bounded and tested. See "Lifecycle".
- **A cleanup error is never discarded.** Every failed library write
  during a lifecycle operation surfaces in its warning with the error
  itself, never only a generic note, and nothing swallows it.
- **Enter acts only on a shown card.** Enter launches (or opens the panel
  for) the selected game only while it's shown in the filtered grid. A
  search or filter that hides it keeps the selection and the open panel,
  but Enter does nothing until it's shown again. See "Selection".

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
- **Motion.** Grid motion only moves cards between the panel-closed and
  panel-open layouts, by slot index (`grid_motion.rs:365`); `retarget`
  does nothing when the panel state hasn't changed (`grid_motion.rs:93`),
  and a search change snaps (`app_grid.rs:61`). So a re-sort can snap
  safely today, but animating cards to new positions needs per-card motion
  that doesn't exist yet. Only the collapse honors reduced motion
  (`main.rs:1114`).
- **Selection.** A search that excludes the selected card keeps it
  selected with the panel open, and Enter can still launch it
  (`keys.rs:201`).
- **Unknown playtime.** Steam playtime that can't be read is `seconds = 0`
  with `playtime_unknown = true` (`stats.rs:704`), shown as "unknown"
  (`library.rs:276`), which is different from never played.
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

Every question is answered (see "Settled by the maintainer"); their text
stays for the record.

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

### 11. Empty-state wording

wineo is for regular software too, so generic copy stays neutral ("Add
something"). The proposed states, in the same grouped surface as the
maintainer's mockup:

```
No matches:     (search icon)
                Nothing matches your filters
                Try another search or clear your filters.
                [ Clear filters ]

All hidden:     (eye icon)
                Everything here is hidden
                3 hidden. Show them to unhide any.
                [ Show hidden ]
```

**A. Neutral, as above (recommended).** **B. "No games match your
filters"**, as in the mockup, accepting that it reads oddly for a library
of apps.

### 12. What "Source" shows

Internally each environment gets a **store** (Steam, GOG, Epic, Amazon, or
none) and an **integration** (how wineo knows it: Steam library, GOG
install, Heroic import, or plain), plus whether it's native or Windows.

**A. Store, else platform (recommended).** The Source sort and filter use
the store when there is one and the platform otherwise:

```
hades          Steam      (store; integration: Steam library)
papers-please  GOG        (store; integration: GOG install)
a Heroic Epic import  Epic   (store; integration: Heroic import)
balatro        Native     (no store)
notepad-plus-plus  Windows  (no store)
```

The integration stays internal for now, for part 4's grouping and any
future badge.

**B. Two filters**, Store and "Added through", each with its own sort key.

Recommendation: **A**. One "Source" reads as where it came from, which is
what the old list (GOG, Steam, native, Windows) meant; B doubles the
controls for a distinction most people won't need yet.

### 13. Existing rescan costs the benchmark finds

The library read already loads every environment's config and its whole
stats log every 2 s. At a few hundred games with long play histories that
may dominate before this plan adds anything.

**A. Fix what the benchmark shows dominating, in this plan (recommended).**
The likely fix is the same stamp cache for stats logs (re-read a log only
when its mtime or size changes), as its own step with before and after
numbers.

**B. Measure only**, and file anything found as a separate patch or plan.

Recommendation: **A**, since you asked for the measurement in this plan and
a fix found by it is easiest to verify here.

### 14. A broken `library.toml`

Settled in principle: never silently replace it. The details, using the
same scenario throughout: `library.toml` holds 40 favorites, and a hand
edit leaves it broken. You then star hades.

**A. Two kinds of broken (recommended).**

- **The file doesn't parse** (a TOML syntax error). Reads degrade: the
  library shows and launches work, but with no stars or hidden flags, and
  a banner says `library.toml can't be read (line 12: expected "=");
  favorites and hidden games are off until it's fixed`. Writes refuse:
  starring hades says `Can't save to library.toml: it can't be read. Fix
  it, or repair it in Checkup.` Repair is explicit, through `wineo
  validate --fix` or Checkup's fix: it copies the file to
  `library.toml.broken-<time>` first, then writes a fresh one, carrying
  over every entry it can still read.
- **The file parses but an entry is wrong** (`favorite = "yes"`). Only
  that entry is skipped, with a `validate` warning; everything else works,
  and starring hades writes normally. `toml_edit` changes only the key
  being set, so the bad entry stays in the file untouched for the user to
  fix.

```
Syntax error, star hades:  refused; 40 favorites still in the file
Bad entry, star hades:     saved; 39 favorites shown, 1 skipped and kept
Repair:                    library.toml.broken-2026-10-17T19-20 kept,
                           fresh file with the 39 readable favorites
```

**B. Refuse writes for any problem**, including a single bad entry. Safer
on paper, but one typo turns off starring for the whole library.

Recommendation: **A**. A syntax error means wineo can't know what the file
holds, so it mustn't write; a bad entry is a known, contained problem.

## Proposed design

Follows the answers to every question.

### `library.toml`

`Paths::library_file()` is `<config>/library.toml`, so it's backed up with
the configs and travels with a portable folder.

```toml
schema_version = 1

[view]
sort = "last-played"
descending = true
favorites_first = false

[environments.hades]
favorite = true
added = 2026-10-17T18:00:00Z

[environments.old-demo]
hidden = true
```

- Lenient like 0053's internal files: unknown keys and unknown sort names
  are ignored. Bad values and a file that doesn't parse follow
  "Corruption and recovery": nothing is overwritten without a backup, and
  nothing that launches is refused. `validate` reports unknown keys,
  unknown sort names, bad fields, and entries for environments that don't
  exist, with `--fix` removing stale entries and repairing a broken file.
- **Every write is one field.** Starring hades reads the file fresh,
  changes `environments.hades.favorite`, and writes it back; it never
  writes a whole struct, so it can't carry a window's cached `[view]` or
  another environment's stale values with it. The `[view]` table is only
  written by the sort control, and only the field that changed.
- **Writes hold the config lock across the whole read, edit, and atomic
  replace** (`operation.rs:597`), not just the rename, so two writers (the
  GUI and the CLI, or two windows) can't lose each other's edit. The cached
  copy and its stamp come from the same version of the file. Reads and
  no-op edits never rewrite the file (AGENTS.md).
- Lifecycle operations keep entries in step as in "Lifecycle". `clone`
  starts fresh, and a portable copy carries the entry into the portable
  folder's `library.toml` (there's no copy back out).
- The `[environments.<name>]` table leaves room for completion status and
  notes (their backlog row), which fit the same "how you see it" rule.

### Corruption and recovery

**Fields are read one at a time.** Each `[environments.<name>]` table is
read field by field from the `toml_edit` document, not deserialized as one
struct, so a bad value costs only its own field:

```toml
[environments.hades]
favorite = "yes"                 # bad: hades reads as not a favorite
added = 2026-10-17T18:00:00Z     # still used
hidden = true                    # still used
```

The bad field falls back to its default (`favorite = false`), `validate`
reports it (`library.toml: environments.hades.favorite: expected true or
false, found "yes"`), and the value stays in the file untouched. Starring
hades replaces just that value, which is the user's intent. The `[view]`
table works the same way: a bad `sort` falls back to Name and keeps
`descending` as written.

**A file that doesn't parse** degrades reads and refuses writes, as in
question 14: the library shows and launches work with no stars or hidden
flags, a banner names the error and line, and a write says `Can't save to
library.toml: it can't be read. Fix it, or repair it in Checkup.`

**Repair is explicit** (`wineo validate --fix` or Checkup's fix) and works
in this order:

1. Copy the file to `library.toml.broken-<time>`, with the time to the
   second. The copy is created exclusively, so it never replaces an
   existing file: on a clash it tries `-2`, `-3`, and so on. If the copy
   fails, stop and change nothing.
2. Salvage field by field. Each `[environments.<name>]` header the repair
   can find counts as a recognized entry. For each, a field is kept only
   if its line parses on its own **and** its value is valid for that field
   (`favorite = "yes"` parses but isn't kept). `[view]` is salvaged the same
   way. Text it can't attribute to an entry is dropped (it's in the
   backup).
3. Write the salvaged file and report exactly what came back. An entry is
   **fully** recovered when every field the repair recognized in it was
   kept, and **partly** recovered when some were. Fields too damaged to
   recognize can't be counted, which is why the wording says
   "recognized":

```
Recovered 38 of 40 recognized entries fully and 2 partly. Some data could
not be recovered. Sort settings recovered.
Original saved as library.toml.broken-2026-10-17T19-20-05.
```

When everything comes back, it says `Recovered all 40 recognized entries.
Original saved as ...`. It never claims more than it counted.

The fix needs more than today's validate fix API, which returns only
`Vec<Note>` with `Note::Warning` (`validate.rs:1666`, `354`). The repair
returns a structured report (counts, whether `[view]` came back, the
backup path) that the CLI's `validate --fix` and Checkup both render.

### Lifecycle

`library.toml` is secondary to the environments it describes, so its
updates never block or undo a lifecycle operation. Each one warns when its
library update can't be written, naming what wasn't saved:

- **`new`, imports, GOG installs, `clone`:** after the environment
  exists, one write **replaces any existing entry for the name** with a
  fresh one stamped with `added`, so a new `hades` never inherits an old
  one's star or hidden flag, even if an earlier cleanup failed. If the
  file is broken, the environment is still created and the warning says
  what happened and what it means: `Can't update library.toml: <error>.
  hades may show an old star or hidden flag once it's repaired.`
- **`rm`:** the entry is copied into the trash manifest when the manifest
  is written, before payloads move (`trash/remove.rs:304`), so it's part
  of the journaled removal. Removing it from `library.toml` happens after
  the removal commits; if that write fails, the warning carries the
  error, the stale entry is harmless meanwhile (it names a missing
  environment), and `validate` reports it. The next `new` of that name
  clears it.
- **`restore`:** writes the entry back to `library.toml` **before** the
  trash entry is retired (`trash/restore.rs:250`). If that write fails,
  the restore still finishes, and the warning lists the values that
  weren't restored (`favorite, hidden, added 2026-10-17`) so they can be
  set again.
- **Pruning** drops the manifest copy with the trash entry.

**Known limit.** If `library.toml` can't be read both when `rm` tries to
remove an entry and when `new` reuses the name, the old entry survives,
and once the file is repaired the new environment shows its star or
hidden flag. Until then, removing the new one copies that stale entry
into its trash manifest. It takes three rare events in a row, nothing is
lost, both operations warned at the time, and Show hidden makes a stray
hidden flag easy to find and undo. Nothing in the environment can tell an
old entry from its own, so closing this would take a journal with its own
crash and portability rules; the 2026-10-17 review showed that costs more
than it protects.

Tests: each operation with a broken `library.toml` (it succeeds, warns
with the underlying error, and leaves the file untouched); a failed
cleanup after `rm` followed by `new` with the same name while the file is
readable (the stale entry is replaced); the known-limit chain (failed
cleanup, reuse while broken, repair), asserting the warning text and the
documented outcome, so it can't silently get worse; restore with a
failing library write (the warning names the lost values).

### Sort keys

A `SortKey` enum in core with a stable string name per key (`name`,
`last-played`, `playtime`, `added`, `source`) and a comparator over a
small `SortFields` struct the GUI fills from each card. An unknown name in
the file falls back to Name, keeping `descending` as written. A metadata
key later is a variant, a name, and a match arm.

Missing values follow question 3's answer: a never-played game counts as
the least played and the least recently played, so it's at the bottom of
Most played and Recently played and at the top of their reverses.
Unknown values sort after every known one in both directions, since
unknown isn't oldest or least: an unknown `added` date, and Steam
playtime that couldn't be read (`playtime_unknown`). Ties fall back to
Name, ascending.

### Sources

`Source` in core has `store: Option<Store>` (`Steam`, `Gog`, `Epic`,
`Amazon`), `integration` (`SteamLibrary`, `GogInstall`, `HeroicImport`,
`Plain`), and `native: bool`. The label follows question 12. Precedence,
first match wins:

1. `CardKind::Proxied`: store Steam, integration Steam library.
2. A `gog_id`: store GOG. Integration is Heroic import if there's also a
   `heroic_id`, otherwise GOG install.
3. A `heroic_id`: integration Heroic import. The store comes from the
   `store` hint, which only Wine configs have (`config.rs:330`): `egs` is
   Epic, `gog` is GOG, `amazon` is Amazon, and anything else or none means
   no store. A native Heroic import has no hint, so no store.
4. Otherwise: integration plain, no store.

`native` comes from `CardKind::Native` in every case, so a store-less
entry's label is Native or Windows.

### Empty states

The grid area shows one of these instead of a blank page, in a grouped
surface like the maintainer's mockup, with the wording from question 11:

- **Library empty** (no environments at all): today's "Your library is
  empty" and **Add something**, unchanged.
- **Everything hidden** (environments exist, every one hidden, Show hidden
  off, no search or filters): the hidden count and **Show hidden**.
- **No matches** (search or filters exclude everything shown): **Clear
  filters**, which clears search, source and tag filters, and leaves the
  sort alone. If hidden games would match, the text says so ("2 hidden
  games match") with **Show hidden** beside it.

Each state is chosen from counts the filtering already has, so it costs
nothing extra. With nothing shown, the arrow keys do nothing and Enter
does nothing, since no card is shown; the selection follows "Selection".

### Selection

- **A re-sort or filter change** keeps the selected card selected and
  scrolls it into view if it's still shown. Search keeps today's behavior.
- **Hiding the selected card** (with Show hidden off) selects the card
  that takes its place: the one now at the same position in the shown
  order. If the hidden card was last, the new last card is selected. If
  nothing is left, the selection is cleared and the panel closes. With
  Show hidden on, the card stays shown dimmed and stays selected.
- **Turning Show hidden off, or a filter change, that removes the selected
  card** keeps it selected with its panel open, like search does today.
  The panel still shows which game it is, so its Play button works.
- **Enter acts only on a shown card.** Today Enter opens the selected
  card's panel, or launches it when the panel is open, even when search
  hides the card (`keys.rs:202`). Now it does nothing while the selected
  card isn't in the filtered grid, whatever hid it. The arrow keys move
  from there to the first shown card, as they do today.
- **Arrows on an empty grid keep the selection.** Today a move with
  nothing shown saves `move_index`'s `None` as the selection
  (`keys.rs:191`, `keys.rs:246`), which would drop a selection the rules
  above keep. With nothing shown, a move changes nothing.

### Performance

The rescan runs every 2 s while the window is focused. A quiet rescan,
where nothing changed, should do no work this plan adds; when things do
change, the new features cost some time, and the bar is no material
regression outside normal measurement variance:

- **`library.toml` is read only when its stamp changes.** The rescan
  keeps the parsed file with its stamp and re-reads it only when that
  differs. A successful write updates the cached copy and its stamp
  together. A failed write (refused, disk full, lock held) never updates
  the cache with the value it meant to write: the cache is dropped and the
  next read loads what's really on disk, so the app can't show a star
  that wasn't saved. Tests force each failure and check the card.
- **The stamp is mtime, ctime, size, and inode.** mtime and size alone
  (what the Art page's snapshot uses) miss a file replaced with same-sized
  content and a matching mtime, which `touch -d` or a copy that preserves
  times can produce. ctime changes on every write and can't be set by
  hand, and a replace by rename changes the inode, which closes that in
  practice. The remaining blind spot is a filesystem with coarse
  timestamps (FAT on a portable drive keeps 2 s), where two same-sized
  writes in one tick look alike. That's documented in the code beside the
  stamp and in `config-files.md`, and the next real change, or a restart,
  catches up. Tests cover a changed size, a changed mtime, a same-size
  rewrite (caught by ctime), a rename over the file (caught by inode), and
  the documented coarse-tick case.
- **Sorting and filtering happen once per change, not per frame.**
  `filtered()` is called from the grid's `view`, which runs every frame
  during card motion, and from 14 other places. It becomes a cached list
  of card indices, rebuilt only when the cards, search, sort, or filters
  change, and every caller reads the cache. Sort keys (lowercased titles,
  timestamps) are computed once per rebuild, not per comparison.
- **No redraw when the order didn't change.** The rescan sends a message
  only when its result differs (`rescan.rs:145`). Its payload grows from
  `Result<Vec<Card>, String>` (`rescan.rs:176`) to include the library
  file's diagnostics (a parse error banner appearing or clearing), so
  those are noticed too. The saved `[view]` isn't part of it: an open
  window keeps its own view. The new card fields are stable values, so
  they don't add spurious differences.
- **Measured, not assumed.** A generated library of 300 environments, each
  with poster art and a stats log of a few hundred sessions, measured on
  0.35.1 and after each step: rescan time, idle CPU while focused (Xvfb,
  software rendering, as in 0051), the time to apply a sort or filter
  change, and scroll smoothness. Two rescans are timed separately: a
  **quiet** one, where nothing changed, and a **busy** one, where all 300
  environments genuinely changed (new sessions in every stats log, a
  `library.toml` edit touching every entry). Each measurement is repeated
  enough to know its normal spread. The numbers go in this plan after each
  step; a regression beyond that spread gets investigated before the next
  step, and either explained (a cost the feature needs, kept as small as
  practical) or fixed. Responsiveness is the bar: sorting, filtering, and
  starring feel instant, and scrolling stays smooth.
- **One manual check on real hardware.** The same 300-game library in the
  maintainer's normal session, with the GPU renderer: scroll the grid,
  change the sort, toggle filters, and leave it focused for a minute while
  watching CPU. Software rendering in Xvfb shows relative costs; this shows
  what a person actually feels.
- **An existing cost the baseline finds** (the rescan reads every stats
  log in full each time) is fixed here only under question 13's rule:
  small, isolated, and measurably faster. The likely fix, the same stamp
  cache for stats logs, qualifies if it stays inside the stats read. Any
  fix that reaches further gets its own plan.

### Cards

`Card` gains raw `seconds`, `last_played`, `added`, `favorite`, `hidden`,
and `source`, filled in `load_library` from `stats::Summary`, the loaded
`Environment`, and `library.toml` (read once per rescan). The formatted
strings stay for display. Since every field joins the rescan diff, the
values are stable ones, not "today" labels.

### Filtering and order

`filtered()` becomes: drop hidden unless Show hidden; keep sources and tags
that match the active filters; match search against title, identifier,
and tags; then sort, with favorites first when that toggle is on. It's
still the single hook, now backed by the cache in "Performance", so keys
and art loading follow. A change of sort or filter snaps the grid, like a
search change does today (`app_grid.rs:61`); animating cards to their new
places is part 2. Selection follows "Selection".

## Implementation sequence

0. **Benchmark baseline.** A script that generates a 300-environment
   library with art and stats logs under a throwaway root, and the
   measurements from "Performance" taken on 0.35.1.
1. **Core `library.toml`.** `Paths::library_file()`, the lenient types,
   load and `toml_edit` writes, the core API (set favorite, set hidden, set
   view, stamp added), one-field writes under the config lock, the
   "Lifecycle" rules for `new`, imports, GOG installs, `clone`, `rm`,
   `restore`, and pruning, carrying the entry into portable copies, and
   `validate` checks with the structured repair report. Upgrade fixtures: a missing file and an older
   `library.toml` load.
2. **Card fields and source.** Raw sort fields and `source` on `Card`, from
   the summary, config, and library file, with the `library.toml` stamp
   cache. Rescan diff tests. If the baseline shows the stats logs
   dominating and question 13's rule holds, their stamp cache lands here
   as its own commit with before and after numbers.
3. **Sort.** `SortKey`, the comparator, the cached order behind
   `filtered()`, the "Favorites first" toggle, and the sort control with
   direction. Persist through `[view]`.
4. **Filters, search, and empty states.** Source and tag filters, Show
   hidden with its count, tag matching in search, the filter popup, the
   three empty states, and the "Selection" rules.
5. **Favorite and hide actions.** Context menu rows, the panel star, the
   `F` key, the poster star mark, and the dimmed hidden treatment.
6. **Benchmark again** on the finished work against step 0's numbers.
7. **Docs.** `gui.md`, `config-files.md` (its `library.toml` placeholder at
   323-326 becomes the real schema), `features.md` rows, and `gui-style.md`
   for the toolbar and the star and hidden marks.

## Testing and acceptance

- Core: `library.toml` round-trips, ignores unknown keys, keeps a newer
  release's keys through an edit, never rewrites on read, survives `rm`
  and `restore`, and `validate` reports stale entries. Two writers at once
  (two threads through the core API) both land. A favorite written while
  another process changed `[view]` keeps that `[view]`. The "Lifecycle"
  tests, including the known-limit chain. A portable copy carries the
  entry into the folder's `library.toml` and leaves the folder's other
  entries and its `[view]` as they were.
- GUI: `filtered()` order for every key and direction with never-played
  and unknown-date games, favorites first, hidden with and without the
  switch, source and tag filters, tag search, unknown Steam playtime last
  in both directions; the selection survives a re-sort; hiding the
  selected card selects the one in its place, the previous one when it
  was last, and nothing when it was the only one; a search, filter, or
  Show hidden change that hides the selected card keeps it selected and
  Enter does nothing until it's shown again; with every card filtered
  out, each arrow key leaves the selection and panel unchanged; a re-sort
  snaps without
  stale bounds; keys move through the sorted order; each empty state appears for its case and
  never for a populated grid; the cached order rebuilds on each kind of
  change and not on an unchanged rescan; `library.toml` isn't re-read when
  its stamp is unchanged. Driven with `tests::app()` and fake ticks, like
  the existing grid tests.
- Performance: the step 0 measurements before and after, recorded in this
  plan, plus the real-hardware check in the acceptance checklist.
- Corruption: a file that doesn't parse refuses writes with the error
  and is left byte-for-byte unchanged; a bad field falls back to its
  default while the same entry's other fields still apply (the
  `favorite = "yes"` example), and survives an unrelated write; repair
  writes the backup before the new file, a failed backup aborts it, two
  repairs in the same second keep both backups, a parseable but invalid
  value isn't salvaged, and the report's full and partial counts match
  what was salvaged, for a fully and a partly recoverable file.
- Cache: after each kind of failed write, the card and the cache match
  the file on disk.
- Acceptance: a checklist the maintainer runs in a real window on a
  throwaway library, written when the steps land.

## Scope limits

- No zoom, poster-only, list view, presets, grouping, sections, coverflow,
  or gamepad: parts 2 to 5.
- No install-size sort: Wine prefixes would need a tree walk the 2 s
  rescan can't afford.
- No metadata sorts until there's metadata.
- No CLI surface (question 9).
- No animation for a re-sort or filter change; they snap (part 2).
- No copy of a portable environment back out of its folder, so no
  carrying its entry home; a backlog row.
- A reused name can show an old star or hidden flag when `library.toml`
  was unreadable through both its removal and its reuse; the known limit
  in "Lifecycle".
- Reduced motion still only covers the collapse; the grid and panel ignore
  it today. A separate fix if wanted.
