# Config file versioning

Global config, environment configs, and templates carry a top-level
`schema_version` so a file can outlive the wineo version that wrote it.
Snapshot manifests and internal metadata (component installs, runtime
records) have their own separate formats and are not covered here.

```toml
schema_version = 1
prefix = "~/prefixes/notepad"
runner = "wine-tkg"
arch = "win64"
```

## Which files are versioned

- `config.toml` (global config)
- `environments/<name>.toml`
- `templates/<name>.toml`, including the built-in `gaming` template

## Legacy files

A missing `schema_version` means a legacy, unversioned file. Wineo reads it
with the current parser and does not touch it just for reading: no bytes and
no modification time change. Wineo adds `schema_version = 1` only when it
also makes a real change to the file, in that same atomic write. A no-op edit
leaves a legacy file exactly as it is.

User templates have no wineo edit command, so a legacy template stays
unversioned until the user edits it themselves or a future command writes it.

## Unsupported versions

`schema_version = 0` written explicitly, a version newer than 1, or a value
of the wrong type are all refused with:

```
unsupported schema_version 2 in /home/user/.config/wineo/config.toml
```

Wineo never rewrites or downgrades a version it doesn't recognize, and it
never falls back to defaults for a config in this state. `wineo status`
prints this as a warning rather than failing outright, since the global
config only affects its runner version line.

## Snapshots

Restoring a snapshot brings back the exact config text that was saved,
version key and all. A snapshot of a legacy config restores as legacy, and
only gets `schema_version = 1` on its next edit.
