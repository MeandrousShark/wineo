# JSON output

`status`, `list`, `validate`, `stats`, `import gog --list`, and `update
--check` accept `--json` for scripting:

```sh
wineo status hades --json
wineo status --json
wineo list --json
wineo validate --json
wineo stats --json
wineo import gog --list --json
wineo update --check --json
```

Each command writes one compact JSON document and a newline to stdout. Fields
use snake case, enum values are lowercase, optional values are `null`, arrays
remain arrays when empty, and timestamps use RFC 3339 with an offset. Saved
launch arguments and environment variables are never included.

## Named status

`wineo status <name> --json` returns one object. The common status fields are:

```json
{
  "name": "hades",
  "tags": ["game"],
  "state": "running",
  "lost": false,
  "busy": null,
  "unresolved": false,
  "run": {
    "target": "game",
    "mode": "detached",
    "pid": 1234,
    "started_at": "2026-09-23T12:00:00Z",
    "program_exited_at": null,
    "ended_at": null,
    "exit_code": null
  }
}
```

`tags` is the environment's tags, an empty array when it has none. `run` is `null` when there is no saved run. Its timestamps and exit code are
nullable. `busy` is a lowercase operation value such as `launch` or
`winetricks`. `state` reports runtime state independently of `busy` and
`unresolved`.

Named status also includes `kind`, `wine`, `native`, or `proxied`; `runner` with
`configured`, `resolved`, and `version`; `prefix`; nullable `target_exe`, `gameid`, and `store`; `gamemode`, `gamescope`, `mangohud`, and `wayland` (see [Launch options](launch-options.md));
`winetricks`, the configured verbs, and `winetricks_missing`, those not yet in
the prefix's `winetricks.log`, both arrays (see
[Winetricks dependencies](config-files.md#winetricks-dependencies));
`desktop`, an array of target names; and nullable `dxvk` and `vkd3d` component
objects. Each component object has `wanted`, nullable `resolved`, nullable
`installed`, and `current`.

`runner.configured` is the configured value. `runner.resolved` follows the
same precedence as human status: the active operation's runner, then the
runner saved with the last run, then resolution of the configured value. It
is `null` if no runner can be resolved.

Named status also includes `played`, which is `null` for an environment
that has never been played, and otherwise has `seconds`, `sessions`, and a
nullable `last_played`. See [Playtime](stats.md).

A [native environment](native.md) has no runner or prefix, so `runner` and
`prefix` are `null`, along with `gameid`, `store`, `dxvk`, and `vkd3d`.

A [Steam game](steam.md) has `"kind": "proxied"` and more fields:
`launcher` (`steam`), `app_id`, `install`, one of `installed`, `updating`,
`installing`, `install_queued`, `uninstalling`, `not_installed`,
`unavailable`, or `unknown`, `install_progress`, and `started_by`.
`install_progress` is a whole percent from 0 to 100 while the game is
installing and Steam knows its size, otherwise `null`; Wine and native
environments don't have it. `install` is separate from `state`, which stays
the run status, so a game can be `inactive` and `installing` at once. `started_by`
is `wineo` while a wineo run is starting or running, `steam` when the game
was started from the Steam client, and `null` when it isn't running.
`install_problem` gives the reason when there is one, like `Steam's library
at /mnt/games isn't available`. Other kinds leave these fields out. `runner`,
`prefix`, `target_exe`, `gameid`, `store`, `dxvk`, and `vkd3d` are `null`,
and `desktop` is `["steam"]` when the game has a desktop entry.

`state` can also be `starting`, while Steam is asked to launch the game and
it hasn't appeared yet, with a `null` `pid`. A game started from the Steam
client is `running`, with `"started_by": "steam"`. Its `played` is Steam's
total, with `sessions` `null`, since Steam keeps no sessions, and `seconds`
`null` when Steam's playtime can't be read.

## Collections

Nameless `status --json` and `list --json` return the same envelope:

```json
{
  "environments": [],
  "errors": []
}
```

Each item in `environments` contains the common status fields shown above,
plus `kind`, which is `null` for a config that can't be read, and a Steam
game's `launcher`, `app_id`, `install`, `install_progress`, `install_problem`,
and `started_by`.
`list` includes every environment whose runtime status can be read, and
`list --tag` only those with every given tag. Nameless `status` includes
running, busy, or unresolved environments. Both sort by environment name.
They read each config only for its tags, so a config that can't be read still
appears, with empty `tags`, unless `--tag` is given.

If a runtime status cannot be read for one environment, or with `--tag` its
config can't be read, it is omitted from `environments` and an error with its
`name` and `message` is added to `errors`.
The command still prints the complete partial result and exits with status 1.
Failures before a complete collection can be gathered, and failures of named
status, exit 1 with a diagnostic on stderr and no JSON on stdout. Clap
argument errors keep their normal stderr output.

## GOG

`wineo import gog --list --json` lists the games your GOG account owns:

```json
{
  "games": [
    {
      "id": "1094900565",
      "title": "RimWorld",
      "platforms": ["linux"],
      "state": "imported",
      "environment": "rimworld",
      "track": "installer",
      "version": "1.6.4871 rev588",
      "update": "available",
      "applicable": true,
      "reason": null,
      "dlc": [{"id": "1233017772", "title": "Royalty", "state": "unknown"}]
    }
  ],
  "errors": [],
  "warnings": []
}
```

`state` is `not_installed`, `heroic` (installed by Heroic, not migrated
yet), or `imported`. Only an imported game has `environment`, `track`
(`installer` or `galaxy`), `version`, `update`, and `dlc`; the rest have
`null`, `false` for `applicable`, and an empty `dlc`.

`wineo update --check --json` checks the environments that track a GOG
game:

```json
{
  "environments": [
    {
      "name": "rimworld",
      "id": "1094900565",
      "track": "installer",
      "installed": "1.6.4871 rev588",
      "available": "1.6.4900 rev600",
      "update": "available",
      "applicable": true,
      "reason": null,
      "dlc": [{"id": "1233017772", "title": "Royalty", "state": "available"}]
    }
  ],
  "errors": [],
  "warnings": []
}
```

In both, `update` is `available`, `none`, `galaxy` (an update on GOG that
a later release applies), or `cant_compare`. `applicable` says whether this
wineo can apply it, and `reason` says why not: `windows_installer`,
`galaxy`, or a sentence when the versions can't be compared. A DLC's
`state` is `up_to_date`, `available`, `unknown` (a migrated DLC whose version
Heroic never recorded), `cant_compare`, `failed`, or `unavailable`. GOG ids
are strings.

A game that can't be checked goes in `errors` with its `name` (the
environment, or `null`), `id` (or `null`), and `message`; the rest still
print and the command exits 1. `warnings` holds notes like an older saved
list standing in for one GOG couldn't send, and they also go to stderr.
A failure of `update --check NAME` exits 1 with a diagnostic on stderr and
no JSON, like named status.

## Validate

`wineo validate --json` reports every problem and never asks, even at a
terminal. With `--fix`, it applies every fix first and then reports what's
left:

```json
{
  "problems": [
    {
      "environment": "zomboid",
      "area": "target",
      "level": "error",
      "code": "exe_missing",
      "message": "target zomboid: C:\\Games\\Zomboid\\ProjectZomboid64.exe not found",
      "fix": null
    },
    {
      "environment": null,
      "area": "runtime",
      "level": "warning",
      "code": "state_dir_stray",
      "message": "state dir ~/.local/state/wineo/oldb has no environment",
      "fix": "Remove oldb's leftover state"
    }
  ],
  "fixed": ["Drop zomboid's missing icon record"],
  "errors": [
    {
      "fix": "Remove oldb's leftover state",
      "message": "environment `oldb` is busy: a snapshot operation is running"
    }
  ]
}
```

`environment` is `null` for the global config, templates, and anything left
behind by an environment that no longer exists. `level` is `error` or
`warning`. `fix` is the label of the fix that would solve the problem, or
`null` when there isn't one. Paths under the home dir are shown with `~`.

`code` names the check, so a script can match on it instead of the message.
The codes are stable:

| Area | Codes |
|---|---|
| `global_config` | `global_config_invalid` |
| `template` | `template_invalid`, `template_unresolved` |
| `environment_config` | `environment_config_invalid` |
| `wine` | `runner_missing`, `build_missing`, `prefix_missing`, `prefix_refused` |
| `target` | `exe_missing`, `cwd_missing`, `gamemode_missing`, `gamescope_missing`, `mangohud_missing`, `wayland_unsupported` |
| `native_target` | `native_exe_invalid` |
| `environment` | `target_ambiguous` |
| `runtime` | `lost_run`, `installer_unresolved`, `restore_interrupted`, `state_dir_stray`, `stats_stray` |
| `desktop` | `desktop_entry_stray`, `desktop_outdated`, `desktop_portable` |
| `steam` | `steam_not_found`, `steam_install_missing`, `steam_library_unavailable`, `steam_data_unknown` |
| `trash` | `trash_interrupted`, `trash_invalid` |
| `gog` | `gog_state_invalid`, `gog_state_missing`, `gog_folder_missing`, `gog_folder_taken`, `gog_mid_update`, `gog_owner_missing` |
| `art` | `art_config_invalid`, `art_file_missing`, `art_target_unknown`, `art_duplicates`, `art_dir_stray` |

A native target's working dir, gamemode, gamescope, and MangoHud problems use the `target` area.

`fixed` lists the fixes that were applied, and `errors` lists the ones that
failed, each with its `fix` label and a `message`. Both are always present,
and both are empty without `--fix`. A failed fix's problem stays in
`problems`.

The command exits 1 when any error-level problem is left or any fix failed,
and 0 otherwise. A failure before the checks finish exits 1 with a diagnostic
on stderr and no JSON on stdout.

## Stats

`wineo stats NAME --json` returns one object:

```json
{
  "name": "the-sims-4",
  "seconds": 18600,
  "sessions": 3,
  "average_seconds": 6200,
  "longest": { "seconds": 17460, "started_at": "2026-09-20T19:02:11Z" },
  "first_played": "2026-09-12T20:14:00Z",
  "last_played": "2026-09-26T10:00:00Z",
  "running": false,
  "imported_seconds": 0,
  "targets": [
    { "target": "game", "seconds": 17820, "sessions": 2 },
    { "target": "editor", "seconds": 780, "sessions": 1 }
  ]
}
```

Durations are whole seconds. `seconds` includes `imported_seconds`, the
total carried over from Heroic, while `sessions`, `average_seconds`, and
`longest` count only wineo's own sessions. `average_seconds`, `longest`,
`first_played`, and `last_played` are `null` when there's nothing to base
them on. A running session counts its time so far, and `running` is `true`.
`targets` is sorted by time played. `source` is `wineo` for Wine and native
environments.

A [Steam game](steam.md#playtime) has `"source": "steam"`. `seconds` and
`last_played` are Steam's, or `null` when Steam's playtime can't be read.
Steam keeps no sessions, so `sessions`, `average_seconds`, `longest`,
`first_played`, `this_week_seconds`, and `this_month_seconds` are `null`,
`targets` is empty, and `--sessions` adds no `sessions_log`.

`--sessions` adds `sessions_log`, the environment's finished sessions as
they're stored, for charts:

```json
"sessions_log": [
  {
    "target": "game",
    "started_at": "2026-09-17T19:00:00Z",
    "ended_at": "2026-09-17T22:00:00Z",
    "seconds": 10800,
    "end": "exited"
  }
]
```

`end` is `exited`, `failed`, `stopped`, or `lost`.

Nameless `wineo stats --json` returns the collection envelope, with every
environment in `environments`, played or not, sorted by name. An environment
whose log can't be read is left out and reported in `errors`, and the command
exits 1.

`wineo stats --tag TAG --json` returns the same envelope with only the
environments that have every given tag.

`wineo stats --by-tag --json` returns one row per tag:

```json
{
  "tags": [
    {
      "tag": "game",
      "seconds": 367800,
      "sessions": 15,
      "environments": 3,
      "played_environments": 2,
      "last_played": "2026-10-06T19:00:00Z"
    }
  ],
  "untagged": {
    "seconds": 10800,
    "sessions": 1,
    "environments": 1,
    "played_environments": 1,
    "last_played": "2026-09-17T22:00:00Z"
  },
  "errors": []
}
```

`tags` is sorted by `seconds`, then by tag. `seconds` includes time imported
from Heroic and Steam's totals, and `sessions` doesn't, as in the overview. `untagged` is `null`
when every environment has a tag. A config that can't be read is reported in
`errors`, and the command exits 1.
