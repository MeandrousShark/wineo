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

## Creating an environment from a template

Wineo creates prefixes itself, so an environment config written by hand
has no prefix behind it, and `wineo start` refuses it. To describe an
environment in a file, write a template instead and create from it. A user
template lives in `templates/<name>.toml` in the config directory and can
set:

```toml
schema_version = 1
runner = "proton"      # a runner name, or "proton" for the newest Proton installed
arch = "win64"         # or "win32"
gamemode = true
dxvk = "latest"        # left out on Proton, which bundles its own
vkd3d = "latest"
gameid = "umu-12345"
store = "steam"

[env]
DXVK_HUD = "fps"
```

Then create an environment from it, optionally running an installer:

```sh
wineo new -n palworld -t mygame --install-exe ~/Downloads/setup.exe
```

Wineo builds the prefix with the template's runner and arch, then writes
the rest of its settings into `environments/palworld.toml`. Flags like
`--runner` and `--arch` win over the template, and a template without a
runner uses the global default. `wineo templates` lists the templates, and
the built-in `gaming` template can be shadowed by a file of the same name.
Targets come from the installer, `wineo setup`, or `wineo set` afterwards.

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

## art.toml

Each environment's art store keeps its own `art/<env>/art.toml`, versioned
under the same `schema_version = 1` rules. It is not part of the
environment config, so adding art does not bump that config's schema.

A top-level `game` holds the SteamGridDB game ID once picked. Each kind is a
table with `file`, the stored file name, and exactly one source key:
`steamgriddb` with the asset ID, or `manual` with the original file name.
Target overrides live in the same file as `[targets.<target>.<kind>]` with
the same fields, so the game ID is shared across the environment and its
targets:

```toml
schema_version = 1
game = 5247

[icon]
file = "icon.png"
steamgriddb = 36952

[grid]
file = "grid.jpg"
manual = "poster final.jpg"

[targets.server.icon]
file = "icon.svg"
manual = "server.svg"
```

Unknown keys are refused, like other configs, and a bad value invalidates
the whole file. wineo edits art.toml in place, so hand-added comments
survive. See [Artwork](art.md) for the commands that read and
write it.
