# GOG games

GOG games are DRM-free, so a GOG game in wineo is a normal environment:
Wine or [native](native.md), with every setting, snapshot, hook, and stat
other environments get. What GOG adds is a login, a list of what you own,
moving the games Heroic installed into wineo, and updates.

This release moves games Heroic installed and keeps native (Linux) games
up to date. Installing new games, updating Windows games, and Galaxy builds
come in later releases.

## Logging in

```
$ wineo import gog --login
Log in to GOG in your browser. If it doesn't open, go to:
https://auth.gog.com/auth?client_id=...
Then copy the address of the page you land on.
Address:
Logged in to GOG as account 48628349957132247.
```

wineo opens GOG's login page in your browser and prints its address too,
for when there's no browser at hand. Log in, then paste the address of the
page GOG lands you on (it starts with `https://embed.gog.com/on_login_success`).
What you paste isn't shown, and it never appears in wineo's output or logs.

The login is kept in your keyring as `service=wineo`, `account=gog`, like
the Steam and SteamGridDB keys. wineo renews it as it goes, saving the new
one GOG hands out each time. A timeout or GOG being down keeps the login;
only GOG refusing it logs you out: `Log in to GOG again: wineo import gog
--login.`

`wineo import gog --logout` removes the login and that account's list of
owned games.

wineo uses the GOG Galaxy client's login, like every other Linux GOG tool,
since GOG offers no other. If GOG ever blocks it, a wineo release changes it.

## Owned games

```
$ wineo import gog --list
ID          TITLE                          STATE          ENVIRONMENT  TRACK      VERSION          UPDATE
1094900565  RimWorld                       imported       rimworld     installer  1.6.4871 rev588  update available
1771589310  Disco Elysium - The Final Cut  in Heroic
1207658924  Unreal Tournament 2004         not installed
```

`--list` shows what your account owns, with each game's state:
`not installed`, `heroic` (installed by Heroic, not moved yet), or
`imported` with its environment, and for imported games, their version and
update state. The list is cached for an hour; `--refresh` asks GOG again.
If GOG can't send it, the last whole list stands in with a warning. See
[JSON output](json-output.md#gog) for `--json`.

## Moving games from Heroic

```
$ wineo import gog --game 1094900565
Migrate RimWorld (1094900565)
  environment  rimworld (new native environment)
  folder       ~/Games/RimWorld
  track        installer
  version      1.6.4871 rev588
  language     en
  dlc          3 (1233017772, 1827857764, 1865538500)
Migrate it?
Press Enter to migrate it, Esc or Ctrl+C to cancel.
Migrated RimWorld as rimworld.
Remove it from Heroic's library, without deleting its files, so both don't update it.
```

`--game ID` moves a game Heroic installed; `--all` moves every one, and
with no flags wineo lets you pick. It needs no GOG login: everything it
reads is on your disk. Heroic's own files are never changed, and the game
stays where Heroic put it.

- If an environment already runs the game (same `gog_id`, or its default
  target is the game's program and, for Windows games, Heroic's prefix),
  the game attaches to it and every setting stays. Otherwise a new
  environment is made with what Heroic knows: runner, launch arguments,
  environment variables, and the rest.
- Windows games keep Heroic's prefix in place. Native games run
  `start.sh`.
- The game folder is recorded as the game's, so wineo knows which files
  updates may change. Your own files there, like RimWorld's `mods-1.5`,
  are never touched.
- When you're logged in, GOG's playtime and artwork come along: playtime
  becomes the larger of what wineo had and GOG's total, and GOG's poster,
  background, logo, and icon fill any art the environment doesn't have.
- Afterwards, remove the game from Heroic's library without deleting its
  files, so the two don't both update it.

If both a native and a Flatpak Heroic list a game in the same folder,
it's one game, read from the native Heroic (or the one used last), and the
review says which. If they list it in different folders, wineo asks you to
remove one.

A game already moved says `Already migrated.` Every move shows a review
first; `--yes` skips the question and is needed without a terminal.
`--dry-run` shows the review and changes nothing.

A game started before it was moved can't be seen by wineo, so stop it in
Heroic first.

## Updates

```
$ wineo update --check
NAME      INSTALLED        AVAILABLE        STATE
rimworld  1.6.4871 rev588  1.6.4900 rev600  update available
  DLC Royalty: 1.6.4871 rev588 -> 1.6.4900 rev600
  DLC Ideology: version unknown
```

`wineo update --check` compares your GOG games with what GOG offers now.
Native games compare installer versions, DLC included, so an update to one
DLC is found too. A game Heroic installed has no record of its DLC
versions, so it's compared by its base game until its first update or
`--reinstall` records them. When wineo can't tell (another language, a
version it doesn't know), it says `can't compare` rather than guess.

Windows games show their state but can't be updated yet: installer games
in a later release, Galaxy games (`update on GOG`) after that.

```
$ wineo update rimworld --yes
Update rimworld
  folder    ~/Games/RimWorld
  RimWorld  1.6.4871 rev588 -> 1.6.4900 rev600
  Royalty   1.6.4871 rev588 -> 1.6.4900 rev600
  Ideology  unknown -> 1.6.4900 rev600
  download  412 MiB
  unpacked  about 412 MiB (an estimate)
Updated rimworld.
  RimWorld  1.6.4871 rev588 -> 1.6.4900 rev600
  Royalty   1.6.4871 rev588 -> 1.6.4900 rev600
  Ideology  unknown -> 1.6.4900 rev600
Overwrote these files that differed from the new version:
  start.sh
```

`wineo update NAME` downloads every installed product's current package
first, so you can keep playing while it downloads. If something is still
running from the game's folder when the download finishes, it stops and
says what (`rimworld-copy is running from this game folder. Stop it before
updating rimworld.`), and the downloads stay for next time. It rebuilds the game from the
packages, base game first and then each DLC, and lists what it did with
your files:

- A file of yours where the new version has a file is moved aside to
  `NAME.wineo-saved`, never overwritten.
- An old file you changed, which the new version doesn't ship any more,
  is kept.
- A shipped file you changed is overwritten, and listed. So is any file
  that differed on a game's first update after moving from Heroic.

`--reinstall` rebuilds from current packages even when nothing changed,
which repairs changed or missing files. `--all` updates every native game
with an update in one go, after one question. `--dry-run` shows the review
without downloading.

### Interrupted updates

An update writes down what it's doing before it touches the game, so a
crash, a power cut, or Ctrl+C never leaves a half-updated game that runs.
Until it's finished, the game won't start:

```
$ wineo start rimworld
error: rimworld's game folder is mid-update. Finish it with wineo update rimworld.
```

`wineo update NAME` finishes it, from the packages already downloaded when
they're all there, even offline. Other environments running from the same
folder (a clone, say) are blocked too and say which game to finish.

## Where GOG games live

GOG games live in the host library. In a [portable folder](portable.md),
logging in and listing work, but moving and updating say `GOG games live
in the host library. Run this without --portable.`

`wineo import gog --forget PATH` drops wineo's record of a game folder
that no environment owns, like one you deleted by hand.

## Limits

- wineo protects a game folder from updates while something wineo started
  runs from it. A game started another way (from Heroic, or by a program
  that finds it on its own) isn't seen.
- Launches that name the game by a bare program name Wine resolves, or
  arguments like `--data=/path`, aren't recognized as using the folder.
- Snapshots, clones, and portable copies copy the environment, not the
  game folder, which stays shared.
- Bonus content, cloud saves, and posting playtime back to GOG aren't
  supported.
