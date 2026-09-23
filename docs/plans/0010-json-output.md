Status: approved

# 0010: JSON output for status and list

## Goal

Make environment state usable from scripts without parsing aligned columns or
relative times. Add JSON to `status` and `list` while keeping their current
human output and failure behavior.

## Decisions

Settled by the maintainer while drafting:

- Cover `status <name>`, `status` without a name, and `list`. Other commands
  stay outside this plan.
- Include useful status detail, but never serialize saved launch arguments or
  environment variables into this output.
- Collection output includes per-environment errors in JSON and exits with
  failure if any environment could not be inspected.

The maintainer approved the output contract below.

## Existing foundation

- `EnvironmentReport` in `wineo-core/src/report.rs` already gathers and
  serializes named status, but its nested `Status.run` serializes the persisted
  `RunState`, including launch arguments and environment variables. JSON must
  use a safe projection instead of serializing that report directly.
- `list` and nameless `status` share `wineo-cli/src/commands/table.rs::gather`.
  It reads environment names and runtime state without parsing configs. A
  broken config still appears in `list`, as required by 0001.
- The current collection commands skip a runtime status that cannot be read,
  show the other rows, and exit with failure. Named `status` fails directly.
- `serde_json` is a workspace dependency and is used by core, but the CLI does
  not yet depend on it.

## Command and output contract

Add `--json` to the `status` and `list` subcommands:

```text
wineo status <name> --json
wineo status --json
wineo list --json
```

The flag is command-specific. Without it, existing output stays the same.
JSON is one complete document on stdout, followed by a newline. It contains
no color, progress text, relative times, or terminal formatting. Optional
values are `null` and collections are always arrays. Timestamps use RFC 3339
strings with an offset. Field names and enum values use lowercase snake case.

`status <name> --json` emits one object. It includes:

- `name`, `state`, `lost`, `busy`, and `unresolved`.
- `run`, either `null` or an object with `target`, `mode`, `pid`,
  `started_at`, `program_exited_at`, `ended_at`, and `exit_code`.
- `runner`, an object with `configured`, `resolved`, and `version`, plus
  `prefix`, `target_exe`, `gamemode`, `gameid`, `store`, `dxvk`, `vkd3d`, and
  `desktop`. The component objects retain `wanted`, `resolved`, `installed`,
  and `current`.

`state` is the runtime state such as `running` or `inactive`. `busy` names an
operation separately, and `unresolved` is a separate boolean; neither
replaces the runtime state. `runner.configured` is the value in the config,
and `runner.resolved` is the selected installed build, or `null` when it
cannot be resolved. No field contains a human display string such as
`latest -> GE-Proton11-1`.

Collection commands emit the same envelope, including when empty:

```json
{
  "environments": [
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
  ],
  "errors": []
}
```

`list --json` includes every environment whose runtime status can be read.
`status --json` includes running, busy, or unresolved environments, matching
its current filter. Both retain sorted environment names. They do not parse
configs or load the full named report. A runtime read failure adds
`{"name":"bad","message":"..."}` to `errors`, skips that environment,
and makes the command exit 1. No duplicate diagnostic goes to stderr for a
failure already represented in the JSON document.

A failure before a complete collection can be gathered, or a failure of
`status <name> --json`, exits 1 with a diagnostic on stderr and no JSON on
stdout. Clap argument errors retain Clap's existing stderr behavior.

## Implementation

1. Add the command flags and CLI JSON dependency.
2. Add explicit serializable output types for the safe common state and named
   detail. Build them from runtime/report data without passing `RunState`
   directly to the serializer. Keep internal process identity and saved launch
   options out of the public JSON schema.
3. Let the shared collection path return structured rows and per-name errors.
   Keep human rendering and diagnostics unchanged; render JSON from the same
   gathered state without a second runtime probe.
4. Document the commands and examples. Once accepted, update the feature
   roadmap. Plan 0009's status and its original manual acceptance remain a
   separate tracking item.

## Acceptance

- Parse JSON from named status, list, and nameless status in CLI tests. Cover
  inactive and running states, empty collections, stable ordering, busy and
  unresolved state, and a failing environment alongside a healthy one.
- Prove list still includes an environment with a broken config, and a
  collection with one unreadable runtime emits partial results plus an error
  and exits 1.
- Prove saved arguments, environment variables, and internal process identity
  do not appear in JSON. Check timestamps, nulls, and unstyled stdout.
- Verify the unchanged human output and named error behavior.
- Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo test`. Run the ignored real-Wine tests when their local fixtures are
  available; otherwise record why they were skipped.
- Complete the independent QA deferred by plan 0011, including its prompt-key
  changes, before marking this plan done.
