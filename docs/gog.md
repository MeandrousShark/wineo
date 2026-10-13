# GOG games

GOG games are DRM-free, so a GOG game in wineo is a normal environment:
Wine or [native](native.md), with every setting, snapshot, hook, and stat
other environments get. What GOG adds is a login, a list of what you own,
installing games, moving the games Heroic installed into wineo, and updates.

wineo installs and updates native (Linux) games and Windows games from
GOG's offline installers. Games GOG Galaxy installed through its build
system (`galaxy` track, like a game Heroic installed that way) show their
updates but can't be updated yet; that comes in a later release.

## Logging in

```
$ wineo import gog --login
Log in to GOG in your browser. If it doesn't open, go to:
https://auth.gog.com/auth?client_id=...
Then copy the address of the page you land on.
Address:
Logged in to GOG as rimworlder.
```

wineo opens GOG's login page in your browser and prints its address too,
for when there's no browser at hand. Log in, then paste the address of the
page GOG lands you on (it starts with `https://embed.gog.com/on_login_success`).
What you paste isn't shown, and it never appears in wineo's output or logs.
wineo then asks GOG's public profile service for your username, sending
only your account number. If that doesn't answer within 2 seconds, you're
still logged in and wineo shows `Logged in to GOG as account N.` until a
later renewal finds the name.

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
`not installed`, `heroic` (installed by Heroic, not moved yet), `imported`
(moved from Heroic), or `installed` (installed by wineo), with its
environment, version, and update state. The list is cached for an hour; `--refresh` asks GOG again.
If GOG can't send it, the last whole list stands in with a warning. See
[JSON output](json-output.md#gog) for `--json`.

## Installing games

```
$ wineo import gog --game 1207658924
Install Unreal Tournament 2004 (1207658924)
  environment  unreal-tournament-2004 (new Windows environment)
  folder       ~/Games/Unreal Tournament 2004
  platform     windows
  language     en-US
  runner       GE-Proton11-7
  arch         win64
  version      3369.3
  download     about 4.6 GiB (an estimate)
  installed    about 9.2 GiB (an estimate)
Esc or Ctrl+C to cancel.
Install it? [Y/n]:
Installed Unreal Tournament 2004 as unreal-tournament-2004.
```

`--game ID` installs a game you own that isn't installed yet (a game
Heroic has is moved instead; see below). It makes a new environment named
after the game, in a folder named after it under `defaults.games_root`
(`~/Games` unless you [set it](config-files.md)), with the DLC you own.

- `--platform linux|windows` picks the installer. The default is Linux when
  GOG has one.
- `--language TAG` is a locale tag like `fr-FR`; the default is
  `gog.language`, else `en-US`. A product GOG doesn't offer in that language
  installs in English, and the review says so.
- `--runner` and `--arch` choose how a Windows game runs, with the same
  defaults as `wineo new`. `--dir PATH` installs somewhere else; the folder
  must be new or empty.
- `--dry-run` shows the review, `--yes` skips the question (and is needed
  without a terminal), and the game's GOG art and a desktop entry come
  along, like moving a game.

Everything is downloaded first, the game and each DLC, and checked before
anything changes. The download and installed sizes are estimates, since
GOG rounds its sizes; the real space check runs on the downloaded files.

A native game is unpacked from GOG's package, never run as a script, and
starts with `start.sh`. A Windows game runs GOG's installer silently in a
new prefix, then each DLC's, which needs your desktop session: without one,
it's refused before downloading. wineo reads the game's
`goggame-<id>.info` afterwards for the program to start; when it names
none, you pick one from the programs installed, as with `wineo new`.

If a Windows installer quits before installing anything, wineo asks once
whether to open its own setup window instead:
`Unreal Tournament 2004's installer didn't start. Run its own setup window? [Y/n]:`.

### Interrupted installs

An install writes down what it's doing first, like an update, so a
crash, a power cut, or Ctrl+C leaves something to finish rather than a
broken game. Running the same `--game ID` again, or `wineo update NAME`,
finishes it from what was already downloaded. A game whose install stopped
before its environment was saved shows as `Install didn't finish` in the
app; `wineo import gog --forget FOLDER` drops it instead, leaving the files.

wineo never deletes files after a failed Windows install or update, since
it can't tell the installer's files from yours. `wineo update NAME
--reinstall` is the repair.

## Moving games from Heroic

```
$ wineo import gog --game 1094900565
Migrate RimWorld (1094900565)
  environment  rimworld (new native environment)
  folder       ~/Games/RimWorld
  track        installer
  version      1.6.4871 rev588
  language     en
  dlc          Royalty (1233017772)
               Ideology (1827857764)
               Biotech (1865538500)
Esc or Ctrl+C to cancel.
Migrate it? [Y/n]:
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
  DLC Royalty (1233017772): 1.6.4871 rev588 -> 1.6.4900 rev600
  DLC Ideology (1827857764): version unknown
```

`wineo update --check` compares your GOG games with what GOG offers now.
Native games compare installer versions, DLC included, so an update to one
DLC is found too. A game Heroic installed has no record of its DLC
versions, so it's compared by its base game until its first update or
`--reinstall` records them. When wineo can't tell (another language, a
version it doesn't know), it says `can't compare` rather than guess.

Windows games installed from GOG's installers compare the installer
version wineo (or Heroic) ran. Galaxy games show `update on GOG` but can't
be updated yet.

Every check also asks GOG which DLC you own, so DLC you bought since the
install shows up as `not installed`, and `wineo update` installs it. A DLC
that failed before is tried again, and one GOG didn't offer for your
platform is checked again. If GOG can't say which DLC you own, nothing new
is listed and a warning says so.

```
$ wineo update rimworld --yes
Update rimworld
  folder                 ~/Games/RimWorld
  RimWorld               1.6.4871 rev588 -> 1.6.4900 rev600
  Royalty (1233017772)   1.6.4871 rev588 -> 1.6.4900 rev600
  Ideology (1827857764)  unknown -> 1.6.4900 rev600
  download               412 MiB
  unpacked               about 412 MiB (an estimate)
Updated rimworld.
  RimWorld               1.6.4871 rev588 -> 1.6.4900 rev600
  Royalty (1233017772)   1.6.4871 rev588 -> 1.6.4900 rev600
  Ideology (1827857764)  unknown -> 1.6.4900 rev600
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

A Windows game is updated by running the newer installer over it, then
each DLC's that changed, with the same rules as installing. GOG's patch
installers aren't used, since GOG doesn't say which version a patch starts
from; the full installer always works.

`--reinstall` rebuilds from current packages even when nothing changed,
which repairs changed or missing files; for a Windows game it reruns the
current installers, which also repairs a replaced prefix's registry and
redistributables. `--all` updates every game with an update in one go,
after one question, and skips Windows games when there's no desktop
session. `--dry-run` shows the review without downloading.

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
that no environment owns, like one you deleted by hand, or of an install
that stopped before its environment was saved. The files stay.

## Limits

- wineo protects a game folder from updates while something wineo started
  runs from it. A game started another way (from Heroic, or by a program
  that finds it on its own) isn't seen.
- Launches that name the game by a bare program name Wine resolves, or
  arguments like `--data=/path`, aren't recognized as using the folder.
- Snapshots, clones, and portable copies copy the environment, not the
  game folder, which stays shared.
- A Windows install or update trusts the installer's exit code: an
  installer that changes files and then says it never started is taken at
  its word on a first attempt.
- Galaxy builds, bonus content, cloud saves, Mac games, and posting
  playtime back to GOG aren't supported.
