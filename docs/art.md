# Artwork

Each registered environment has an art store: an icon, a grid (poster), a
hero (banner), and a logo. Art comes from SteamGridDB or from files already
on disk. Manual files never need an API key.

```sh
wineo art key
wineo art set hades icon ~/Pictures/hades-icon.png
wineo art fetch hades
wineo art list hades
wineo art rm hades grid
```

Wineo stores and manages art here. Showing posters or heroes in a GUI is
later work.

## The store

Art lives at `$XDG_DATA_HOME/wineo/art/<env>/`, or `DIR/art/<env>/` in a
portable folder so it moves with the folder. A target can override any kind
under `art/<env>/<target>/`; a kind the target does not override falls back
to the environment's own art. Each kind is one file named for its kind and
real format, such as `icon.png` or `grid.jpg`.

Accepted formats depend on kind. The type is detected from the file's
contents, not its extension:

| Kind | Formats              |
|------|----------------------|
| icon | PNG, JPEG, SVG, ICO  |
| grid | PNG, JPEG, SVG, WebP |
| hero | PNG, JPEG, SVG, WebP |
| logo | PNG, JPEG, SVG, WebP |

Files are capped at 32 MiB. The original bytes are kept as is in the store;
wineo does not convert or resize anything there. A desktop entry's icon is a
converted copy; see [Desktop entries](#desktop-entries) below.

`art/<env>/art.toml` records where each file came from: a SteamGridDB asset
ID, or a manual file with its original name. See [Config file
versioning](config-files.md) for its layout. A file that art.toml does not
record, including every file when art.toml is absent, is unrecorded; `art
fetch` never replaces it without `--replace` or `--asset`.

If more than one file exists for a kind, the one art.toml records wins.
With no record, the first found in the order PNG, SVG, ICO, JPEG, WebP wins.
`art set` and `art fetch` remove the old file when a kind changes format.

## The SteamGridDB key

```sh
wineo art key
wineo art key --remove
```

`wineo art key` prompts for the key without echoing it, checks it against
the SteamGridDB API, and saves it to the system keyring over the Secret
Service API. With no keyring available, it saves a `0600` file at
`$XDG_DATA_HOME/wineo/steamgriddb.key` instead and warns in one line that
the key is stored in plain text, naming the file. The key file always lives
in the host data directory, never in a portable folder. Saving to the
keyring deletes a leftover key file. Piped stdin is accepted.

A keyring that exists but fails, such as a dismissed unlock prompt, is an
error; the key never silently falls back to the plain text file in that
case. `wineo art key --remove` deletes the key from both the keyring and the
file, printing `Removed the SteamGridDB key` or `No SteamGridDB key was
saved`.

Lookup order for commands that need the key: `WINEO_STEAMGRIDDB_KEY`, then
the keyring, then the plain text file. When the variable is set, `art key`
and `art key --remove` add `note: WINEO_STEAMGRIDDB_KEY is set and overrides
the saved key`. A command that needs the key and finds none says to run
`wineo art key` or set the variable, in one short line.

Keyring access needs `dbus` (`libdbus-1`) at runtime; the Arch package
already depends on it.

## Setting manual art

```sh
wineo art set ENV[.TARGET] KIND FILE
```

Copies `FILE` into the store as the given kind, for the environment or one
of its targets, and prints a line like `Set icon for hades.server`. Refuses
a file that is not one of the kind's accepted formats, or one over 32 MiB,
or an unknown target.

## Listing art

```sh
wineo art list ENV
```

Shows the environment's kinds, then a block for each target that overrides
something:

```
hades (SteamGridDB game 5247)
  icon  icon.png   SteamGridDB 36952
  grid  grid.jpg   manual, from poster final.jpg
  hero  hero.webp  unrecorded
  logo  none
hades.server
  icon  icon.svg   manual, from server.svg
```

Warnings go to stderr and do not change anything on disk: a duplicate file
for a kind (`grid.png ignored, grid.jpg is recorded`), a target file losing
to its own duplicate (`server/hero.jpg ignored, server/hero.png takes
precedence`), a record whose file is missing (`server/logo.png is
missing`), and a `[targets.<t>]` table for a target the environment does not
have (`` art for unknown target `old` ``). The next `art set` or `art rm` of
that kind replaces or drops the stale record.

## Removing art

```sh
wineo art rm ENV[.TARGET] KIND
```

Removes one piece of art and prints `Removed grid from hades`. Fails with
`` `hades` has no grid art `` if there is nothing to remove. Removing a
target's override never touches the environment's own art.

## Fetching art from SteamGridDB

```sh
wineo art fetch ENV[.TARGET] [--search TERM | --game ID] [--only KIND] [--replace] [--asset ID]
```

Searches SteamGridDB with the environment name, or `--search TERM`, and
shows the matches as a numbered pick list with the release year when known:

```
Which game is it?
  1) Hades (2020), 1234
  2) Hades II (2024), 5678
  3) Hades Demo, 9012
  4) Search for something else
Ctrl+C to cancel.
Game [1]:
```

`Search for something else` asks for a new term and shows its matches,
until a game is picked. Esc at `Search for:` goes back to the list. A search
with no matches says so and asks for another term:

```
No SteamGridDB games match `dusk`.
Search for: Dusklight
```

Esc there ends the fetch without fetching anything. Ctrl+C cancels the fetch
with exit code 130. The pick is saved in art.toml
and reused by later fetches. `--search` searches again even when a game is
already saved; `--game ID` skips the search and overwrites the saved game.
art.toml has one game per environment, shared by all its targets. Picking a
game needs a terminal; without one, a fetch that would search fails with
`picking a game needs a terminal, pass --game instead`. `--game`, a saved
game, and `--asset` all work without a terminal.

By default fetch fills gaps: a kind that already has art is skipped. Prints
one line per kind:

```
icon: fetched SteamGridDB 36952
grid: skipped, it already has art
hero: nothing found
```

`--only KIND` limits the fetch to one kind, and exits 1 if that kind found
nothing (a plain fetch without `--only` exits 0 even if some kinds found
nothing). `--replace` re-ranks and replaces every kind it fetches, manual
files included. `--asset ID` requires `--only` and picks a specific image by
ID, replacing whatever that kind currently has. It looks the ID up in that
kind's results with no filters, since you picked that exact image, so
animated, NSFW, humor, and landscape grids are all allowed. The downloaded
file must still be a format the kind can store, or it is refused with `asset
999 is not a PNG, SVG, JPEG, or WebP image`. An ID not among that kind's
results fails with `asset 999 is not among this game's icon art`.

A fetch on a target only fills the target's own overrides; art it inherits
from the environment does not count as present.

Every ranked request filters to static images with NSFW and humor off, and
grids to the portrait sizes 600x900, 342x482, and 660x930. `--asset` skips
all of these filters. Icons must be at least 128 px on their short side to
rank first, and icons and logos prefer the official style. Ranking then
prefers your language (from `LC_ALL`, `LC_MESSAGES`, or `LANG`, with `C` or
`POSIX` meaning `en`, and `en` next), higher score, and larger images. An
error during a fetch stops it but keeps the kinds already fetched, so
running it again fills the rest. An interrupted download leaves no partial
file. Without a terminal, a search with no matches fails with `` no
SteamGridDB games match `term` ``.

## Fetching during new and import

`wineo new` and `wineo import heroic` can fetch art for the environment they
create, so a new game gets its art without a separate `art fetch`. Each flow
looks for a SteamGridDB key before offering art. With none, it prints one
line and asks nothing:

```
note: run `wineo art key` to fetch art from SteamGridDB
```

A key lookup that fails, such as a dismissed keyring prompt, prints a
warning instead and skips art the same way.

Guided `wineo new` asks right before its review, defaulting to yes, then asks
for the search term, starting from the environment name:

```
Fetch art from SteamGridDB?
  1) Yes
  2) No
Esc to go back, Ctrl+C to cancel.
Art [1]: 1
Search for [the-sims-4]: The Sims 4
```

The review shows `art  fetch from SteamGridDB, searching "The Sims 4"`, or
`art  none`. The search and pick list run right after the environment is
created and before the installer, so every question comes before the wait.
Guided `wineo import heroic` has an `art` row on its review instead,
searching the Heroic title, and editing it asks the same two questions.

The pick list in these flows has the same `Search for something else`
entry and asks again after a search with no matches, as `art fetch` does. It
ends with `None of these, skip art`, which fetches nothing and prints how to
fetch later. Esc after a search with no matches does the same:

```
Skipped art. Fetch later with: wineo art fetch the-sims-4 --search <term>
```

Unguided flows fetch nothing unless given `--art`:

- `wineo new -n NAME --art` searches the environment name.
- `wineo import heroic --game ID -n NAME --art` searches the Heroic title.
- `wineo import heroic --all` asks `Fetch art for these games?` after the
  desktop question. `--art` answers yes ahead of time, and `--yes` without
  `--art` means no.

With no terminal to show the pick list, and for every game in `--all`, the
first search result is used and named in one line:

```
hades: art from SteamGridDB game 1234, Hades (2020)
```

`wineo art fetch --search` or `--game` replaces a wrong pick later. Only
kinds with no art are fetched, as with a plain `art fetch`.

Failures never stop the flow. A rejected key, a failed download, or a
search with no matches when there's no pick list prints a warning such as
`warning: art fetch failed: ...` and the command carries on; kinds fetched before the error stay. Art never
changes the exit code of `new` or `import`, and in `--all` it never counts
as a failed game. Ctrl+C at the pick list cancels the rest of the command
and keeps the environment.

### Which kinds

By default these flows fetch all four kinds. The global config can limit
them to the icon:

```toml
# ~/.config/wineo/config.toml
[art]
fetch = "icon"   # or "all", the default
```

This only applies to `new` and `import`. `wineo art fetch` always fetches
every kind unless `--only` says otherwise.

## Desktop entries

A target with `desktop = true` uses icon art for its entry, in the same
order as any other icon lookup: the target's own icon first, then the
environment's, then the icon wine's menu builder pulls out of the exe.

SVG is installed as is, under
`$XDG_DATA_HOME/icons/hicolor/scalable/apps/`. PNG, JPEG, and ICO are
converted to one PNG, at the largest standard hicolor size that fits the
image, up to 256x256. A non-square image is padded to a square with
transparency first. An image is never upscaled, except one under 16 px,
which goes to 16. An ICO with several images uses its largest one. The cap
is 256 rather than 512 because some launchers, such as Noctalia, never look
in `512x512`.

`wineo art set`, `wineo art rm`, and `wineo art fetch` update the affected
entries right away, printing the usual `Updated the desktop entry for
<target> in <env>` line, but only when the icon itself changed. A launch
also picks up an icon file swapped by hand. The entry records where its icon
came from in an `X-Wineo-Icon` key (`art:<hash>` or `exe`), so an unchanged
icon costs nothing on the next reconcile.

Icon art that wineo can't decode falls back to the exe's icon, when there
is one, printing
`Icon art for <target> in <env> could not be read` once per bad file.

Portable environments still get no desktop entries.

## Lifecycle

`clone` and `portable copy` copy an environment's art to the new name, and
their reviews show `art: copied`; they refuse if art already exists under
the new name. `wineo rm` retains an environment's art in trash for restoration
or pruning; see [Trash and removal](trash.md). Removing a target, whether by
unsetting it with `wineo set` or during `wineo setup`, deletes that target's
art and its art.toml records. Snapshots
do not include art: a snapshot restore that drops a target keeps its art
on disk, and `art list` warns about it.
