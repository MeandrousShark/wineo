# JSON output

`status` and `list` accept `--json` for scripting:

```sh
wineo status hades --json
wineo status --json
wineo list --json
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

Named status also includes `runner` with `configured`, `resolved`, and
`version`; `prefix`; nullable `target_exe`, `gameid`, and `store`; `gamemode`;
`desktop`, an array of target names; and nullable `dxvk` and `vkd3d` component
objects. Each component object has `wanted`, nullable `resolved`, nullable
`installed`, and `current`.

`runner.configured` is the configured value. `runner.resolved` follows the
same precedence as human status: the active operation's runner, then the
runner saved with the last run, then resolution of the configured value. It
is `null` if no runner can be resolved.

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
