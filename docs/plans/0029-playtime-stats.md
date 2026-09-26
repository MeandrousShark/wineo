Status: draft

# 0029: Playtime statistics

## Goal

Track how much each game gets played: launch count, total and per-session
time, and when it was first and last played. Show it in the CLI now and keep
it in a shape the GUI library can sort and chart later. This is GUI entry
criterion 3, moved up ahead of trash and `prune`.

## Existing foundation

- `RunState` in `runtime.rs` (`state.toml`) already records `target`, `mode`,
  `started_at`, `program_exited_at` (only when the run can outlive the
  program), `ended_at`, `exit_code`, and `status` (`exited`, `failed`,
  `stopped`). It only holds the latest run, so there's no history today.
- `Supervisor::wait` in `supervisor.rs` owns every target run, attached
  (`run`) or detached (`start`, `restart`, via `wineo __supervise`), Wine and
  native alike. It calls `state.finish(status)` and writes the final state,
  so it's the one place a finished run passes through.
- A run whose supervisor died is a lost run. `Runtime::clean_up_lost_run`
  (moved to core in 0028) finishes it as `stopped` with `ended_at` set to the
  cleanup time, which can be days after the game really ended.
- `exec`, `winetricks`, setup installers, and imports run programs through
  `operation.toml`, not as target runs.
- Art (0021) is the model for per-environment data: `Paths::art_dir` puts it
  under `$XDG_DATA_HOME/wineo/art/<env>`, or `<folder>/art/<env>` in a
  portable folder. `rm` removes it, `clone` and `portable copy` copy it, and
  snapshots leave it alone. 0028's validate warns about an art dir with no
  environment.
- `status` and `list` share `table::gather`, and both have `--json`
  (`docs/json-output.md`).
- Heroic keeps playtime in `~/.config/heroic/store/timestamp.json`, keyed by
  app ID: `totalPlayed` in minutes, plus `firstPlayed` and `lastPlayed` as
  RFC 3339 strings that are often empty. Imported environments record
  `heroic_id`.

## Proposed decisions for maintainer approval

### 1. What counts as a session

Every target run: `start`, `run`, and each half of a `restart`, for Wine and
native environments. Stopped and failed runs count too, since a crash an
hour in is still an hour played. `exec`, `winetricks`, installers, and
imports don't count.

Recommended: count every run, however short, and show them all. A launch
that crashes in two seconds is still a launch, and hiding short ones means
picking a cutoff.

### 2. When a session ends

The run lasts until the prefix goes idle, which can be well after the
launched program exits. Which end marks the session?

**A. When the run ends (recommended)**

`started_at` to `ended_at`. Right for launchers and wrapper scripts, where
the launched exe exits and the game keeps going. Adds wineserver's few
seconds of linger to each Wine session.

```
launcher.exe  |--|
game.exe         |==============================|
counted       |=================================|
```

**B. When the launched program exits**

`started_at` to `program_exited_at`. Right when something like the EA app
lingers in the tray after the game closes, but it counts a launcher run as
a few seconds.

```
launcher.exe  |--|
game.exe         |==============================|
counted       |--|
```

A is right far more often, and a lingering helper still ends when you run
`wineo stop`. B would need a per-target setting to be right for launchers,
which is a config change for a later plan if A turns out wrong.

### 3. Lost runs

When the supervisor dies (a crash, a reboot with the game open), nobody saw
the run end. Using the cleanup time would count all the time the machine was
off.

**A. Heartbeat (recommended)**

The supervisor stamps `heartbeat_at` in `state.toml` once a minute while the
run is live. A lost run's session ends at its last heartbeat, so it's at most
a minute short, and it's marked `lost` in the log. It's recorded when the
lost run is cleaned up (`stop`, `validate`, or whatever already does it).

**B. Count the launch, not the time**

A lost run adds a session with no duration. No extra writes, but a reboot
mid-game loses that whole session's time.

One small `state.toml` write a minute is nothing next to what the
supervisor already does, and lost playtime can't be rebuilt, so A.

### 4. Storage

Recommended: an append-only session log per environment,
`$XDG_DATA_HOME/wineo/stats/<env>.jsonl`, or `<folder>/stats/<env>.jsonl` in
a portable folder. One JSON object per line:

```json
{"target":"game","started_at":"2026-09-26T11:47:02-07:00","ended_at":"2026-09-26T14:02:40-07:00","seconds":8138,"end":"stopped"}
```

`end` is `exited`, `failed`, `stopped`, or `lost`. Totals, averages, and
per-target numbers are computed on read, so nothing has to stay in sync.

It's in the data dir, not state, since people often skip `~/.local/state`
in backups and playtime can't be rebuilt. It's a log rather than a TOML
file because appending one line is crash-safe without a read-modify-write
or a lock, and a year of daily play is well under 100 KB. It's not a config,
so there's no `schema_version`. Unknown fields are ignored and a line that
doesn't parse is skipped with a warning, so a torn last line never loses the
rest.

### 5. Where it shows

**A. A `wineo stats` command, plus a line in `status` (recommended)**

```
$ wineo stats
                  played    sessions  last played
cyberpunk-2077    107 h 19 m      42  Sep 4
the-sims-4        18 h 3 m        11  today
palworld          9 h 40 m         6  Sep 17
dusklight         1 h 12 m         3  yesterday
3 never played. Total 136 h 14 m.

$ wineo stats the-sims-4
the-sims-4
  played      18 h 3 m over 11 sessions
  average     1 h 38 m
  longest     4 h 51 m, Sep 20
  first       Sep 12
  last        today, 2 h 15 m
  this week   6 h 2 m
  this month  18 h 3 m
  targets     game 17 h 50 m (9), editor 13 m (2)

$ wineo status the-sims-4
the-sims-4: running the-sims-4 since 11:47 (1 h ago)
  ...
  played    18 h 3 m over 11 sessions, last today
```

The overview sorts by time played. `targets` only shows when more than one
target has sessions. A running session counts toward the numbers so far and
is marked `(running)`.

**B. Only in `status` and `list`**

A `played` column in `list` and the line in `status`, with no new command.
Less to learn, but no room for the per-target and longest-session detail,
and `list` gets wider.

### 6. `--json`

`wineo stats --json` returns a collection like `list`, and
`wineo stats <name> --json` one object. Durations are whole seconds and
times are RFC 3339, as elsewhere:

```json
{
  "name": "the-sims-4",
  "seconds": 64980,
  "sessions": 11,
  "average_seconds": 5907,
  "longest": { "seconds": 17460, "started_at": "2026-09-20T19:02:11-07:00" },
  "first_played": "2026-09-12T20:14:00-07:00",
  "last_played": "2026-09-26T11:47:02-07:00",
  "running": true,
  "imported_seconds": 0,
  "targets": [
    { "target": "game", "seconds": 64200, "sessions": 9 },
    { "target": "editor", "seconds": 780, "sessions": 2 }
  ]
}
```

Named `status --json` gains a nullable `played` object with `seconds`,
`sessions`, and `last_played`. A `--sessions` flag on `stats <name> --json`
adds the raw log for the GUI's charts.

### 7. Carrying over Heroic's playtime

Heroic already counted time for imported games, often a lot of it.

**A. Import it as a starting total (recommended)**

`import heroic` records Heroic's `totalPlayed` and `lastPlayed` as one
`imported` line at the top of the log, and `wineo stats --import-heroic`
does the same for environments already imported (they have `heroic_id`).
It counts toward the total but not sessions or averages, and `stats <name>`
shows it: `played 107 h 19 m over 42 sessions, 95 h from Heroic`.
Importing again replaces that line with Heroic's current total, so time
played through Heroic since then is picked up without counting anything
twice.

**B. Start everything at zero**

Simpler, but the games with the most history show the least.

### 8. Lifecycle

- `rm` deletes the log, like art, and says `Removed play stats`.
- `clone` starts the new environment at zero. It's a new install to play,
  and copying the log would count the same hours twice.
- `portable copy` carries the log, like art, since it's the same game
  moving.
- Snapshots leave it alone, so a restore never rolls back playtime.
- `validate` warns about a log with no environment (`stats_stray`, with a
  remove fix), like `art_dir_stray`.
- No backfill from `state.toml`. It holds only the latest run, so it would
  add one session at most.

## Implementation sequence

1. Core: a `stats` module with the session record, the append and read side,
   and the summary math (totals, average, longest, first and last, this week
   and month, per target). `Paths::stats_log`. The supervisor appends a
   session when a run finishes. The heartbeat, and lost-run cleanup
   appending a `lost` session.
2. CLI: `wineo stats [NAME]` with `--json` and `--sessions`, the `played`
   line in `status` and its JSON. rm, clone, and portable copy handling, and
   the validate check.
3. Heroic: the `imported` line from `import heroic`, and
   `wineo stats --import-heroic` for existing environments.
4. Docs: a `docs/stats.md` page, `json-output.md` sections, README row,
   `features.md` completed line and criteria, and notes in `clone.md`,
   `portable.md`, `snapshots.md`, and `validate.md`.

## Verification and acceptance

- Unit tests for the summary math, including a running session, an empty
  log, a torn last line, and an `imported` line.
- Supervisor tests: an exited, a failed, and a stopped run each append one
  session with the right `end`. A lost run cleaned up after a gap ends at its
  last heartbeat.
- CLI tests: `stats` with no plays, the overview sort, one environment, and
  the JSON shapes. rm deletes the log, clone doesn't copy it, portable copy
  does, and a snapshot restore leaves it alone.
- Heroic tests against a fixture `timestamp.json`, including empty dates and
  importing twice.
- The maintainer plays something for real, checks `wineo stats`, imports
  Heroic playtime for existing games, and reboots mid-game once to see the
  lost run counted.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`.

## Scope limits

- No achievements, streaks, or charts in the CLI. The log keeps enough for
  the GUI to draw them later.
- No idle detection. Time with the game open counts as played.
- No per-target session-end setting (decision 2 B).
- No sync of playtime back to Heroic.
