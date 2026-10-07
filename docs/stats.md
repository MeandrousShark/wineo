# Playtime

```sh
wineo stats
wineo stats the-sims-4
wineo stats --import-heroic
wineo stats --tag game
wineo stats --by-tag
```

Wineo keeps a log of every time a target runs, so you can see how much each
game gets played. Nothing needs turning on.

## The overview

`wineo stats` lists every played environment, most played first:

```
$ wineo stats
                played      sessions  last played
cyberpunk-2077  97 h               0  Sep 4
the-sims-4      5 h 10 m           3  today
palworld        3 h                1  Sep 17
1 never played. Total 105 h 10 m.
```

A game that's running now counts its time so far, and its last played day
is marked `(running)`.

## By tag

`--tag` limits the overview to environments with that tag, like `list
--tag`. Give it more than once to require every tag. The total and the
"never played" count cover only those environments:

```
$ wineo stats --tag game
         played    sessions  last played
hades    97 h            12  Sep 4
celeste  5 h 10 m         3  today
1 never played. Total 102 h 10 m.
```

When nothing has the tag, it prints `No environments are tagged game` and
exits 0.

`--by-tag` shows one row per tag, most played first, with untagged
environments last:

```
$ wineo stats --by-tag
            played      sessions  played by  last played
game        102 h 10 m        15  2 of 3     today
co-op       97 h              12  1 of 1     Sep 4
work        0 m                0  0 of 2
(untagged)  3 h                1  1 of 1     Sep 17
An environment with several tags counts under each.
```

`played by` is how many of the tag's environments have been played, out of
how many have it. An environment with several tags counts under each one, so
the rows can add up to more than the real total; the last line says so when
that happens. `--by-tag --tag game` only counts environments tagged `game`,
which shows how they split across their other tags.

Both flags read each environment's config for its tags, so a config that
can't be read is reported and the command exits 1. Plain `wineo stats` never
reads configs. See [Tags](config-files.md#tags) for setting them.

## One environment

```
$ wineo stats the-sims-4
the-sims-4
  played      5 h 10 m over 3 sessions
  average     1 h 43 m
  longest     4 h 51 m, Sep 20
  first       Sep 12
  last        today, 13 m
  this week   13 m
  this month  5 h 10 m
  targets     game 4 h 57 m (2), editor 13 m (1)
```

`targets` only shows when more than one target has been played. `wineo
status NAME` also shows a one-line summary:

```
  played     5 h 10 m over 3 sessions, last today
```

## What counts

Every target run counts as a session: `start`, `run`, and each half of a
`restart`, for Wine and [native](native.md) environments alike. A run that
crashes or is stopped still counts, however short. `exec`, `winetricks`,
installers, and imports don't count.

A session lasts from the launch until the run ends, which is when nothing is
left running in the prefix, not when the launched exe exits. That's right for
launchers and wrapper scripts, which exit while the game keeps going. A helper
that stays in the tray after the game closes, like the EA app, keeps the run
going until it exits or you run `wineo stop`. Idle time with the game open
counts as played.

If wineo dies while a game is running, say in a crash or a reboot, the run is
lost. The supervisor notes the time once a minute while a game runs, so a lost
session ends at the last note instead of whenever the lost run gets cleaned
up. It's added to the log when `wineo stop` or `wineo validate` cleans the run
up.

## Heroic playtime

Heroic counts playtime too. `import heroic` carries a game's Heroic total and
last played date over, and `wineo stats --import-heroic` does the same for
every environment that was already imported from Heroic:

```
$ wineo stats --import-heroic
cyberpunk-2077: 97 h from Heroic
```

Heroic's time counts toward the total but not toward sessions or averages,
and `stats NAME` shows how much of the total it is. Importing again uses
Heroic's current total in place of the old one, so time played through Heroic
since then is picked up without counting anything twice. If an environment
can't be updated, `--import-heroic` warns, goes on with the rest, and exits
1.

## Where it's kept

Each environment's log is `$XDG_DATA_HOME/wineo/stats/<name>.jsonl`, or
`stats/<name>.jsonl` in a [portable folder](portable.md). It's in the data
dir rather than the state dir, since `~/.local/state` is often left out of
backups and playtime can't be rebuilt.

The log is one JSON object per line, one per session:

```json
{"target":"game","started_at":"2026-09-20T19:02:11Z","ended_at":"2026-09-20T23:53:11Z","seconds":17460,"end":"stopped"}
```

`end` is `exited`, `failed`, `stopped`, or `lost`. Heroic's time is an
`{"imported": {"seconds": ..., "last_played": ...}}` line. Each import adds a
new one and only the last counts, so the log is never rewritten and a session
finishing at the same moment can't be lost. A line that doesn't parse, like
one cut off by a crash, is skipped with a warning and the rest of the log
still counts.

- `wineo rm` retains the playtime log in trash with the config and art; see
  [Trash and removal](trash.md).
- `wineo clone` starts the clone at zero, since it's a new install to play.
- `wineo portable copy` carries the log, like art.
- Snapshots leave it alone, so a restore never rolls back playtime.
- `wineo validate` warns about a log with no environment and offers to remove
  it.

`--json` output is described in [JSON output](json-output.md#stats).
