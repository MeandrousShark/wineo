# JSON output

`status`, `list`, and `validate` accept `--json` for scripting:

```sh
wineo status hades --json
wineo status --json
wineo list --json
wineo validate --json
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

`run` is `null` when there is no saved run. Its timestamps and exit code are
nullable. `busy` is a lowercase operation value such as `launch` or
`winetricks`. `state` reports runtime state independently of `busy` and
`unresolved`.

Named status also includes `kind`, `wine` or `native`; `runner` with
`configured`, `resolved`, and `version`; `prefix`; nullable `target_exe`, `gameid`, and `store`; `gamemode`;
`desktop`, an array of target names; and nullable `dxvk` and `vkd3d` component
objects. Each component object has `wanted`, nullable `resolved`, nullable
`installed`, and `current`.

`runner.configured` is the configured value. `runner.resolved` follows the
same precedence as human status: the active operation's runner, then the
runner saved with the last run, then resolution of the configured value. It
is `null` if no runner can be resolved.

A [native environment](native.md) has no runner or prefix, so `runner` and
`prefix` are `null`, along with `gameid`, `store`, `dxvk`, and `vkd3d`.

## Collections

Nameless `status --json` and `list --json` return the same envelope:

```json
{
  "environments": [],
  "errors": []
}
```

Each item in `environments` contains the common status fields shown above.
`list` includes every environment whose runtime status can be read. Nameless
`status` includes running, busy, or unresolved environments. Both sort by
environment name and do not load environment configs.

If a runtime status cannot be read for one environment, it is omitted from
`environments` and an error with its `name` and `message` is added to `errors`.
The command still prints the complete partial result and exits with status 1.
Failures before a complete collection can be gathered, and failures of named
status, exit 1 with a diagnostic on stderr and no JSON on stdout. Clap
argument errors keep their normal stderr output.

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
| `target` | `exe_missing`, `cwd_missing`, `gamemode_missing` |
| `native_target` | `native_exe_invalid` |
| `environment` | `target_ambiguous` |
| `runtime` | `lost_run`, `installer_unresolved`, `restore_interrupted`, `state_dir_stray` |
| `desktop` | `desktop_entry_stray`, `desktop_outdated`, `desktop_portable` |
| `art` | `art_config_invalid`, `art_file_missing`, `art_target_unknown`, `art_duplicates`, `art_dir_stray` |

A native target's working dir and gamemode problems use the `target` area.

`fixed` lists the fixes that were applied, and `errors` lists the ones that
failed, each with its `fix` label and a `message`. Both are always present,
and both are empty without `--fix`. A failed fix's problem stays in
`problems`.

The command exits 1 when any error-level problem is left or any fix failed,
and 0 otherwise. A failure before the checks finish exits 1 with a diagnostic
on stderr and no JSON on stdout.
