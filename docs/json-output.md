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

## Compatible changes

These outputs are part of wineo's [stable contract](stability.md). A field
keeps its name, type, and meaning. Within a release series:

- New fields can appear, in any object. Scripts should ignore fields they
  don't know.
- Enum fields can gain values, so scripts should handle a value they don't
  know, for example by treating it as "other".
- Renaming a field, removing it, changing its type, or changing what a value
  means is a breaking change, and waits for the next major version.
- Validate's `code` values are stable, so new checks get new codes rather
  than reusing old ones.

Nothing in the output carries a version number; these rules are the
guarantee.

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

`tags` is the environment's tags, an empty array when it has none. `run` is
`null` when there is no saved run. Its timestamps and exit code are nullable,
and `mode` is `detached` or `attached`.

`state` reports runtime state independently of `busy` and `unresolved`:
`running`, `starting`, `exited`, `failed`, `stopped`, or `inactive` (never
started). `busy` is `null`, or the operation holding the environment:
`create`, `remove`, `winetricks`, `configure`, `launch`, `install`,
`cleanup`, `snapshot`, `copy`, `trash`, `migrate`, `update`, `hook-before`,
or `hook-after`.

`hooks` is `{"before": true, "after": false}`, saying which
[launch hooks](hooks.md) are set, or `null` when neither is. The commands
themselves are never included.

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
including `hooks`, plus `kind`, which is `null` for a config that can't be read, and a Steam
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
      "branch": null,
      "pinned": null,
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

`platforms` holds `windows`, `mac`, and `linux`, the ones GOG sells the
game for. `state` is `not_installed`, `heroic` (installed by Heroic, not
migrated yet), `imported` (migrated from Heroic), or `installed` (installed
by wineo). Only an imported or installed game has `environment`,
`track` (`installer` or `galaxy`), `version` (the installer version, or the
build id on the Galaxy track, where the table shows GOG's name for it),
`update`, and `dlc`; the rest
have `null`, `false` for `applicable`, and an empty `dlc`. An imported game
whose update check failed, or any game in a [portable folder](portable.md),
where the check is skipped, has its `environment` but `null` for the rest.

`wineo update --check --json` checks the environments that track a GOG
game:

```json
{
  "environments": [
    {
      "name": "rimworld",
      "id": "1094900565",
      "track": "installer",
      "branch": null,
      "pinned": null,
      "installed": "1.6.4871 rev588",
      "available": "1.6.4900 rev600",
      "update": "available",
      "applicable": true,
      "reason": null,
      "unfinished": false,
      "dlc": [{"id": "1233017772", "title": "Royalty", "state": "available"}]
    }
  ],
  "errors": [],
  "warnings": []
}
```

`installed` and `available` are installer versions, or build ids on the
Galaxy track (the table shows GOG's names for those builds), and `null`
when they aren't known. `unfinished` is `true` when an update or install
was interrupted and has to be finished before the game starts: with
`wineo update NAME`, or for a Galaxy game's first install, by installing
it again with `wineo import gog --game ID`. `import gog --list --json` has
it too, `false` for games that aren't imported.

In both, `branch` is the Galaxy branch the game follows, and `null` for
the default branch and on the installer track. `pinned` is the build id
the game is [pinned](gog.md#branches-and-pinned-builds) to, or `null`.

In both, `update` is `available`, `none`, `galaxy`, or `cant_compare`.
`galaxy` means a Galaxy game GOG lists no generation 2 build of on the
branch it follows, which wineo can't install; `wineo update NAME --reinstall --track installer`
moves it to offline installers. `applicable` says whether this wineo can
apply the update, Galaxy updates included, and `reason` says why not:
`galaxy`, `pinned` (a newer build waits on the branch of a pinned game,
which `wineo update NAME --latest` installs), or a sentence when the
versions can't be compared, including when a private branch's password
is missing. A DLC's `state` is `none` (up to
date), `available`, `unknown` (a migrated DLC whose version Heroic never
recorded), `cant_compare`, `failed`, `unavailable`, or
`not_installed` (owned and offered, but not installed yet: bought since, or
newly offered for the platform; `wineo update` installs it), and its
`title` is `null` when GOG's details don't list it. GOG ids are strings.

A game that can't be checked goes in `errors` with its `name` (the
environment, or `null`), `id` (or `null`), and `message`; the rest still
print and the command exits 1. `warnings` holds notes like an older saved
list standing in for one GOG couldn't send, and they also go to stderr.
A failure of `update --check NAME` exits 1 with a diagnostic on stderr and
no JSON, like named status.

`wineo update NAME --builds --json` lists the builds GOG offers for a
Galaxy game:

```json
{
  "environment": "papers-please",
  "id": "1207659209",
  "track": "galaxy",
  "branch": null,
  "pinned": null,
  "installed": "56353505673715002",
  "private_branches": false,
  "builds": [
    {
      "branch": null,
      "build": "56353505673715002",
      "version": "1.4.11.124",
      "published": "2023-03-22T06:14:05+0000",
      "latest": true,
      "private": false
    },
    {
      "branch": "Legacy (64-bit)",
      "build": "55257829924550446",
      "version": "1.2.76",
      "published": "2022-03-09T16:04:40+0000",
      "latest": true,
      "private": false
    }
  ]
}
```

`branch` and `pinned` say what the game follows, as above, and
`installed` is its base game's build id, or `null` when it isn't known. `private_branches` is `true`
when GOG says the game has branches only a password lists. `builds` has
the default branch first (`branch` is `null`), then the others, each
newest first. `version` is GOG's name for the build, or `null`;
`published` is GOG's timestamp as given, or `null` when GOG gives none; `latest` marks the build an
update on that branch goes to; and `private` marks a build listed with a
saved password. A game on the installer track has no builds to list: it
exits 1 with a diagnostic on stderr and no JSON.

## Validate

`wineo validate --json` reports every problem and never asks, even at a
terminal. With `--fix`, it applies every fix first and then reports what's
left. The exception is an interrupted snapshot restore
(`restore_interrupted`), which `wineo snapshot recover` finishes; `--fix`
leaves it in `problems`, with its `fix` label, and lists it in neither
`fixed` nor `errors`:

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
| `config` | `deprecated_key` |
| `global_config` | `global_config_invalid` |
| `template` | `template_invalid`, `template_unresolved` |
| `environment_config` | `environment_config_invalid` |
| `wine` | `runner_missing`, `build_missing`, `prefix_missing`, `prefix_refused`, `winetricks_missing`, `winetricks_unavailable` |
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
  "source": "wineo",
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
Steam keeps no sessions, so `sessions`, `average_seconds`, `longest`, and
`first_played` are `null`, `targets` is empty, and `--sessions` adds no
`sessions_log`.

This week's and this month's totals, which `wineo stats NAME` shows, aren't
in the JSON for any environment: they depend on the local time zone and the
current time, and `sessions_log` has what a script needs to work them out.

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
