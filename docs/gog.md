# GOG games

GOG games are DRM-free, so a GOG game in wineo is a normal environment:
Wine or [native](native.md), with every setting, snapshot, hook, and stat
other environments get. What GOG adds is a login, a list of what you own,
installing games, moving the games Heroic installed into wineo, updates,
and repairs.

A GOG game follows one of two tracks:

- **Galaxy:** a Windows game installed from GOG Galaxy's builds, file by
  file, the way the GOG Galaxy client and Heroic install them. Updates
  download only the files that changed, a game can follow a beta branch
  or stay on an older build, and `--repair` fixes damaged files.
- **Installer:** a native (Linux) game, or a Windows game, installed from
  GOG's offline installers. Updates download the whole new installer.

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

`wineo import gog --logout` removes the login, that account's list of
owned games, and the passwords saved for private branches. Logging in as
another account removes those passwords too.

wineo uses the GOG Galaxy client's login, like every other Linux GOG tool,
since GOG offers no other. If GOG ever blocks it, a wineo release changes it.

## Owned games

```
$ wineo import gog --list
ID          TITLE                          STATE          ENVIRONMENT    TRACK      VERSION           UPDATE
1094900565  RimWorld                       imported       rimworld       installer  1.6.4871 rev588   update available
1207658924  The Witcher: Enhanced Edition  installed      the-witcher    galaxy     1.5 (CS) GOG 0.2  up to date
1771589310  Disco Elysium - The Final Cut  in Heroic
1455877361  HuniePop 2: Double Date        not installed
```

`--list` shows what your account owns, with each game's state:
`not installed`, `heroic` (installed by Heroic, not moved yet), `imported`
(moved from Heroic), or `installed` (installed by wineo), with its
environment, track, version (on the Galaxy track, GOG's name for the
build, else its id), and update state. The list is cached for an hour; `--refresh` asks GOG again.
If GOG can't send it, the last whole list stands in with a warning. See
[JSON output](json-output.md#gog) for `--json`.

## Installing games

```
$ wineo import gog --game 1455877361
Install HuniePop 2: Double Date (1455877361)
  environment  huniepop-2-double-date (new Windows environment)
  folder       ~/Games/HuniePop 2: Double Date
  track        Galaxy
  platform     Windows
  language     en-US
  runner       GE-Proton11-7
  arch         win64
  version      1.1.0a
  setup        script interpreter
  download     1.2 GiB
Esc or Ctrl+C to cancel.
Install it? [Y/n]:
Installed HuniePop 2: Double Date as huniepop-2-double-date.
```

`--game ID` installs a game you own that isn't installed yet (a game
Heroic has is moved instead; see below). It makes a new environment named
after the game, in a folder named after it under `defaults.games_root`
(`~/Games` unless you [set it](config-files.md)), with the DLC you own.

- `--platform linux|windows` picks the platform. The default is Linux when
  GOG has a Linux installer.
- `--track galaxy|installer` picks how a Windows game installs. The default
  is Galaxy when GOG has a Galaxy build of the game, else its offline
  installer. A native game always uses its installer.
- `--branch NAME` installs from a Galaxy branch and follows it, and
  `--build ID` installs one listed build and stays on it (see
  [Branches and pinned builds](#branches-and-pinned-builds)).
- `--language TAG` is a locale tag like `fr-FR`; the default is
  `gog.language`, else `en-US`. A product GOG doesn't offer in that language
  installs in English, and the review says so.
- `--runner` and `--arch` choose how a Windows game runs, with the same
  defaults as `wineo new`. `--dir PATH` installs somewhere else; the folder
  must be new or empty.
- `--dry-run` shows the review, `--yes` skips the question (and is needed
  without a terminal), and the game's GOG art and a desktop entry come
  along, like moving a game.

A Windows game needs your desktop session to install on either track,
since setup runs in its prefix: without one, it's refused before
downloading. wineo reads the game's `goggame-<id>.info` afterwards for the
program to start; when it names none, you pick one from the programs
installed, as with `wineo new`.

### On the Galaxy track

wineo installs the latest build GOG lists on the game's default branch
(or the branch or build you name), with the DLC you own. Every file is downloaded in pieces, each checked against GOG's
hashes, and put in place once whole. The review's download size is the
build's exact size; what crosses the network is compressed, so it's
usually less. The space check runs before anything is written.

Then setup runs in the new prefix, the steps the GOG Galaxy client would
run: GOG's script interpreter or the game's own setup program, then the
redistributables the game needs, like DirectX or Visual C++. The review's
`setup` row lists them. Files the game's setup needs but the game folder
doesn't (the `support` row) go to a folder of wineo's own, and the
redistributables to a store every Galaxy game shares, so one version is
downloaded once. A redistributable that asks for a restart is noted, since
Wine has nothing to restart.

### From offline installers

`--track installer` (or a native game) downloads GOG's installers first,
the game and each DLC, and checks them before anything changes. The
download size is GOG's, rounded to whole MiB, and the installed size is an
estimate (twice the installers for a Windows game); the real space check
runs on the downloaded files.

A native game is unpacked from GOG's package, never run as a script, and
starts with `start.sh`. A Windows game runs GOG's installer silently in a
new prefix, then each DLC's. If a Windows installer quits before
installing anything, wineo asks once whether to open its own setup window
instead:
`Unreal Tournament 2004's installer didn't start. Run its own setup window? [Y/n]:`.

### Interrupted installs

An install writes down what it's doing first, like an update, so a
crash, a power cut, or Ctrl+C leaves something to finish rather than a
broken game. Running the same `--game ID` again finishes it from what was
already downloaded, on the track it began on; an install from offline
installers also finishes with `wineo update NAME`. Until then the game
won't start, and says which command finishes it. A game whose install
stopped before its environment was saved shows as `Install didn't finish`
in the app; `wineo import gog --forget FOLDER` drops it instead, leaving
the files.

wineo never deletes files after a failed install from offline installers,
since it can't tell the installer's files from yours; `wineo update NAME
--reinstall` is the fix.

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
stays where Heroic put it. A game Heroic installed from Galaxy builds
stays on the Galaxy track.

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
NAME         INSTALLED         AVAILABLE         STATE
rimworld     1.6.4871 rev588   1.6.4900 rev600   update available
  DLC Royalty (1233017772): 1.6.4871 rev588 -> 1.6.4900 rev600
  DLC Ideology (1827857764): version unknown
the-witcher  1.5 (CS) GOG 0.2  1.5 (CS) GOG 0.2  up to date
```

`wineo update --check` compares your GOG games with what GOG offers now.

- **Galaxy games** compare the installed build with the latest one GOG
  lists on the branch the game follows, shown by GOG's name for each build (the build id when there's
  none). Any other build is an update, including one another tool
  installed or one GOG no longer lists.
- **Native games** compare installer versions, DLC included, so an update
  to one DLC is found too. A game Heroic installed has no record of its
  DLC versions, so it's compared by its base game until its first update
  or `--reinstall` records them.
- **Windows installer games** compare the installer version wineo (or
  Heroic) ran.

When wineo can't tell (another language, a version it doesn't know), it
says `can't compare` rather than guess. A Galaxy game GOG lists no
current (generation 2) build of on the branch it follows, which wineo
can't install, shows `update on GOG` with a
way out under its row: `GOG has no Galaxy build of this game that wineo
can update. Move it to offline installers with: wineo update NAME
--reinstall --track installer`. On a branch other than the default, the
way out is `wineo update NAME --default-branch` instead. A game pinned to
a build shows `pinned` when its branch has a newer one, with a note under
its row saying how to follow the branch again.

Every check also asks GOG which DLC you own, so DLC you bought since the
install shows up as `not installed`, and `wineo update` installs it. A DLC
that failed before is tried again, and one GOG didn't offer for your
platform is checked again. If GOG can't say which DLC you own, nothing new
is listed and a warning says so.

`wineo update NAME` updates one game. `--all` updates every game with an
update in one go, after one question, and finishes interrupted updates
and installer-track installs; an interrupted Galaxy install is listed with
the `wineo import gog --game ID` that finishes it. It skips Windows games
when there's no desktop session.
`--dry-run` shows the review without downloading.

### Galaxy updates

```
$ wineo update papers-please --yes
Update papers-please
  folder          ~/Games/Papers, Please
  track           Galaxy
  Papers, Please  1.2.76 -> 1.4.11
  setup           script interpreter
  download        84.6 MiB
Setup's own writes in the prefix aren't counted in the space check.
Updated papers-please.
  Papers, Please  1.2.76 -> 1.4.11
Moved your files aside where the new version has files:
  data/levels.dat -> data/levels.dat.wineo-saved
```

A Galaxy update downloads only new and changed files, reusing the parts of
a changed file that are the same. Each file is checked and put in place
beside the old one, and only after every file is in place are old ones
removed. What it does with your files:

- A game file you edited that the new build changes is moved aside to
  `NAME.wineo-saved`, never overwritten, and the new one takes its place.
- A file of yours where the new build has a file is moved aside the same
  way.
- An old file the new build drops is deleted only when it's exactly what
  GOG shipped; one you changed is kept, and listed.

Setup steps whose program or version changed run again, as do new ones,
and newly owned DLC installs with the update. Setup's own writes in the
prefix can't be known ahead, so the space check leaves them out and the
review says so.

A game installed by another tool (Heroic, or the GOG Galaxy client before
a move) has no list of its files in wineo yet. When GOG still lists its
build, that build's file list is the starting point and the files the
update keeps are checked by size. Otherwise every file is checked against
the new build: matches are kept, anything else is moved aside, and nothing
is deleted, so files of the old build the new one doesn't list stay. The
review says so.

If GOG stops listing the build an interrupted install or update was going
to, and wineo's own copy of that build's file list was lost too, finishing
it moves to the latest build instead. The review says so,
and answering yes (or `--yes`) agrees.

### Installer updates

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

An update on the installer track downloads every installed product's
current package first, so you can keep playing while it downloads. If
something is still running from the game's folder when the download
finishes, it stops and says what (`rimworld-copy is running from this game
folder. Stop it before updating rimworld.`), and the downloads stay for
next time. It rebuilds the game from the packages, base game first and
then each DLC, and lists what it did with your files:

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

### Reinstalling and repairing

`--reinstall` rebuilds a game even when nothing changed. On the installer
track it rebuilds from current packages, which repairs changed or missing
files; for a Windows game it reruns the current installers, which also
repairs a replaced prefix's registry and redistributables. On the Galaxy
track it moves to the latest build of the branch the game follows (or
rebuilds the build it's pinned to), checks every file against it, fetches
only what doesn't match, and runs all of setup again.

`--repair` checks a Galaxy game's files without changing its build:

```
$ wineo update papers-please --repair --yes
Repair papers-please
  folder          ~/Games/Papers, Please
  track           Galaxy
  Papers, Please  1.4.11.124
  check           every file, 56 files (86.2 MiB)
  setup           script interpreter, all run again
  download        only what's missing or damaged
Setup's own writes in the prefix aren't counted in the space check.
Repaired papers-please.
  Papers, Please  1.4.11.124
Saved these files aside, since they didn't match GOG's:
  baselib.dll -> baselib.dll.wineo-saved
```

Every file of the installed build is hashed, support files included. An
update leaves files it doesn't change alone, so damage to them is only
found here. Missing and damaged
files are downloaded again, and a file that doesn't match is moved aside
first, never overwritten. Then all of setup runs again, which also sets up
a replaced prefix. A repair works from the file list wineo stored at the
game's last install or update. A game wineo has no list for, like one
another tool installed, is checked against its build on GOG instead; when
GOG no longer lists that build, the repair says to run `wineo update NAME`
to move to the latest.
Windows games on the installer track can't be repaired this way: `Only
Galaxy games can be repaired. Move it with: wineo update NAME --reinstall
--track galaxy`. For a native game, `--reinstall` is the repair.

### Moving between tracks

`--reinstall --track galaxy` moves a Windows installer game onto Galaxy
builds, and `--reinstall --track installer` moves a Galaxy game onto the
offline installers. The review shows the move (`track  installer ->
Galaxy`).

A move onto Galaxy checks each file the installer left against the latest
build: matches are kept, anything else is moved aside and downloaded, and
nothing is deleted, so files the installer left that the build doesn't
list stay. The review says so. A move to the installer track runs the
current installers over the game, like a Windows installer update.

### Branches and pinned builds

Some Galaxy games have branches besides the default one, like a beta or a
legacy version. `--builds` lists what GOG offers, and changes nothing:

```
$ wineo update papers-please --builds
BRANCH           BUILD              VERSION     PUBLISHED
default          56353505673715002  1.4.11.124  2023-03-22  installed, latest
default          55257829924550446  1.2.76      2022-03-09
Legacy (64-bit)  55257829924550446  1.2.76      2022-03-09  latest
Legacy (32-bit)  51283582026170494  1.1.65      2018-06-07  latest
```

The default branch comes first, then the others in GOG's order, each
newest first. The marks say which build is `installed`, which is the
`latest` of its branch, which one the game is `pinned` to, which branch it
`followed` (when it isn't the default), and which builds are on a
`private` branch. `--refresh` asks GOG again rather than using the details
it saved, and `--json` prints [JSON](json-output.md#gog).

- `--branch NAME` follows a branch from now on and updates to its latest
  build, which may be older than what's installed. Branch names are
  matched exactly; quote ones with spaces.
- `--default-branch` returns to the default branch.
- `--build ID` installs one listed build and pins the game to it. It takes
  a build id, or a version name that matches exactly one listed build. A
  build listed only on another branch follows that branch too.
- `--latest` unpins, and updates to the latest build of the branch
  followed.

```
$ wineo update papers-please --branch "Legacy (64-bit)"
Update papers-please
  folder          ~/Games/Papers, Please
  track           Galaxy
  branch          default -> Legacy (64-bit)
  Papers, Please  1.4.11.124 -> 1.2.76
  setup           script interpreter
  download        58 MiB
```

A change that moves no files, like pinning the build that's installed or
`--latest` on a game already at the latest, only changes what the game
follows:

```
$ wineo update papers-please --build 1.4.11.124
Pin papers-please
  folder  ~/Games/Papers, Please
  track   Galaxy
  pin     build 56353505673715002, until --latest
Pinned papers-please to build 56353505673715002.
```

A pinned game stays on its build. A pin holds the build, not the DLC:
DLC you buy since, or that failed before, is still installed from the
pinned build when it has it. Otherwise `update --all` leaves the game
alone, and plain `wineo update NAME` changes nothing and exits 0:
`papers-please is pinned to build 56353505673715002 (1.4.11.124). Run
wineo update papers-please --latest to follow its branch again.` `--reinstall` on a pinned game rebuilds the pinned build and keeps the
pin. An interrupted change to a pinned build GOG has since stopped
listing finishes on the latest build of the branch instead, after asking,
and drops the pin.

Branch flags exclude each other and `--repair`, need a Galaxy game (`Only
Galaxy games have branches. Move it with --reinstall --track galaxy.`),
and are refused while an update is unfinished, until it's finished. A
branch GOG doesn't list is refused with a pointer to `--builds`.

### Private branches

Some branches are private: GOG only lists them with a password, which
the game's developer hands out. `--builds` says `GOG also has private
branches. Name one with --branch NAME.` when a game has them.
`--branch NAME` on one asks for its password at a terminal:

```
$ wineo update the-witcher --branch beta
Branch beta of The Witcher is private. Password:
```

`--password-stdin` reads it from standard input instead, one line, with
only the line ending removed. Without a terminal, wineo names the command
that works (`Give its password with --password-stdin.`). GOG answers a
wrong password exactly as it answers a branch that isn't there, so either
says `GOG lists no branch beta for The Witcher, or refused its password.`

A password is saved once GOG lists its branch with it, in your keyring
(`service=wineo`, `account=gog-branch`, with `user`, `game`, and `branch`
attributes), or without a keyring in `gog-branches.key` in wineo's data
directory, readable only by you. It belongs to the GOG account that saved
it, and only that account's passwords are used. After that, `--builds`,
`--check`, and updates use it, and saved private branches show in
`--builds` marked `private`. Entering a password again replaces it, and
one GOG refuses later is removed. When the password of the branch a game
follows is missing, `--check` says `can't compare` and how to enter it
again, `wineo update NAME` asks at a terminal, and `update --all` reports
that game as failed rather than asking.

### Interrupted updates

An update writes down what it's doing before it touches the game, so a
crash, a power cut, or Ctrl+C never leaves a half-updated game that runs.
Until it's finished, the game won't start:

```
$ wineo start rimworld
error: rimworld's game folder is mid-update. Finish it with wineo update rimworld.
```

`wineo update NAME` finishes it, as the kind of change it was (an update,
reinstall, repair, or move). On the installer track it finishes from the
packages already downloaded when they're all there, even offline; on the
Galaxy track a setup step that failed is retried from there, without
downloading again. Other environments running from the same folder (a
clone, say) are blocked too and say which game to finish. When something
fails, the error ends with the command that finishes it.

## Where GOG games live

GOG games live in the host library. In a [portable folder](portable.md),
logging in and listing work, but moving and updating say `GOG games live
in the host library. Run this without --portable.`

A Galaxy game's support files live in wineo's data directory under
`gog/support/`, and the redistributables every Galaxy game shares under
`gog/redist/`. A redistributable version no Galaxy game uses any more is
removed after the next successful install or update.

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
- A Windows installer trusts its exit code: an installer that changes
  files and then says it never started is taken at its word on a first
  attempt.
- Galaxy installs and updates take the latest build of the branch a game
  follows, or the build it's pinned to. Older Galaxy builds (GOG's first
  generation), patches between builds, and builds GOG no longer lists
  can't be installed.
- Bonus content, cloud saves, Mac games, and posting playtime back to GOG
  aren't supported.
