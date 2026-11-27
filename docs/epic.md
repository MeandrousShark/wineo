# Epic games

wineo talks to Epic through [Legendary](https://github.com/legendary-gl/legendary),
the open-source Epic client Heroic also uses. An installed Epic game is a
normal Wine or Proton environment, with every setting, snapshot, hook, and
stat other environments get. What Epic adds is a login, a list of the games
your account owns, and installing them.

## Legendary

Legendary is GPL-3.0 software from its own project, not part of wineo. The
first time a command needs it, wineo offers to download one pinned release
from Legendary's GitHub releases and checks its size and SHA-256 before
using it:

```
$ wineo import epic --login
Epic needs Legendary 0.21.1 (3 MB, from its GitHub releases). Download it? [Y/n]:
Downloading Legendary 0.21.1...
Downloaded Legendary 0.21.1.
```

Answering no cancels with exit 130. Without a terminal, `--yes` agrees to
the download. `--dry-run` shows the question and downloads nothing.

The release is a Python zipapp, so it needs Python 3.10 or newer as
`python3`. Without it, commands that need Legendary stop with `Epic needs
Python 3.10 or newer to run Legendary.`

wineo looks for Legendary in this order:

1. `[epic] legendary` in the global config, if set:
   `wineo set --global epic.legendary=PATH`. A copy there that can't run,
   or is older than 0.21.1, is an error, not skipped.
2. wineo's own downloaded copy.
3. `legendary` on `PATH`, if it's 0.21.1 or newer. An older one is named
   when wineo offers its own download.

`wineo set --global --unset epic.legendary` goes back to the other two.
Plain `--list` and `--logout` never offer the download: they use the
saved list or remove the login without it.

## Logging in

```
$ wineo import epic --login
Log in to Epic in your browser. If it doesn't open, go to:
https://legendary.gl/epiclogin
Then paste what the page shows, or only its authorizationCode.
Code:
Logged in to Epic as rimworlder.
```

Log in, then paste the page Epic shows, or only its `authorizationCode`.
What you paste isn't shown, and it never appears in wineo's output or
logs. Logging in replaces any saved login, and needs a terminal.

The login lives in Legendary's own folder inside wineo's data,
`~/.local/share/wineo/epic/legendary`, never in your own Legendary or
Heroic setup. A `--portable` run uses the host's login, so logging in from
one saves it on the host.

`wineo import epic --logout` removes the login and the list of owned
games. It works without Legendary too. It's refused with `--portable`,
since Epic games live in the host library.

Bare `wineo import epic` logs you in, or lists your games once you are.

## Owned games

```
$ wineo import epic --list
ID       TITLE           STATE
Kestrel  Kestrel Rally   not installed
Lantern  Lantern Keeper  not installed
Harbor   Harbor Tactics  unavailable: needs an account at another launcher
```

`--list` shows what your account owns. A game is `not installed`,
`installed`, `unfinished` (an install didn't finish), `downloaded` (its
files are whole but no environment owns them, like one whose environment
is in the trash), or `unavailable`, with the reason: `another launcher
manages it`, `needs an account at another launcher`, or `no Windows build`.
Once wineo has a record of a listed game, installed or not, the table
adds ENVIRONMENT and VERSION columns. An empty library prints `You don't own any Epic games.`

The list is cached for an hour; `--refresh` asks Epic again. If Epic
can't send it, the saved list stands in with a warning. Plain `--list`
also falls back to it when Legendary or Python is missing; `--refresh`
offers to download Legendary instead, and stops without Python. Being
logged out is always an error, even with a saved list. See
[JSON output](json-output.md#epic) for `--json`.

## Installing games

```
$ wineo import epic --game Kestrel
Install Kestrel Rally (Kestrel)
  environment  kestrel-rally (new Windows environment)
  folder       ~/Games/Kestrel Rally
  runner       wine
  arch         win64
  version      1.4.2
  dlc          Kestrel Rally Night Tracks
  download     12.4 GiB plus 1 DLC
  installed    18.0 GiB plus 1 DLC
Esc or Ctrl+C to cancel.
Install it? [Y/n]:
Downloading 1 of 2: Kestrel Rally
Downloading 1 of 2: 37%
...
Installed Kestrel Rally as kestrel-rally.
```

`--game ID` asks Epic about the game, shows what it would install, and
asks first. The game goes in a new folder under
[`defaults.games_root`](config-files.md) (`~/Games` unless set), or in
`--dir PATH`, which has to be new or empty. `--runner` and `--arch` pick
the environment's runner, and a Proton environment gets `store = "egs"`.
The game and each DLC you own are separate runs, each checked against
Legendary's records when it ends. A DLC that fails is a warning, and the
game installs without it. The environment is created last, with
`epic_id` set to the game's id and a target for the program Legendary
names. When that program isn't inside the game folder, the environment
gets no target, and wineo says to pick one with `wineo setup NAME`.
`--yes` skips the questions, and is needed
without a terminal. `--dry-run` shows the review and installs nothing.

The review warns about what may keep a game from running: Easy
Anti-Cheat, an Epic ownership token (which usually means Denuvo), a
prerequisite installer wineo doesn't run, and a game that isn't marked to
run offline, which starts only while Epic answers. A game that needs an
account at another launcher, or that another launcher manages, isn't
installed. A download with no progress for five minutes says so and keeps
going.

### Interrupted installs

Ctrl+C stops Legendary and exits 130, keeping what it downloaded. The game
shows as `unfinished`, can't be started, and running the same
`wineo import epic --game ID` resumes it into the same folder:

```
Install interrupted.
Finish the interrupted install with: wineo import epic --game Kestrel
```

`wineo import epic --forget ID` drops wineo's and Legendary's records of
an unfinished or downloaded game and leaves its files where they are. It
refuses a game an environment owns, in the trash too. Nothing in wineo
deletes Epic game files.

## Starting games

`wineo start NAME` asks Legendary how to start the game, then runs the
program it names in the environment's prefix with the environment's
runner. The game gets its own parameters, then the target's saved
arguments, then the command line's, then Epic's. The desktop app shows
`Asking Epic...` meanwhile, which takes a second or two. Each start and
`restart` asks again, and a game with an update waiting still starts.
Other targets, tools, and `wineo exec` start without asking Epic.

Epic's arguments carry a one-use login for the game, so wineo keeps them
out of everything it saves or shows: the run's state, `restart`,
`status`, `--json`, dry runs, logs, and desktop entries. The running
game's own command line shows them to your other programs, as it does
under Epic's launcher, and so would a Proton debug log you turn on.

When Epic can't be asked, a game that's marked to run offline still
starts, with a notice:

```
Epic didn't answer, so kestrel-rally starts offline. Online features may not work.
```

Logged out, without Legendary or Python, or with an `[epic] legendary`
setting that can't be used, it starts from what wineo recorded at install, which lacks any extra arguments Epic's own answer
would add. A game that isn't marked to run offline refuses and says why,
like `kestrel-rally isn't marked to run offline, so it needs an Epic
login. Log in with wineo import epic --login.`

The main target's program comes from Epic, so `wineo set`, `setup`, and
the desktop app refuse to change it: `NAME's main program comes from Epic
and can't be changed.` Its arguments, working folder, name, wrappers,
hooks, and desktop entry can all be changed. A game's own pre-launch
command isn't run; the install review says when a game has one.

### A game that stops at its title screen

Some games log in to Epic themselves and may need you to agree to
something on Epic's site first, like new terms. The game opens a browser
tab for that, and waits until you've answered. If no tab opens, check
that the game can reach your desktop session, then start it again.
