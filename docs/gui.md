# Desktop app

`wineo-gui` is wineo's desktop app. It shows every environment as a library
of cards with its art, whether it's running, and how long it's been played,
and it starts and stops games. It was added in
[plan 0036](plans/0036-gui-library-launcher.md).

For now it's a launcher. Creating, importing, editing, and removing
environments still happen in the `wineo` CLI, and the app picks up the
changes on its own.

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
  while the game runs. **Back** returns to the details.

Starting can take a while the first time, since it builds a missing prefix
and installs missing winetricks verbs first. The panel shows what it's
doing. **Cancel** stops the start cleanly, and pressing it again as
**Force stop** kills it right away, like pressing Ctrl+C twice in the CLI.

Games keep running after you close the app, the same as games started with
`wineo start`. Reopen it to see or stop them.

If wineo lost track of a run, for example after a crash, the error has a
**Clean up** button that does what `wineo stop` does.

## Keyboard

| Key | Does |
|---|---|
| Arrow keys | Move between cards |
| Enter | Open the panel, or play when it's open |
| Esc | Close the log, then the panel, then clear the search |
| `/` or Ctrl+F | Search |
| Down (in the search) | Back to the cards |

## Look

The app follows your desktop's light or dark preference and switches when
it changes.

## Not yet

- Creating and importing environments, editing settings, validating, and
  the trash come in later plans.
- Gamepad navigation.
- Fetching art from the app. Use `wineo art fetch`.
