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

## Creating an environment by hand

Write `environments/<name>.toml` yourself, or restore one from a dotfiles
repo, and `wineo start`, `run`, `exec`, `winetricks`, or `restart` builds the
missing prefix from the config's `runner` and `arch` before it does anything
else. `docs/environment.toml.example` in the repo (installed to
`/usr/share/doc/wineo/environment.toml.example` by the Arch package) is a
commented starting point with every key.

```
$ wineo start palworld
Creating a Proton prefix for palworld in ~/Games/prefixes/palworld, this takes a while
Started palworld
```

The config file itself is never written; DXVK, vkd3d, and desktop entries
come from the normal start that follows. Checks that fail fast, like an
unknown target or winetricks missing from PATH, still run before the build.
`programs`, `set`, `clone`, `snapshot`, and `portable copy` keep refusing a
missing prefix, since they read or copy one and a fresh empty prefix gives
them nothing to work with. `rm` can remove a registration without a prefix;
see [Trash and removal](trash.md).

Building is refused when the prefix path isn't safely empty, mainly to avoid
hiding a real prefix on an unmounted drive:

| What's at the prefix path | Result |
|---|---|
| A prefix of the runner's own layout | Nothing to build, already there |
| A prefix of the other layout (Wine vs. Proton) | Refused |
| Missing, but its parent exists and isn't empty | Built |
| Missing, under wineo's prefix root | Built, making parent dirs if needed |
| Missing, outside the prefix root, and its parent is missing too | Refused |
| Missing, outside the prefix root, and its parent is empty | Refused, with a hint to use `wineo setup` |
| An empty dir under the prefix root | Built in it |
| An empty dir outside the prefix root | Refused, with a hint to use `wineo setup` |
| Anything else non-empty | Refused as not a prefix |

An empty dir looks exactly like an unmounted drive's mount point, so those
rows with a hint are left to `wineo setup`, which asks first. It offers to build any
missing prefix that isn't refused outright:

```
Its prefix doesn't exist yet.
  1) Skip for now

Press Enter to create it, or pick a number. Ctrl+C to cancel.
Choice:
```

Enter builds it and moves on to the setup menu with the new prefix's
programs; `Skip for now` leaves things as they are. When the path itself is
refused (the wrong prefix layout, say), `setup` prints the reason and
continues with no offer.

## Native environments

A top-level `kind = "native"` makes an environment launch a Linux program
directly, with no prefix or runner. A config without `kind` is a Wine
environment, so adding the key needed no schema bump: an older wineo refuses
a native config with `unknown field kind` rather than misreading it, and Wine
configs never get the key written. `docs/native-environment.toml.example`
(installed next to the Wine example) lists every key a native config can
hold. See [Native Linux games](native.md).

## Launch options

`mangohud`, `wayland`, and `[gamescope]` needed no schema bump either, for
the same reason: an older wineo refuses a config using them with
`unknown field` instead of launching without them. They can also go in a
template. See [Launch options](launch-options.md).

## Tags

`tags` groups environments, like `game`, `work`, or `creative`. Tags use the
same characters as environment names (letters, digits, `-`, and `_`), match
exactly, and can't repeat. They work on Wine and native environments and in
templates, and needed no schema bump for the same reason as the launch
options.

The GUI uses **Play** for the exact `game` tag and **Launch** otherwise. This
changes the label, not launch behavior. Future creations from the built-in
`gaming` template receive the tag; existing environments are not retagged.

```sh
wineo set hades 'tags=["game", "co-op"]'
wineo list --tag game
wineo list --tag game --tag co-op   # both tags
wineo set hades --unset tags
```

Several `--tag` flags list only environments with all of them. When nothing
matches, `list` prints `No environments are tagged game` and exits 0.
`status NAME` shows a `tags` line, and `--tag` completes the tags in use.

## Winetricks dependencies

`winetricks` lists the verbs an environment needs. Before each `start`,
`run`, or `restart`, wineo installs the ones missing from the prefix, in
order, in one unattended `winetricks -q` call. A launch with nothing missing
starts no extra process.

```sh
wineo set hades 'winetricks=["vcrun2022", "corefonts"]'
```

```
$ wineo start hades
Installing vcrun2022, corefonts with winetricks
...
Started hades
```

- What counts as installed is winetricks' own record,
  `winetricks.log` in the prefix (`pfx/` for Proton). Verbs installed by hand
  with `wineo winetricks` count too, and the log travels with the prefix
  through clone, snapshots, and portable copies.
- If winetricks fails, the launch fails and names the verbs still missing.
  The next launch retries only those.
- A running environment isn't touched; `start` on one that's already running
  skips the install.
- `-e` overrides from the launch reach winetricks too.
- Removing a verb from the list only stops wineo from installing it again.
  winetricks can't uninstall.
- Settings verbs like `win10` or `renderer=vulkan` apply once. If the setting
  is changed by hand later, wineo won't put it back.
- Verbs are checked for shape only (no spaces, no leading `-`, no
  duplicates). An unknown verb fails when winetricks runs, with its own error.
- It's Wine-only, works in templates, and needed no schema bump, for the same
  reason as the launch options.

On Proton, umu runs its own winetricks, so nothing extra is needed. On Wine,
`winetricks` has to be on `PATH`, and a launch that needs it fails before
anything runs when it isn't. `status NAME` lists the verbs and marks missing
ones, and `wineo validate --fix` installs them ahead of time.

## Runners from other launchers

`external_runners` in the global config lets environments use Proton and
Wine builds that Steam, Heroic, and Lutris installed, read in place. It's
unset until wineo asks, and setting it to `true` or `false` records the
answer. It needed no schema bump: an older wineo refuses the file with
`unknown field` instead of ignoring the setting.

```sh
wineo set --global external_runners=true
wineo set --global --unset external_runners
```

When a name exists in several places, the first folder that has it owns it,
in this order: wineo's runners root, Steam, Heroic, Lutris. That holds even
when the first copy is broken, so a damaged wineo build reports an error
instead of quietly running another launcher's copy. The setting does nothing
while a portable folder is active.

## Creating an environment from a template

To reuse the same settings across environments instead of writing each one by
hand, write a template and create from it. A user template lives in
`templates/<name>.toml` in the config directory and can set:

```toml
schema_version = 1
runner = "proton"      # a runner name, or "proton" for the newest Proton installed
arch = "win64"         # or "win32"
gamemode = true
tags = ["game"]
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
Its built-in settings include the `game` tag. A custom template named
`gaming` keeps its own settings and tags.
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
