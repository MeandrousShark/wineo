# Epic games

wineo talks to Epic through [Legendary](https://github.com/legendary-gl/legendary),
the open-source Epic client Heroic also uses. What Epic adds is a login and
a list of the games your account owns.

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
`--list` and `--logout` never offer the download: they use the saved list
or remove the login without it.

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
ID         TITLE           STATE
Kestrel    Kestrel Rally   not installed
Lantern    Lantern Keeper  not installed
Harbor     Harbor Tactics  unavailable: needs an account at another launcher
```

`--list` shows what your account owns. A game is `not installed` or
`unavailable`, with the reason: `another launcher manages it`, `needs an
account at another launcher`, or `no Windows build`. An empty library
prints `You don't own any Epic games.`

The list is cached for an hour; `--refresh` asks Epic again. If Epic
can't send it, the saved list stands in with a warning. Plain `--list`
also falls back to it when Legendary or Python is missing; `--refresh`
offers to download Legendary instead, and stops without Python. Being
logged out is always an error, even with a saved list. See
[JSON output](json-output.md#epic) for `--json`.
