# Desktop app

`wineo-gui` is wineo's desktop app. It shows every environment as a library
of cards with its art, whether it's running, and how long it's been played,
and it starts and stops applications and games. It was added in
[plan 0036](plans/0036-gui-library-launcher.md).

[Plan 0037](plans/0037-gui-settings.md) added settings pages for each
environment and for the global config, covering what `wineo set` and the
target half of `wineo setup` do. The GUI also has guided flows for creating,
importing, and cloning environments.
[Plan 0039](plans/0039-gui-trash-and-validate.md) added removal, the trash,
and checking for problems.
[Plan 0042](plans/0042-gui-visual-polish.md) adds compact controls, panel
motion, and grouped review and management layouts. Future GUI changes follow
the [GUI style guide](gui-style.md), which includes the original visual preview.

A sidebar on the left has **Library** at the top. **Trash**, **Checkup**,
**Downloads**, and **Snapshots** sit at the bottom, above a divider and
**Settings**. Add opens
from the Library.

## Starting it

Run `wineo-gui`, or open "wineo" from your desktop's app menu. The Arch
package installs both binaries.

It uses the same configs, prefixes, and run state as the CLI, including a
portable folder set with `WINEO_PORTABLE_DIR`. There's no `--portable` flag
yet.

## The library

Each environment gets a card with its grid art, or its name when it has
none. Posters are 160 x 240 px, with the title and compact metadata below.
The title comes from the default target's explicit desktop entry name when
that entry is enabled. Otherwise it uses the environment name. The environment
name remains visible as muted context when different, and choosing another
launch target does not rename the card.
Art comes from `wineo art`, using the default target's art when it
has its own. See [Art](art.md) for adding and fetching it.

Art bigger than the app shows it is shrunk once and kept in
`$XDG_CACHE_HOME/wineo/art` (default `~/.cache/wineo/art`), so later starts
don't decode the full-size file again. The original in the art store is
never changed. Each size is made from the original, and replacing art makes
a new copy. The app removes copies of changed or deleted art when it starts.
A portable folder uses the host's cache too. Deleting the folder is always
safe.

Under the art are the environment's state and playtime, the same numbers
`wineo list` and `wineo stats` show. An environment whose config can't be
read gets a warning dot, and its panel shows the error.

The library refreshes every 2 seconds, so a game started or stopped from
the CLI shows up without doing anything.

Type in the search field to filter by desktop title or environment name.

## The detail panel

Click a card to open its panel, and click it again, click unused Library space,
or press Esc to close it. Double-click a card to open it and launch it right
away. The panel shows
the hero art with the logo in its corner, the state, playtime, sessions,
when it was last played, and its tags.

Play/Launch and Stop occupy the first action row, with Log, Settings, Art,
and Actions on a quieter row below. They share compact rounded controls.
The panel slides and
fades over about 160 ms, keeping the library layout steady during the
transition. Closing takes effect immediately, including stopping log follows.
Switching cards while open changes the details without replaying the entrance.

- **Play** or **Launch** starts the environment's default target. The label
  is Play for the exact `game` tag and Launch otherwise, including native
  applications and games. New environments from the built-in `gaming`
  template receive the tag; existing environments can add it in Settings.
  An environment with more than one target has a **Launch target:** dropdown.
- **Stop** stops it, like `wineo stop`.
- **Log** shows the run's log, the one `wineo logs` shows, and follows it
  while the game runs. Drag to select text, or click into the log and use
  Ctrl+A and Ctrl+C. **Copy** copies the whole log. Scrolling up or
  selecting pauses following; **Jump to end** resumes it. **Back** returns
  to the details.

Starting can take a while the first time, since it builds a missing prefix
and installs missing winetricks packages first. The panel shows what it's
doing, including which package is installing, like "Installing gdiplus
(2 of 3)...". **Log** is available during the install and follows the
install log, labeled **Install log**; once the game runs, **Log** shows its
**Game output** instead. **Cancel** stops the start cleanly, and pressing it again as
**Force stop** kills it right away, like pressing Ctrl+C twice in the CLI.
Creation and imports work the same way: **Force stop** replaces **Cancel**
only after you cancel.

Games keep running after you close the app, the same as games started with
`wineo start`. Reopen it to see or stop them.

If wineo lost track of a run, for example after a crash, the error has a
**Clean up** button that does what `wineo stop` does.

**Settings** opens the environment's settings page, and **Art** opens its
[Art](#art) page.

The detail actions menu and each card's right-click menu also offer
**Settings**, **Art...**, **Clone**, **Check for problems**, and
**Remove...**. Cloning
has a review page; Wine clones copy the prefix and art, while native clones
share the game files. Both keep external game paths, disable desktop
entries, clear the Heroic id, and reset playtime.

### Removing

**Remove...** asks first. Its config, art, and playtime go to the trash, like
`wineo rm`. **Also trash the prefix and snapshots** is off by default and
does what `--delete-prefix` does; the dialog shows the prefix's path and
size. Native environments have no prefix, so the box isn't there. Remove is
unavailable while the environment is running or busy, or after a lost run
until **Clean up**.

If wine is still running in the prefix, the dialog waits for it to exit,
and **Cancel** stops the removal without changing anything. Once data starts
moving, it can't be canceled.

Afterward, a bar at the bottom says what was removed and what was left in
place, with **Undo** for 10 seconds. The timer pauses while the pointer is
over it or **Undo** has focus, and Ctrl+Z works too. Removed environments
can also be restored from the Trash later.

### Checking for problems

**Check for problems** runs `wineo validate` for that environment and shows
the results in the panel, like **Log**. Each problem uses the CLI's message.
A problem with a fix has a checkbox under it, checked by default. Two
problems fixed the same way, like two games missing the same DXVK build,
share one fix, and their boxes toggle together.

**Fix N problems** applies the checked fixes one at a time, so one failing
doesn't stop the rest. Downloads show their progress, and installing
winetricks packages shows "Installing gdiplus (2 of 3)..." with
**Install log**, like Play. **Cancel** stops the running fix and skips the
rest; fixes that already finished stay done. **Force stop** kills winetricks
right away. When the fixes end, the check runs again, and a short summary of
what was fixed, what failed, and what was skipped stays until the next check.

Closing the panel leaves the results; reopening the environment shows its
details. A running batch keeps going and shows its status there.

## Trash

The Trash lists removed environments, newest first, with how long ago they
were removed and whether the prefix went with them. Hover a row for its ID
and anything left in place. The sidebar shows how many entries can be
restored.

- **Restore** brings an entry back under its old name and paths, like
  `wineo restore`. It refuses when something has taken the name or the
  prefix was replaced, and says why on the row.
- **Delete forever**, **Delete entries older than N days**, and
  **Empty trash** prune, like `wineo prune`. Each shows what it will delete
  and asks first, and confirming deletes exactly that list. A prefix left
  in place by a plain remove isn't part of the trash and stays where it is.
  Results for each entry stay on the page until your next action there.
  Pruning can't be canceled.

If a removal, restore, or prune was interrupted, the Trash page shows a
banner with **Recover** for each one, the same recovery as
`wineo validate --fix`, and the sidebar shows a warning dot. Those entries
can't be restored or deleted until they're recovered. If the trash can't be
read at all, the page shows the error and **Check again**.

## Checkup

Checkup runs `wineo validate` for everything when you open it: the global
config, templates, every environment, and leftovers from environments that
no longer exist. Problems are grouped like the CLI's report, and healthy
environments aren't listed. Click an environment's heading to open it in
the Library. Fixes, progress, and Cancel work as in the panel, and
interrupted trash work can be recovered here too. The summary of the last
batch stays until you press **Check again** or fix more, even if the batch
finished while you were on another page.

## Downloads

**Downloads** lists the installed runners, DXVK builds, and vkd3d-proton
builds, each with what uses it: "used by hades, celeste (GE-Proton-Latest)"
or "unused". The uses are the same ones `wineo runners remove` checks; see
[Remove a runner or graphics component](downloads-and-imports.md#remove-a-runner-or-graphics-component).
Directories that aren't usable builds are listed under **Not usable** with
the reason.

**Delete** shows only on unused builds. It asks first with how much space
it frees, and refuses on the row if something started using the build in
the meantime. A config that can't be read is skipped with a warning naming
the file, the same as the CLI.

**Download...** picks a provider and a release, newest stable first, with
**Load more** for older ones, then **Install** downloads it with progress
and **Cancel**, like `wineo download`. A canceled download leaves nothing
behind; one canceled after it finished installing stays installed.

Downloads and deletes keep running when you switch sections, with a spinner
next to **Downloads**. Closing the window cancels a download and waits for a
delete to finish. The list
reloads when the page opens and after its own changes; **Refresh** reloads
it after a game exits or a change from the CLI.

## Art

**Art** in the panel, or **Art...** in the menus, opens an environment's art
page. It replaces the library like a settings page, and **Back** or Esc
returns.

Each kind has a row: the art at its own shape, where it came from
(SteamGridDB with its asset ID, Manual, or None) and its size, and where it
appears:

| Kind | Appears |
|---|---|
| Icon | App menu and desktop entry |
| Grid | The library card |
| Hero | The top of the detail panel |
| Logo | Over the hero, in the panel corner |

- **Browse...** picks a local file, through the desktop's file chooser or a
  typed path when there isn't one. Its contents decide the format, like
  `wineo art set`.
- **Find...** opens the SteamGridDB picker.
- **Remove** deletes that kind's art, like `wineo art rm`.

Changes apply right away. The card, panel, and desktop entry update to
match.

The header shows the SteamGridDB game the art comes from, looked up by its
saved ID, and **Change** searches for a different one, like `wineo art fetch
--search`. Without a key, it shows the ID, and **Find...** and **Change**
are off, with a **Settings** button to add one.

**Showing** switches between the environment and its targets. On a target,
kinds it inherits are dimmed and say "From the environment". **Browse...**
and **Find...** set an override for that target, and **Remove** on an
override reads **Use environment art**.

### Finding art on SteamGridDB

The picker shows every image SteamGridDB has for that kind, as thumbnails
ranked best first. The first is marked **Best match**, and it's the one
`wineo art fetch` would pick. **Load more** shows more rows.

**Filters** reveals switches for **Animated**, **NSFW**, **Humor**, and, for
grids, **Other sizes**: images the ranked fetch leaves out. They're off
each time the picker opens. Images they add are tagged.

Click a thumbnail to see its ID, author, size, style, and score. **Use**, a
double click, or Enter downloads that image and replaces the kind's art,
with progress and **Cancel**. Leaving the page doesn't stop the download.
Closing the window cancels it, and canceled downloads change nothing.

## Running changes

Only one filesystem change runs at a time, including snapshots and portable
copies alongside removal, trash, downloads, and Checkup fixes.
While one runs, the others are unavailable and say what's busy. Games still
start and pages still open. Switching sections doesn't stop it, and its
result is waiting when you come back. Closing the window offers
**Keep working** or **Cancel and close**; the latter cancels what can be
canceled and waits for the rest.

Program runs and winetricks use per-environment workers. They block changes
to their own environment and other registrations sharing its prefix, while
unrelated environments stay usable. Switching pages keeps their workers,
output, and results. Closing cancels every active worker and waits for cleanup.
Once a snapshot swap or portable publication starts, the app says
**Finishing changes...** and waits; those changes have no Force stop.

## Snapshots

Open **Snapshots** in the sidebar (Ctrl+5), or choose **Snapshots...** from a
Wine environment's Actions menu. Pick an environment, enter a label, then
review **Create snapshot...**. Snapshots require reflinks and exclude external
files, art, and playtime. The page lists labels and creation times in label
order, with Restore, Delete, and Refresh.

Snapshot lists and review reports scroll inside bounded areas. During review,
the list and create form are hidden so the confirmation buttons stay close to
the report. Review details, expanded config text, and completion results are
read-only and selectable; use Ctrl+C for a selection or **Copy** for the whole
report.

Restore reviews the saved and current runner, prefix, and config. Expand
**Show config changes** to read both exact config texts. Restore replaces the
prefix and config, preserving art and playtime. The result retains any old
subvolume path that needs manual removal and offers **Check for problems**.
Desktop entries reconcile on the next run or through Checkup.

An interrupted restore shows **Roll back restore**, even when the ordinary
environment report cannot load. Checkup offers the same fix with its own
confirmation. Batch fixing skips rollback; it only runs when explicitly
chosen by itself. Malformed markers show an error. See [Snapshots](snapshots.md).

## Copy to a portable folder

Choose **Copy to portable folder...** from Actions, type a destination or
browse for one, then Review and Copy. Wine reviews include the prefix,
runner/layout, targets, snapshots, art, playtime, config/templates policy,
external links, and source paths that may need manual changes. Native reviews
copy the registration, art, and playtime; program files stay where they are.

Copies can use full space when reflinks aren't available. Future snapshot
create/restore still needs reflinks. Desktop entries are disabled in the
destination. Collisions and changed reviews require a fresh review; partial
publication errors retain the paths to move aside before retrying.

The result names the destination and the app keeps the current library. Use
**Copy to another folder...** to start another copy. A portable session says
**Copy from the standard library.** See [Portable folders](portable.md).

## Run program and winetricks

Wine Actions offers **Run program...** and **Winetricks...**, including in
portable sessions. Native environments don't show these actions.

Run program accepts a Wine name such as `winecfg`, a Windows path, or a Unix
path that is absolute or begins with `~/`. Browse supplies an absolute path.
Each argument chip is one exact argument, including spaces, and the preview
shows argv. Temporary KEY/VALUE overrides apply only to this run. No shell is
involved, and bare names never resolve against the GUI's working directory.

Exec can run beside a tracked application. It builds a missing prefix and
reconciles components and desktop entries, but doesn't install declared
winetricks verbs, apply target wrappers, register a target, or count playtime.
Output and the exit result stay on the page. Cancel stops only the process
group wineo started; programs handed off to Wine may remain running. If the
GUI crashes, the program keeps running and the GUI's locks are released.

Winetricks shows declared verbs as Installed or Missing. **Install missing**
builds a missing prefix and installs declarations without opening the picker.
**Edit in Settings** changes declarations. One-off verbs and Additional
arguments run without changing them; **Open winetricks** opens its own window.
An installer window may open. Cancel and Force stop settle the exclusive run,
and the installed record refreshes even after failure or cancellation.
Completed verbs stay installed; removing a declaration doesn't uninstall it.

## Add, import, and clone

The new-environment review groups Application, Source, and Runtime details
as applicable, with warnings and missing prerequisites below. Long paths
remain complete, and Back/Create stay outside the scrolling summary.

**Add** opens a compact overlay for **New environment**, **Import prefix**,
or **Import from Heroic**, keeping the library visible behind it. The same
choices are available when the library is empty. Back preserves entries;
leaving an initial draft returns to Add choices and asks before discarding
edits.

New environments can be Windows or Native Linux. Windows supports an empty
environment, an existing executable, or an installer; Native Linux supports
an empty registration or executable. Installer setup scans installed
programs after completion so you can choose the game's target, rather than
registering the installer. The **Name** is the game's title, used for its
desktop entry, and the environment name is derived from it: lowercase, with
spaces and punctuation turned into dashes, accents folded, and apostrophes
and trademark symbols dropped, so "Assassin's Creed: Origins" becomes
`assassins-creed-origins`. The derived name shows under the field; **Edit**
lets you choose a different one, up to 64 characters. The installer log can
be selected and copied like the panel's log. Prefix imports can copy a Wine prefix or Proton
compatdata, or register it in place. **Set up targets** opens Settings' Add
target editor after import. Target setup can be skipped and done later.

Heroic import discovers native and Flatpak configs, and supports manually
added Windows and Native Linux games. Games appear as a poster grid using the
posters Heroic already cached, with a title placeholder where Heroic has
none, so opening the picker makes no network requests. Click posters to
select them, or use **Select all** and **Select none**; arrow keys, Space,
Ctrl+A, and Enter work too. Already imported and unsupported games are
dimmed with the reason under the title. **Import 1 game** opens an editable
review; importing several opens a batch review with per-game results. A
game's Heroic poster becomes its grid art, shown as a prefilled pick marked
**From Heroic** in the single review, where **Remove** lets SteamGridDB fill
it instead. The review opens at the top and only calls out anti-cheat that
won't be imported, anything that blocks the import, and other games sharing
the prefix when it's used in place. While importing, the status names the
game and its phase, such as checking or copying the prefix with a file count,
with "(2 of 5)" in front during a batch. Imports and clones do not launch
automatically.

New and import flows show clickable poster, icon, hero, and logo previews.
New environment previews the game's page: the hero is a banner across the
top with the logo in its corner, like the library panel, the poster sits
beside the form, and the icon sits beside the name. Narrow windows move the
poster and icon below the form. Click a placeholder or preview to browse
for a file; use **Remove** to drop it. File paths are an optional fallback.
Manual files work without a SteamGridDB key or network access. Fetching uses
the global **Art to fetch** switches: after you search and choose a match,
each enabled kind downloads right away and appears in its tile, and creation
stores those images without downloading them again. Local files can be mixed
with fetched art, and the local choice wins for that kind. If artwork fails
after registration, retry or skip it. General artwork, key, and download
management remain CLI tasks; GUI managers are planned separately.

Windows creation keeps runner, architecture, and prefix overrides under
**Additional wine options**. Selecting a template shows a compact summary
of the settings it changes. Native creation hides the Wine-only controls.

The new environment finish page has an unchecked **Launch game when this
page closes** option when a playable target is ready. If checked, leaving for
the library or Settings launches that target once. Closing the application
does not launch it.

The GUI offers contextual runner and component downloads when a flow needs
them. It asks before downloading and shows progress; completed downloads
stay installed if the flow is later canceled. While a create, import, clone,
download, or installer operation is active, closing the window offers
**Keep working** or **Cancel and close**. Cancellation waits for cleanup and
offers **Force stop** while waiting.

## Settings

The panel's **Settings** button opens an environment's page, which replaces
the library until you press **Back**. **Settings** in the sidebar opens the
global page, which stays open until you pick another sidebar section.

Edits collect on the page, and the bar at the bottom counts them.
**Save** writes them the way `wineo set` does, with the same checks, and
keeps the file's comments and formatting. **Revert** throws them away.
Leaving with unsaved edits asks first. While a save runs, the controls are
locked, since installing DXVK or vkd3d-proton can take a moment. The bar
then says what changed, and whether a running game picks it up the next
time it starts.

If the file changes on disk while the page is open, from the CLI or an
editor, the page reloads on its own when you have no edits. With edits, the
bar says so, and **Reload** replaces them with what's on disk.

A config that can't be read shows its error and an **Open config file**
button, and the page reloads once the file is fixed.

### Environment settings

- **Runner:** the runner, from the installed ones, the system wine, and
  aliases like `GE-Proton-Latest`. Switching between Wine and Proton is
  refused, since their prefixes aren't compatible. Arch is shown but can't
  change once the prefix exists.
- **Launch:** gamemode, MangoHud, and Wayland follow the global config
  until you flip them. The label says where the value comes from, and
  **Reset** goes back to following it. Gamescope is a plain switch, since
  there's no global gamescope. Turning it off forgets its fields when you
  save, but the page remembers them until you leave, so flipping it back on
  restores them.
- **Components (Wine only):** DXVK and vkd3d-proton, each None, Latest, or
  an installed build. vkd3d-proton needs DXVK. On Proton they're off, and a
  leftover value gets a **Remove** button.
- **Winetricks (Wine only)** and **Tags:** chips. Type one and press Enter
  or **+**.
- **Environment variables:** name and value rows. `WINEPREFIX` is refused
  in Wine environments, since wineo manages it.
- **Targets:** a card per target with its exe, arguments, working
  directory, desktop entry and its name, and whether it's the default. Its
  gamemode, MangoHud, Wayland, and gamescope follow the environment until
  you flip them, like above. **Edit here** copies the environment's
  gamescope settings into the target, since a target's gamescope replaces
  the environment's whole table. **Add target** offers the programs
  installed in the prefix, like `wineo setup`, or a typed path. **Browse...**
  opens a file picker for the executable and working directory. **Remove**
  says which desktop entry and art go with the target first. Targets can't
  be renamed, and a removed name can be used again after saving.
- **Advanced:** the prefix path, and the game id and store protonfixes use
  (Proton only). The Heroic id and the kind are shown but can't change.

Native environments only show what they can use. Paths typed on the page
have to be absolute or start with `~/`, except inside a portable folder.

Long runner names shorten in the middle to keep dropdowns on one row.
Hover the selected runner to see its full name.

### Global settings

- **Launch defaults:** gamemode, MangoHud, and Wayland for every
  environment that doesn't set its own.
- **New environments:** the runner and arch `wineo new` uses, and where
  prefixes, runners, DXVK, and vkd3d-proton builds go. Empty root fields show
  the default locations for the current XDG or portable setup.
- **Runs:** how long `stop` waits before killing a game, and how big a log
  gets.
- **Art to fetch:** independent Icon, Grid, Hero, and Logo switches for
  `new` and `import`. Turn all four off to disable automatic fetching.
- **Environment variables** for every launch.
- **SteamGridDB:** where the API key comes from: "From
  WINEO_STEAMGRIDDB_KEY", "Saved in the keyring", "Saved in a plain text
  file" with its path, or "No key". The key itself is never shown.

**Set key...** takes the key in a masked field, checks it with SteamGridDB,
and saves it like `wineo art key`: to the keyring, or to a plain text file
with a warning when there's no keyring. **Remove key** deletes the saved
key. Both apply right away, outside the Save bar, and don't count as
edits. You can leave the page while one runs, and its result shows when you
come back. With `WINEO_STEAMGRIDDB_KEY` set, the section says it overrides
the saved key. See [The SteamGridDB key](art.md#the-steamgriddb-key).

## Keyboard

| Key | Does |
|---|---|
| Arrow keys | Move between cards |
| Enter | Open the panel, or play when it's open |
| Esc | Close the log or results, then the panel, then clear the search |
| `/` or Ctrl+F | Search |
| Ctrl+N | Open Add (Library only) |
| Delete | Remove the selected environment, asking first (on the details) |
| Ctrl+Z | Undo a removal while its bar shows |
| Ctrl+1, Ctrl+2, Ctrl+3, Ctrl+4, Ctrl+5 | Library, Trash, Checkup, Downloads, Snapshots |
| Ctrl+, | Global settings |
| Down (in the search) | Back to the cards |
| Tab, Shift+Tab | Move focus through library controls and panel actions |

In the SteamGridDB picker:

| Key | Does |
|---|---|
| Arrow keys | Move between thumbnails |
| Enter | Use the selected image |
| Esc | Back to the Art page |

On a settings page:

| Key | Does |
|---|---|
| Ctrl+S | Save |
| Tab, Shift+Tab | Move between text fields |
| Esc | Leave the field, then an environment's page, asking first if there are edits |

In an Add, import, or clone flow:

| Key | Does |
|---|---|
| Ctrl+Enter | Activate the page's primary action |
| 1, 2, 3 (Add choices) | Choose New, prefix import, or Heroic import |
| Arrow keys, Space (Heroic games) | Move between posters and toggle one |
| Ctrl+A, Enter (Heroic games) | Select every eligible game, import the selection |
| Tab, Shift+Tab | Move between controls |
| Esc | Go back, asking before discarding a filled draft |

## Look

The app follows your desktop's light or dark preference and switches when
it changes.

Settings, Snapshots, Downloads, Checkup, and Trash share grouped surfaces,
compact controls, and consistent headings. Settings keeps Save/Revert and
the edit count in a fixed footer. The [GUI style guide](gui-style.md) records
the palette, spacing, control states, motion, and layout rules for future pages.

## Not yet

- Gamepad navigation.
