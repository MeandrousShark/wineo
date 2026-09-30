# Desktop app

`wineo-gui` is wineo's desktop app. It shows every environment as a library
of cards with its art, whether it's running, and how long it's been played,
and it starts and stops games. It was added in
[plan 0036](plans/0036-gui-library-launcher.md).

[Plan 0037](plans/0037-gui-settings.md) added settings pages for each
environment and for the global config, covering what `wineo set` and the
target half of `wineo setup` do. The GUI also has guided flows for creating,
importing, and cloning environments. Removal, trash, and validation remain
CLI tasks for now.

## Starting it

Run `wineo-gui`, or open "wineo" from your desktop's app menu. The Arch
package installs both binaries.

It uses the same configs, prefixes, and run state as the CLI, including a
portable folder set with `WINEO_PORTABLE_DIR`. There's no `--portable` flag
yet.

## The library

Each environment gets a card with its grid art, or its name when it has
none. Art comes from `wineo art`, using the default target's art when it
has its own. See [Art](art.md) for adding and fetching it.

Under the art are the environment's state and playtime, the same numbers
`wineo list` and `wineo stats` show. An environment whose config can't be
read gets a warning dot, and its panel shows the error.

The library refreshes every 2 seconds, so a game started or stopped from
the CLI shows up without doing anything.

Type in the search field to filter by name.

## The detail panel

Click a card to open its panel, and click it again or press Esc to close
it. Double-click a card to open it and play it right away. The panel shows
the hero art with the logo in its corner, the state, playtime, sessions,
when it was last played, and its tags.

- **Play** starts the environment's default target. An environment with
  more than one target has a picker next to Play.
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

**Settings** opens the environment's settings page.

The detail actions menu and each card's right-click menu also offer
**Settings** and **Clone**. Cloning has a review page; Wine clones copy the
prefix and art, while native clones share the game files. Both keep external
game paths, disable desktop entries, clear the Heroic id, and reset playtime.

## Add, import, and clone

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

The panel's **Settings** button opens an environment's page, and the gear
in the top bar opens the global one. Both replace the library until you
press **Back**.

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

## Keyboard

| Key | Does |
|---|---|
| Arrow keys | Move between cards |
| Enter | Open the panel, or play when it's open |
| Esc | Close the log, then the panel, then clear the search |
| `/` or Ctrl+F | Search |
| Ctrl+N | Open Add |
| Down (in the search) | Back to the cards |

On a settings page:

| Key | Does |
|---|---|
| Ctrl+S | Save |
| Tab, Shift+Tab | Move between text fields |
| Esc | Leave the field, then the page, asking first if there are edits |

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

## Not yet

- Removing, the trash, and validating come next, then snapshots, portable
  copies, general art and download management, and running programs.
- Gamepad navigation.
