# Desktop app

`wineo-gui` is wineo's desktop app. It shows every environment as a library
of cards with its art, whether it's running, and how long it's been played,
and it starts and stops applications and games. It was added in
plan 0036.

Plan 0037 added settings pages for each
environment and for the global config, covering what `wineo set` and the
target half of `wineo setup` do. The GUI also has guided flows for creating,
importing, and cloning environments.
Plan 0039 added removal, the trash,
and checking for problems.
Plan 0042 adds compact controls, panel
motion, and grouped review and management layouts. Future GUI changes follow
the [GUI style guide](gui-style.md), which includes the original visual preview.

A sidebar on the left has **Library** at the top. **Trash**, **Checkup**,
**Downloads**, and **Snapshots** sit at the bottom, above a divider and
**Settings**. Add opens
from the Library.

## Starting it

Run `wineo-gui`, or open "wineo" from your desktop's app menu. The Arch
package installs both binaries and the menu entry. The release tarball has
the binaries only, so start it from a terminal or add your own entry.

It uses the same configs, prefixes, and run state as the CLI, including a
portable folder set with `WINEO_PORTABLE_DIR`. There's no `--portable` flag
yet.

## The library

Each environment gets a card with its grid art, or its name when it has
none. Posters are 160 x 240 px at the usual size (see
[Zoom and poster-only](#zoom-and-poster-only)), with the title and compact
metadata below. The grid is centered, so the space left over beside the
last column splits evenly between the two sides.
The title is the one you set in Settings, then a Steam game's name from
Steam, then the default target's explicit desktop entry name when that entry
is enabled, and otherwise the environment name. The environment
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

[Steam games](steam.md) have a small Steam mark on their card. One Steam
lists as not installed, or whose install is only queued, is grayed out and
dimmed, and its panel shows **Install** in place of Play. A game Steam is
installing gets a thin progress bar along the bottom of its poster once
Steam knows the size, and the panel's State says `installing 42%`. One
whose library is unavailable, or whose Steam files can't be read, gets a
warning dot instead, since it may still be on disk. A Steam game's title is
the one Steam has, and its playtime is Steam's total, with `-` for sessions.

The library refreshes every 2 seconds, so a game started or stopped from
the CLI shows up without doing anything.

Type in the search field to filter by card title, environment name, or
tag, so `rpg` finds games tagged `rpg`.

### Sorting

**Sort** beside the search field picks the order: Name (the title shown on
the card), Last played, Playtime, or Date added. Picking one starts it in
its natural direction: Name A to Z, Last played most recent first, Playtime
most played first, and Date added newest first. The arrow button next to it
flips the direction, and its tooltip names the order you have, like "Most
played first" or "A to Z". **Favorites first**, at the bottom of
the Sort menu, puts favorites ahead of everything else in the same order.

A game with no date added (one made before wineo recorded it) or a playtime
wineo can't read goes last in either direction. A game never played is a
real value: it's least played and least recent. Ties go to the title,
A to Z.

The sort, its direction, and Favorites first are saved in `library.toml`
(see [Library state](config-files.md#library-state)), so the next launch
opens the same way. A window keeps its own order while it's open, even if
another window changes the saved one. If the save fails, the window keeps
the order you picked and says why.

### Filters and hidden games

**Filters** opens a popup with the stores your games came from (Steam,
GOG, Epic, Amazon), the platforms they run on (Native, Windows), your
tags, **Show hidden**, and **Clear filters**, listing only what's in your
library. Within each group, a game has to match any of the checked
entries, and with checks in more than one group, it has to match each of
them; the button counts them, as in **Filters (2)**. Clear filters clears
the search, stores, platforms, and tags, and leaves the sort and Show
hidden alone. Filters and Show hidden start fresh at every launch.

A Steam game counts as Windows once Steam shows it runs through Proton:
it has its own Proton setting in Steam, or a Proton prefix from its first
run. Until then it has no platform, so only the Store filter finds it.

While a store, platform, or tag filter is on, a tab hangs under
**Filters** with a chip for each. A chip's x removes that filter, and the
larger x at the end clears them all but leaves the search. When the chips
don't fit, the last one becomes **+N**, which opens the popup. Tab reaches
the x's right after Filters. The grid scrolls under the tab, and the tab
steps aside while a notice shows under the toolbar.

A hidden game leaves the grid and search. Show hidden brings it back
dimmed, with a crossed-out eye on its poster, so you can unhide it.

### Favorites

Star a game with the star beside its title in the panel, **Add to
favorites** in its right-click menu, `F` on the selected card, or the
outlined star that fades in at a poster's top-right corner when you point
at it. Favorites show a filled star there, and clicking it removes the
favorite without opening the panel. Hide a game with **Hide** in the same menu; there's
no key for it, since the game disappears. Both save to `library.toml` right
away. If a save fails, the card goes back to what's saved and a notice says
why, like `Couldn't hide Hades: ...`.

### Zoom and poster-only

The slider at the right of the toolbar sizes the posters, from 60% to 200%
of the usual 160 x 240 px, and snaps to the usual size when you drag near
it. Ctrl+= (or Ctrl++) and Ctrl+- step 10 points at a time, and Ctrl+0
goes back to the usual size. Ctrl+wheel over the grid or the slider zooms
too, and a trackpad works. A zoom keeps your place: the selected card, or
the first one in view, stays where it was on screen.

Below 90%, only the title shows under each poster. A game that's running,
busy, installing, or has a problem gets a status dot in its poster's
top-left corner instead, and a GOG badge that doesn't fit beside it becomes
a small arrow; the panel has the details. Zooming across 90% fades the
extra lines out or in, rather than cutting them.

The button beside the slider hides the titles. Each card is then just its
poster, and hovering or selecting one shows its title over the bottom. The
zoom and the titles setting save to `library.toml` (`zoom` and
`poster_only` under `[view]`; see [config files](config-files.md)).

Sorting, filtering, searching, and favorites that move a card animate the
cards to their new places: cards that stay slide, new ones fade in, and
ones that leave fade out.

### When nothing shows

An empty grid always says why, so it never looks like a deleted library:

- **Nothing matches your filters**, with **Clear filters**, when a search
  or filter rules everything out. If hidden games would match, it says so,
  as in "2 matches are hidden", with **Show hidden** beside it.
- **Everything here is hidden**, with the count and **Show hidden**, when
  every game is hidden.
- **Your library is empty**, with **Add something**, when there's nothing
  yet.

The buttons can be reached with Tab.

### A library.toml that can't be read

If `library.toml` doesn't parse, a banner above the grid names the problem
and its line, says favorites and hiding are off until it's fixed, and
offers **Open Checkup**, where it can be repaired. Every game still shows
and launches, without stars or hidden flags. A file written by a newer
wineo gets the same banner without the button, since the fix is updating
wineo. The banner clears on its own once the file reads again.

### An unreadable trash entry

If a trash entry can't be verified, a banner above the grid says what it
blocks (one environment, some, or every environment) and offers
**Open Checkup**. It shows even when no environments are left. A blocked
card's panel says why and it can't be launched until the entry is dealt
with; every other environment keeps working. The Trash page lists the entry
under **Unreadable entries** with its reason. When everything it kept is
inside the trash folder, **Delete this entry** deletes it after a
confirmation, here or in Checkup; otherwise the page names the folder to
inspect and remove by hand.

## The detail panel

Click a card to open its panel, and click it again, click unused Library space,
or press Esc to close it. Double-click a card to open it and launch it right
away. The panel shows
the hero art with the logo in its corner, the state, playtime, sessions,
when it was last played, and its tags.

Play/Launch and Stop occupy the first action row, with Log, Settings, Art,
and Actions on a quieter row below. They share compact rounded controls.
The panel slides and fades over about 160 ms. Library cards glide to their
new slots with a lightly staggered spring and a small jiggle, settling in
about 300 ms. A poster's hover star and its poster-only title fade in and
out. Closing takes effect immediately, including stopping log follows.
Switching cards while open changes the details without replaying the entrance.

- **Play** or **Launch** starts the environment's default target. The label
  is Play for the exact `game` tag and Launch otherwise, including native
  applications and games. New environments from the built-in `gaming`
  template receive the tag; existing environments can add it in Settings.
  An environment with more than one target has a **Launch target:** dropdown.
- **Stop** stops it, like `wineo stop`. If it's still running 3 seconds
  later, the button turns into **Force stop**, which kills it right away like
  `wineo stop --force`. The wait keeps a double-click from killing it.
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

A [Steam game](steam.md) starts through Steam, so the panel says
**Waiting for Steam...** until the game appears, with **Stop** available.
Steam may be starting or updating first. A game started from the Steam
client shows as running too, and **Stop** and **Force stop** work on it. The
panel also shows **Launcher** (Steam), the game's **Proton** build when it
has one, and its **Install size**.

**Install** opens Steam's install dialog. The panel says **Asking Steam...**
and then `Asked Steam to install NAME.`, or a note like `Already
installed.`. Play stays off while a game is queued, installing, or
uninstalling. Right-click an installed Steam game for **Uninstall**, which
opens its panel and Steam's uninstall dialog; Steam asks for confirmation,
so wineo doesn't. A running game is refused: `Stop NAME before
uninstalling it.` The panel's hero and logo art are dimmed for a game that
isn't installed, like its card.

A [GOG game](gog.md) shows an **Update** row in its panel: **Up to date**,
**Update available**, **Update on GOG**, or **Can't compare**. Games with
an update, on either track, get an **Update available** badge on their
poster and **Update** in the Actions and right-click menus. **Update**
opens a page with the same review as `wineo update NAME`, then progress,
then what changed and the lists of your files it moved aside, kept, or
overwrote. On the installer track the review shows versions, the download
size, and the unpacked estimate, and progress goes through download,
verify, unpack, and publish; a Windows game's update shows each installer
as it runs, and if one quits before installing anything, the page asks
once whether to open its own setup window. A Galaxy game's review shows
its track, versions, support files, setup steps, and download size, with
the notes `wineo update` prints, and progress goes through downloading
and each setup step, after checking every file when it finishes a repair
or reinstall the CLI started. A warning in the review, like a build
GOG no longer lists, is accepted by pressing **Update**. The page's main
button names what it does: **Update**, **Repair**, **Reinstall**,
**Switch**, or **Finish**. When the branch a game follows is private and
its password isn't saved, the page asks for it (`Branch NAME of TITLE is
private.`) with a hidden Password field and **Continue**; a password GOG
refuses asks again. Planning and updating both count as work for Back,
Cancel, and closing the window. When the page finishes, the game is
checked with GOG again, so its Update row stays current. A game whose last update was interrupted has a **Finish
update** badge and button in place of Play (**Finish install** for an
interrupted install), which finishes it on the update page, or for a
Galaxy install on the install page; when something fails, the page says
which button finishes it.

A GOG Windows game's panel has a **Version** row: on the Galaxy track the
installed build's name, with `(pinned)` or the branch it follows, and on
the installer track the installer version. Until GOG has been asked, it's
read from the game's own record. Its **Change** button, or **Version...**
in the Actions menu, opens the version picker in the panel, like **Log**;
**Back** or Esc returns to the details. The picker asks GOG for the
game's builds (the ones `wineo update NAME --builds` lists) and shows a
**Branch** dropdown, then the branch's builds with their dates and marks.
Picking the newest build on a branch follows that branch; picking an
older one pins the game to it, like `--build`. **Review** opens the update
page with the change, and a change that moves no files, like unpinning
the build that's installed, reviews with **Switch** in place of
**Update**. A pinned game's note says `Use Change to follow its branch
again.`

The picker also has **Repair** and **Move to offline installers**, which
review like `--repair` and `--reinstall --track installer`. A game on the
installer track instead says so and offers **Move to Galaxy** when GOG
has a Galaxy build of it. When GOG says the game has private branches,
**Enter a private branch** asks for a branch's name and password and
lists its builds; GOG answers a wrong password like a missing branch, so
either says `GOG lists no branch NAME for TITLE, or refused its
password.` A password GOG accepts is saved, as in the CLI. Logged out, or
with an update unfinished, the picker says why, and **Finish update**
stays. A portable folder has no Version row, like the panel's other GOG
rows. Another environment running
from its folder can't play and says which game to finish. Once after the
app starts, when you're logged in to GOG and not in a portable folder, it
checks your GOG games for updates in the background; a failure only goes to
the terminal.

Games keep running after you close the app, the same as games started with
`wineo start`. Reopen it to see or stop them.

If wineo lost track of a run, for example after a crash, the error has a
**Clean up** button that does what `wineo stop` does.

**Settings** opens the environment's settings page, and **Art** opens its
[Art](#art) page.

Each card's right-click menu starts with **Add to favorites** (or
**Remove from favorites**) and **Hide** (or **Unhide**). The detail actions
menu and the right-click menu also offer
**Settings**, **Art...**, **Clone**, **Check for problems**, and
**Remove...**. Cloning
has a review page; Wine clones copy the prefix and art, while native clones
share the game files. Both keep external game paths, disable desktop
entries, clear the Heroic id, and reset playtime. Steam games don't offer
Snapshots, Run program, Winetricks, Clone, or Copy to portable, since Steam
owns their prefix.

### Removing

**Remove...** asks first. Its config, art, and playtime go to the trash, like
`wineo rm`. **Also trash the prefix and snapshots** is off by default and
does what `--include-prefix` does; the dialog shows the prefix's path and
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
can't be restored or deleted until they're recovered. Entries wineo can't
verify are listed under **Unreadable entries** (see
[An unreadable trash entry](#an-unreadable-trash-entry)). If the trash can't
be read at all, the page shows the error and **Check again**.

## Checkup

Checkup runs `wineo validate` for everything when you open it: the global
config, templates, every environment, and leftovers from environments that
no longer exist. Problems are grouped like the CLI's report, and healthy
environments aren't listed. Click an environment's heading to open it in
the Library. Fixes, progress, and Cancel work as in the panel, and
interrupted trash work can be recovered here too. An unreadable trash entry
can be deleted here with **Delete this entry** when that's safe. The summary of the last
batch stays until you press **Check again** or fix more, even if the batch
finished while you were on another page.

## Downloads

**Downloads** lists the installed runners, DXVK builds, and vkd3d-proton
builds, each with what uses it: "used by hades, celeste (GE-Proton-Latest)"
or "unused". The uses are the same ones `wineo runners remove` checks; see
[Remove a runner or graphics component](downloads-and-imports.md#remove-a-runner-or-graphics-component).
Directories that aren't usable builds are listed under **Not usable** with
the reason.

With [builds from other launchers](downloads-and-imports.md#use-builds-from-steam-heroic-and-lutris)
on, a read-only **From Steam, Heroic, and Lutris** section follows the
runners, with each build's source and what uses it, like "Steam, used by
celeste". It never offers Delete, and neither does a build in a runners
folder you pointed inside a launcher's own folder. A row whose name hides a
different build elsewhere says so, like "Heroic has a different build".

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
remain complete. On every Add, import, and clone step that can go back,
Back sits at the top and the forward action (Next, Review, Import, Create,
Clone) in a strip pinned at the bottom, so both stay reachable in a short
window while the content scrolls between them.

**Add** opens a compact overlay for **New environment**, **Import prefix**,
**Import from Heroic**, **Import from Steam**, or **Import from GOG**, keeping the library visible behind it. The same
choices are available when the library is empty. Back preserves entries;
leaving an initial draft returns to Add choices and asks before discarding
edits.

New environments can be Windows or Native Linux. Windows supports an empty
environment, an existing executable, or an installer; Native Linux supports
an empty registration or executable. Installer setup sets up DXVK and
vkd3d-proton in the prefix before the installer runs, then scans installed
programs once it's done, even if it exited with an error, so you can choose
the game's target rather than registering the installer. A canceled
installer keeps the environment and skips the scan: `Installer canceled. Add
a target in Settings.` The **Name** is the game's title, used for its
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
Ctrl+A, and Enter work too. Those buttons and **Import** stay pinned at the
bottom while the title and posters scroll. Already imported and unsupported
games are dimmed with the reason under the title. **Import 1 game** opens an editable
review; importing several opens a batch review with per-game results. A
game's Heroic poster becomes its grid art, shown as a prefilled pick marked
**From Heroic** in the single review, where **Remove** lets SteamGridDB fill
it instead. The review opens at the top and only calls out anti-cheat that
won't be imported, anything that blocks the import, and other games sharing
the prefix when it's used in place. While importing, the status names the
game and its phase, such as checking or copying the prefix with a file count,
with "(2 of 5)" in front during a batch. Imports and clones do not launch
automatically.

Steam import shows the installed Steam games that aren't imported yet as a
poster grid, using Steam's cached posters, with the same selection controls
as Heroic import. With a [Steam Web API key](steam.md#owned-games), games
you own but haven't installed follow, dimmed, after **Getting owned
games...**; they import the same way and show **Not installed** instead of a
size. **Refresh** fetches the owned list again. Without a key, the page says
`Add a Steam Web API key to see uninstalled games.` with **Open Settings**,
which goes to the key. When the list can't be fetched, it says `Couldn't
refresh owned games: REASON` and uses the saved list, or `Couldn't get owned
games: REASON` when there isn't one. Steam's own tools are left out, and a game Steam is
updating is marked **Updating in Steam**. Importing one game asks for its
environment name, which defaults to a slug of the title. The review lists
each game with its source, `game` tag, and Steam's cached art, and the
finish page counts the results like `wineo import steam --all`: `Imported 1
of 2 games.` In a portable folder it refuses: `Steam games can't be imported
into a portable folder.`

**Import from GOG** shows every GOG game you own, as tiles, to install or
to move from Heroic like `wineo import gog` (see [GOG games](gog.md)).
Logged out, it offers only the games Heroic installed and says to log in.
Games already in wineo say `Installed as NAME.` or `Already migrated as
NAME.`, an install that stopped before its environment was saved shows
`Install didn't finish` and can be finished, and games
Heroic installed say `In Heroic`. Heroic's issues, and entries Heroic can't describe, or a
game two Heroic configs put in different folders, show as issues instead of
disappearing. A game both a native and a Flatpak Heroic list in one folder
shows once. Each tile shows the game's poster: its library art when wineo
already has it, otherwise GOG's box art, downloaded once and cached.

Picking a game you own opens the install review: the environment it
creates, the folder (with **Browse...**), the platform when GOG offers
both, the track (**Galaxy** or **Offline installer**) when a Windows game
can install either way, the language (the setting, or one you type; Enter
checks it), a runner and arch for Windows, the version, and the DLC you
own. From offline installers it also shows the language it actually
installs in, the download size, and an estimate of the installed size; on
Galaxy, the support files, the setup steps, and the build's exact size.
When GOG can't say whether it has a Galaxy build, both tracks are offered
and the error shows, so **Offline installer** still works. **Install**
stays off until the review matches what you've typed. Then come download
and install progress, with **Cancel** and
**Force stop**, then the result. A Windows game whose info file names no
program offers a list of the programs it installed, the game's own folder
first. Back from the result returns to the library with the game
selected.

Picking a game Heroic installed opens a review: the environment it attaches to or
creates, the folder, track, version, language, DLC, and for a new Windows
environment the prefix (Heroic's, kept where it is) and a runner choice
with the same copy and download offers as Heroic import. When another
Heroic config also listed the game, a **Heroic** row says which one was
used. **Migrate** moves it, then adds GOG's playtime and art like the CLI,
whose problems are warnings that never undo the move. Back, the leave
prompt, Cancel, and closing the window during work behave like Heroic
import; a cancel after the move keeps it and skips what's left of the
playtime and art. In a portable folder the page says GOG games live in the
host library.

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
**Additional wine options**. Picking a template tagged `game` shows a card
under the Template picker, until it's answered once: "Use Proton and Wine
builds from Steam, Heroic, and Lutris too?" with **No** and **Yes**. The
answer is saved and the runner choices refresh in place. Heroic import asks
the same at the top of its details step, and **Yes** uses Heroic's runner
where it is instead of copying it. Runners from other launchers read like
"GE-Proton11-3 (Steam)" in pickers and reviews. The card never shows in a
portable folder. Selecting a template shows a compact summary
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

- **Card:** the **Title** the library card, the panel, and the poster-only
  overlay show. Leave it empty to use the desktop entry's name or the
  environment name, which the field shows as its placeholder. It is trimmed,
  at most 120 characters, with no control characters; Save stays off until
  it fits.
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

Native environments only show what they can use. A Steam game's page has
only **Tags**, **Title**, and **Desktop entry**, since Steam owns
everything else. Its **Title** is read-only: it shows the saved fallback, or
**Not set**, and says it's used when Steam doesn't know the game. The card
keeps Steam's own name. Paths typed on the page
have to be absolute or start with `~/`, except inside a portable folder.

Long runner names shorten in the middle to keep dropdowns on one row.
Hover the selected runner to see its full name.

### Global settings

The global page has two tabs under its title, **General** and
**Appearance**, and opens on General. General holds everything below.
Switching tabs keeps your edits, and one Save and Revert footer covers
both. Each tab remembers how far it's scrolled while the page is open.

- **Launch defaults:** gamemode, MangoHud, and Wayland for every
  environment that doesn't set its own.
- **New environments:** the runner and arch `wineo new` uses, whether to
  use **Builds from Steam, Heroic, and Lutris**, and where prefixes, runners,
  DXVK, and vkd3d-proton builds go. The builds switch is off until it's
  answered, and is disabled in a portable folder. Empty root fields show
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
key. Only the one that applies shows: **Set key...** with no key,
**Remove key** with one saved, and both while the variable's key could
hide a saved one. Both apply right away, outside the Save bar, and don't count as
edits. You can leave the page while one runs, and its result shows when you
come back. With `WINEO_STEAMGRIDDB_KEY` set, the section says it overrides
the saved key. See [The SteamGridDB key](art.md#the-steamgriddb-key).

**Steam Web API** works the same way for the key that lists
[owned Steam games](steam.md#owned-games), checked with Steam before it's
saved, and shows where to get a key: <https://steamcommunity.com/dev/apikey>. With
`WINEO_STEAM_KEY` set, it says that overrides the saved key.

**GOG** shows the GOG username you're logged in as, or the account number
until wineo has looked the name up. **Log in**, shown while you're
logged out, opens GOG's login page
in your browser and shows its address with **Copy link**, then a masked
**Address** field for the address of the page you land on, like `wineo
import gog --login`; what you paste is never shown or logged. **Log out**,
shown while you're logged in, removes the login and that account's owned-games list. Both run outside
the Save bar, and their result shows if you leave and come back. The
section works in a portable folder too. **Games folder** (with
**Browse...**) and **GOG language** (a locale tag like `fr-FR`) set where
and in what language games install; they save with the Save bar, and a
portable folder leaves them alone.

### Appearance

The **Appearance** tab picks the app's colors (plan
0057). Picking a theme repaints the app right
away, so you can try a few before you **Save**; **Revert**, or leaving and
choosing **Discard**, puts back what was saved. Theme edits count in the
same bar as General's, and one **Save** writes both.

- **Follow the system**, on by default, switches with the desktop's light
  or dark mode. It shows a **Dark theme** list and a **Light theme** list,
  and the app uses whichever matches. Turned off, one **Theme** list holds
  every theme, each marked Dark or Light, and the app keeps it whatever the
  desktop does. Turning it off starts that list on the theme showing, so
  nothing changes on screen.
- The built-in themes are Midnight and Daylight (the defaults), Cellar and
  Cellar Light, High contrast, Dracula, and Catppuccin Macchiato and Latte.
  Each row shows a swatch, the name, and a line about it.
- **Colors** shows the nine colors of the theme showing. Built-in themes
  are read-only; **Copy to a custom theme** makes an editable copy, named
  "<Name> copy", and picks it in place of the original. A custom theme has
  a hex field per color ("Invalid color: use #rrggbb." for a bad one, which
  isn't applied), and **Rename** and **Delete** beside its name. Deleting a
  theme that's in use goes back to Midnight or Daylight. A theme a newer
  wineo made is read-only, but you can still copy it.
- **Improve readability**, off by default, raises the contrast of text
  colors that need it, in any theme. Dracula's muted text and several of
  Catppuccin Latte's colors change; the wineo themes and Macchiato already
  pass. Swatches show the colors you'll get.

Copies, renames, color edits, and deletes all wait for **Save**, which
writes the theme files first, then `library.toml`, then removes deleted
themes. Custom themes live in
[`themes/`](config-files.md#custom-themes) as plain TOML files you can
share. If a save can't write `library.toml` because it's damaged, the bar
offers **Open Checkup**; one from a newer wineo says to update instead.
The edits that didn't land stay, so you can save again after fixing it. If
`library.toml` or a theme file changes on disk while you have Appearance
edits, the bar says which, and **Reload** takes what's on disk.

## Keyboard

| Key | Does |
|---|---|
| Arrow keys | Move between cards |
| Enter | Open the panel, or play when it's open (only for a card that's shown) |
| F | Add the selected card to favorites, or remove it (only for a card that's shown) |
| Esc | Close the log, version picker, or results, then the panel, then clear the search |
| `/` or Ctrl+F | Search |
| Ctrl+N | Open Add (Library only) |
| Ctrl+= or Ctrl++, Ctrl+- | Make the posters bigger or smaller (Library only) |
| Ctrl+0 | Posters back to the usual size (Library only) |
| Delete | Remove the selected environment, asking first (on the details) |
| Ctrl+Z | Undo a removal while its bar shows |
| Ctrl+1, Ctrl+2, Ctrl+3, Ctrl+4, Ctrl+5 | Library, Trash, Checkup, Downloads, Snapshots |
| Ctrl+, | Global settings |
| Down (in the search) | Back to the cards |
| Tab, Shift+Tab | Move focus through the toolbar from Add to the titles button, then panel actions |
| Left, Right (on the zoom slider) | Make the posters smaller or bigger |

The mouse Back button does whatever Esc does on the current page, here and
in every table below. Forward does nothing.

In the Sort menu and the Filters popup, which take every key while open:

| Key | Does |
|---|---|
| Enter, Space (on Sort or Filters) | Open the menu under its button, on the current choice |
| Arrow keys, Tab, Shift+Tab | Move between rows |
| Enter, Space | Pick a sort, or toggle a check box or tag |
| Esc | Close the menu and go back to its button |

In the choice lists on **Add**, New's program list, the Heroic and GOG
lists, and the version picker:

| Key | Does |
|---|---|
| Up, Down | Move between rows, stopping at the first and last |
| Enter, Space | Choose the row |
| Tab, Shift+Tab | Walk through the rows with the dialog's other controls |
| Ctrl+Enter | Submit the dialog, without choosing the row |

The first row has focus when a list appears, and **Add**'s 1 to 5 keys still
work with a row focused.

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
| Tab, Shift+Tab | Move between the tabs and fields |
| Left, Right (on the tabs) | Switch between General and Appearance |
| Esc | Leave the field, then an environment's page, asking first if there are edits |

In an Add, import, or clone flow:

| Key | Does |
|---|---|
| Ctrl+Enter | Activate the page's primary action |
| 1 to 5 (Add choices) | Choose New, prefix import, Heroic, Steam, or GOG import |
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
- A list view, saved filter presets, and grouping (the later parts of the
  library views series in plan 0054).
