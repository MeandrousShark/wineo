Status: draft

# 0040: GUI art and downloads

## Goal

Manage artwork and downloaded builds from `wineo-gui` without the CLI.

Each environment gets an **Art** page that shows its four kinds, sets them
from a file, removes them, and picks from SteamGridDB's results with a
thumbnail picker instead of taking only the top-ranked image. Global
settings gains the SteamGridDB key. A **Downloads** page in the sidebar's
tools group lists installed runners, DXVK, and vkd3d-proton builds, shows
which environments use each, downloads new releases, and deletes unused
builds.

0041 follows with snapshots, portable copies, exec, and winetricks. Earlier
plans named all six areas as 0040; the maintainer split them on 2026-10-01
and put art first.

## Settled by the maintainer

From the 2026-10-01 planning conversation:

- Split the old 0040 scope in two. This plan is art and downloads; 0041 is
  snapshots, portable copies, exec, and winetricks.
- The SteamGridDB picker shows many results, not just the top hit.
- Art lives on a per-environment **Art** page opened from the panel, like
  Settings. The exact layout gets confirmed in a real window during step 3,
  since the ASCII mockup didn't settle it.
- The picker is one kind at a time: a thumbnail grid ranked best first,
  **Load more** for paging, a **Filters** toggle for the images ranking
  leaves out, and a details line with **Use** for the selected image.
- Downloads is a sidebar page in the tools group, between Checkup and the
  Settings divider.
- Downloads can delete installed builds. Deleting one that's in use is
  refused, naming what uses it, and the CLI gets the same operation so the
  two stay at parity.

## Existing foundation

- `art::list`, `set_manual`, `set_manual_bytes`, `remove`, `save_game`, and
  `saved_game` cover the store. `art_fetch::fetch_kind` with
  `Mode::Asset(id)` downloads and publishes one chosen image, which is what
  `wineo art fetch --asset` uses.
- `steamgriddb::Api::search` and `Api::images` list games and images, and
  `rank` orders them. `Filters::Ranked` and `Filters::None` are the two
  request shapes. `Image` doesn't parse `thumb`, `author`, `style`, or the
  NSFW, humor, and animated flags yet.
- `steamgriddb::find_key`, `save_key`, `remove_key`, and `check_key` back
  `wineo art key`.
- 0038 added local art selection with previews, SteamGridDB game search and
  match selection in Add flows, and desktop entry reconciliation after art
  changes. 0.19.1 added the decoded art cache and the two-at-a-time decode
  limit.
- `download::Client::list` pages a provider's releases, and `install`
  reports progress and honors cancel. 0039 shared the GUI side as
  `download_progress.rs`.
- `Runners::list` and `Builds::list` list what's installed. Nothing in core
  or the CLI deletes a build.

## Proposed design

### Core: richer SteamGridDB images

- Parse `thumb`, `author.name`, `style`, `nsfw`, `humor`, and the animated
  type into `Image`. Missing fields stay `None` or false, as today.
- Add `Api::images_page(game, kind, page, filters)` returning one page, so
  the picker can load on demand instead of walking up to `MAX_PAGES` first.
  `images` keeps its behavior for fetch.

### Core and CLI: deleting a build

- `runners::remove(paths, name)` and `components::remove(paths, component,
  name)`, each taking the install lock and moving the directory out with
  the same rename-then-delete pattern trash uses, so an interrupted delete
  never leaves a half build that looks usable.
- A shared `uses(paths, build)` lists what keeps a build: environment
  configs that name it, environments whose alias (`proton`,
  `GE-Proton-Latest`, `Proton-CachyOS-Latest`, `latest`) resolves to it
  today, templates that name it, the global default runner, and active runs
  using it.
- Remove refuses while anything uses it, with an error that names them:
  "GE-Proton9-20 is used by hades and celeste."
- CLI: `wineo runners remove NAME`, `wineo dxvk remove NAME`, and
  `wineo vkd3d remove NAME`, each asking first on a terminal and taking
  `--yes` for scripts.

### Art page

Opened by an **Art** button in the panel and an **Art...** item in the
detail actions and card menus. It replaces the library like an
environment's settings page, with **Back**, and keeps Library highlighted.

```
[< Back]  Hades art            SteamGridDB game: Hades (2020) [Change]
                               Showing: [Environment v]

 Icon        Grid          Hero                     Logo
+------+   +--------+   +-----------------------+   +----------+
| img  |   |  img   |   |         img           |   |   img    |
+------+   |        |   +-----------------------+   +----------+
           +--------+
SteamGridDB  Manual       SteamGridDB               none
[Find...]  [Find...]     [Find...]                 [Find...]
[Browse...] [Browse...]  [Browse...]               [Browse...]
[Remove]   [Remove]      [Remove]
```

- Each kind shows its art, where it came from (SteamGridDB, Manual, or
  none), **Find...** for the picker, **Browse...** for a local file through
  the portal chooser from 0038, and **Remove**.
- The game line shows the saved SteamGridDB game. **Change** opens the same
  search and match list as 0038's Add flow and saves the pick with
  `save_game`. With no saved game, **Find...** asks for the game first.
- **Showing** picks the environment or one of its targets. A target shows
  its overrides, with inherited kinds dimmed and labeled "From the
  environment". Remove on a target override falls back to the environment's
  art.
- Changes apply right away, since there's nothing to batch: no save bar.
  Each one updates the card, panel, and art cache, and reconciles desktop
  entries after an icon change.
- Without a key, **Find...** and **Change** are disabled with a line that
  says to add a key in Settings, with a button that goes there.

### SteamGridDB picker

Opened by **Find...**, as a page over the Art page with **Back**.

```
[< Back]  Grid art for Hades                        [Filters v]
+----+ +----+ +----+ +----+ +----+
| 1  | | 2  | | 3  | | 4  | | 5  |
+----+ +----+ +----+ +----+ +----+
+----+ +----+ +----+ +----+ +----+
| 6  | | 7  | | 8  | | 9  | | 10 |
+----+ +----+ +----+ +----+ +----+
              [Load more]

#3  by someone  600x900  official  score 12        [Use]
```

- Results come in `rank` order from the ranked filters, so the first
  thumbnail is what `art fetch` would pick.
- **Filters** reveals switches for Animated, NSFW, Humor, and, for grids,
  Other sizes. Turning any on reloads from `Filters::None` and keeps rank
  order, hiding the kinds still switched off on the client side. They're
  off on every visit.
- Thumbnails come from `thumb` URLs, load a page at a time, decode through
  the existing two-at-a-time limit, and are cached for the session only.
- Clicking a thumbnail selects it and fills the details line. Double click
  or **Use** downloads the full image through `fetch_kind` with
  `Mode::Asset(id)`, with progress and cancel, then returns to the Art page.
  A full image whose format the kind can't store fails on the details line
  with core's message.
- Animated images show their first frame in the grid, labeled
  "Animated". Whether the library can show one animated is a polish
  question, not this plan's.

### SteamGridDB key in global settings

A **SteamGridDB** section in global settings shows where the key comes from
(the environment variable, the keyring, or the plain text file), with
**Set key...** and **Remove key**.

- **Set key...** opens a masked field, checks the key with `check_key`, then
  saves it with `save_key`. The plain text fallback warning shows on the
  section like the CLI's one line.
- These apply right away, outside the page's Save bar, like a download.
- With `WINEO_STEAMGRIDDB_KEY` set, the section says it overrides the saved
  key.

### Downloads page

A **Downloads** sidebar section in the tools group, after Checkup. Ctrl+4
opens it.

```
Downloads                                  [Download...]
Runners
  GE-Proton9-20     used by hades, celeste
  proton-cachyos    unused                       [Delete]
DXVK
  2.5.3             used by hades (latest)
vkd3d-proton
  2.14              unused                       [Delete]
```

- Each build shows what uses it, using core's `uses`. A build kept by an
  alias says so, like "(latest)". **Delete** shows only on unused builds,
  and asks first with how much space it frees.
- **Download...** picks a provider, then a release from `Client::list` with
  **Load more** for older pages, and installs it with the shared progress
  and cancel. The newest stable release is preselected.
- A download or delete is a running change, like 0039's: it keeps going
  when you switch sections, the sidebar shows a spinner next to Downloads,
  and closing the window waits for it the same way.
- Directories that aren't usable builds show under **Not usable**, with
  core's reason, so a leftover folder isn't invisible.

### Keyboard

| Key | Does |
|---|---|
| Ctrl+4 | Downloads |
| Arrow keys, Enter (picker) | Move between thumbnails, use the selected one |
| Esc | Back from the picker, then from the Art page |

## Implementation sequence

1. **Core images.** `Image` fields, `images_page`, and tests against the fake
   API.
2. **Core and CLI build removal.** `uses`, `remove` for runners and
   components, the three CLI commands, and docs.
3. **Art page.** Panel and menu entries, kinds with Browse and Remove, the
   target picker, and the game line. Maintainer looks at the layout in a
   real window before step 4.
4. **Picker.** Thumbnail grid, paging, filters, Use, and the missing-key
   state.
5. **Key in global settings.**
6. **Downloads page.** Listing with uses, Download with releases and
   progress, Delete with its confirm, and the sidebar spinner.
7. **Docs.** `gui.md`, `art.md`, `downloads-and-imports.md`, and the
   features backlog.

## Testing and acceptance

- Core: image parsing with and without the new fields; `images_page` URLs;
  `uses` for each kind of reference, including aliases that move after a
  delete; remove refusing while in use, while running, and while another
  install holds the lock; an interrupted remove leaves no usable half
  build.
- CLI: lifecycle tests for the three remove commands with the fake runners,
  including `--yes` and refusal output.
- GUI: Art page actions update the card and cache; picker paging, filters,
  selection, and Use against the fake API; missing key states; key set,
  invalid key, and remove; Downloads listing, delete confirm, refusal, and
  a canceled download leaving nothing behind.
- An acceptance checklist goes at the end once implementation lands, as in
  0039.

## Open questions

1. **Deleting what an alias points at.** Deleting the newest GE-Proton
   moves `GE-Proton-Latest` to the next one rather than breaking anything.
   Proposed: still refuse, since it silently changes which runner those
   environments use. Or allow it with a confirm that says which build they
   move to?
2. **Global default runner.** Proposed: count it as a use and refuse. Or
   allow it, since only new environments are affected?
3. **Filters reset.** Proposed: off on every visit. Or remember them for the
   session?

## Scope limits

- No library-wide art page or bulk fetch. `wineo art fetch` per environment
  covers bulk work for now.
- No art editing, cropping, or upload to SteamGridDB.
- No automatic update checks for installed builds.
- Downloads stays Linux x86_64 only, as in the CLI.
- Snapshots, portable copies, exec, and winetricks are 0041.
