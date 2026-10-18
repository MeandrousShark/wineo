# Config file versioning

Global config, environment configs, templates, and art.toml carry a
top-level `schema_version` so a file can outlive the wineo version that
wrote it.
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
- `art/<env>/art.toml` (see [art.toml](#arttoml))

## Prefixes inside prefixes

Two environments may share the exact same prefix, but one environment's
prefix can't sit inside another's or contain it, since restoring, removing,
or copying the outer one would take the inner one along. `new`, `clone`,
imports, `set prefix=`, portable copies, snapshot restore, trash restore,
and `rm --include-prefix` refuse it: `b's prefix is inside a's prefix. Move
one of them first.` `wineo edit` doesn't check, so a hand edit can still
nest two prefixes.

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

## Proxied environments

`kind = "proxied"` makes an environment a game another launcher installs,
launches, and owns. `launcher` names the launcher, only `steam` for now, and
`app_id` is the game's id there, as a string. A proxied config can also hold
`title`, `tags`, `[hooks]`, and a top-level `desktop` switch, which defaults
to `false`. Every Wine and native key is refused, as is `[targets]`:

```
bg3 is a Steam game, so it can't set runner.
```

`launcher` and `app_id` are refused the other way around, on Wine and native
configs (`hades isn't a Steam game, so it can't set app_id.`), and an empty
`app_id` is `Invalid app_id: empty.` A new `kind` value needed no schema
bump, for the same reason as `native`. Install state, size, and Proton build
are read from the launcher each time and never stored.
`docs/proxied-environment.toml.example` lists every key. See
[Steam games](steam.md).

The global config's `steam` key picks the Steam install for games when both
native and Flatpak Steam are installed: `"native"`, the default, or
`"flatpak"`. It needed no schema bump either.

```sh
wineo set --global steam=flatpak
wineo set --global --unset steam
```

Two global keys are for installing GOG games, and needed no schema bump:

- `defaults.games_root`, where new GOG games install, each in a folder
  named after its title. Default `~/Games`. GOG games live in the host
  library, so it's stored as `~/...` or an absolute path, never relative to
  a portable folder.
- `gog.language`, the language to install in, as a locale tag like `en-US`,
  `fr-FR`, or `pt-BR` (the default is `en-US`). wineo maps it to GOG's own
  language codes. A tag needs a 2 or 3 letter language; script, region,
  variant, and extension parts are fine. `english` or `en_US` is refused:
  `invalid value for gog.language: use a locale tag like en-US`.

```sh
wineo set --global defaults.games_root=~/Spiele
wineo set --global gog.language=de-DE
```

## Launch options

`mangohud`, `wayland`, and `[gamescope]` needed no schema bump either, for
the same reason: an older wineo refuses a config using them with
`unknown field` instead of launching without them. They can also go in a
template. See [Launch options](launch-options.md).

## Hooks

`[hooks]` with `before` and `after` shell commands, on an environment, a
target, or a template. A target's table replaces the environment's, and
`hooks = false` on a target turns them off. An empty command is refused.
Like the launch options, it needed no schema bump. See [Hooks](hooks.md).

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
error: unsupported schema_version 2 in ~/.config/wineo/config.toml
A newer version of wineo wrote it; update wineo to use this file.
```

Wineo never rewrites or downgrades a version it doesn't recognize, and
nothing acts on a config in this state. A few commands that only finish or
show something carry on with defaults instead (see [A global config wineo
can't use](#a-global-config-wineo-cant-use)).

## Unknown keys

Every config refuses a key it doesn't know, so a typo like `gamemod = true`
fails instead of launching without GameMode:

```
error: invalid config ~/.config/wineo/environments/hades.toml: TOML parse error at line 4, column 1
  |
4 | gamemod = true
  | ^^^^^^^
unknown field `gamemod`, expected one of `schema_version`, `kind`, `prefix`, ...
A newer version of wineo may have written it; update wineo or remove the key.
```

The same happens when an older wineo opens a file a newer one wrote, say a
portable folder from another machine or a config after going back to an
earlier release. Only the files that use the newer key are refused; other
environments keep working. Reading an older file in a newer wineo always
works, but going the other way isn't promised.

Settings that only change how the desktop app shows your library, like
favorites or the sort order, stay out of these files, so that going back to
an older release only loses those preferences. They live in `library.toml`
(see [Library state](#library-state)), which ignores keys it doesn't know.

## Library state

`~/.config/wineo/library.toml` holds how the desktop app shows each
environment: favorites, hidden games, when it was added, and the saved sort.
It's written for you, one field at a time, and never stops anything from
launching. A bad value, like `favorite = "yes"`, counts as unset for that
field only and stays in the file until you fix it or change that setting.

When the file doesn't parse, or has a `schema_version` this wineo doesn't
support, favorites and hidden games are off and every write to it is refused,
so the file stays exactly as it is. `wineo validate --fix` or Checkup can
repair it: the original is copied to `library.toml.broken-<time>` (with
`-2`, `-3` if that name is taken), and that copy is synced to disk before
the new file replaces it. The repair keeps each setting whose line still
reads correctly on its own, under an entry or `[view]` it can recognize, and
reports what it couldn't keep. A file from a newer wineo is never repaired;
update wineo instead. The fix that removes entries for environments that no
longer exist adds `schema_version = 1` if the file had none.

## A global config wineo can't use

When `config.toml` has an unknown key or an unsupported version, commands
that act on settings refuse with the errors above: `set`, `new`, `start`,
removing a runner, portable copies, and the rest. When it's broken in
another way, like bad TOML or a value of the wrong type, they use the
default settings and say so:

```
warning: using default settings: invalid config ~/.config/wineo/config.toml: TOML parse error at line 2, column 10
```

A few commands carry on with defaults and that warning even for an unknown
key or an unsupported version, since refusing would leave something half
done:

- `wineo status`, which only shows things.
- `wineo stop`. The stop timeout is the only global setting it reads, so a
  wrong guess costs seconds, and refusing would leave a game you can't stop.
- Finishing an interrupted snapshot restore or trash removal.

`wineo validate` reports the problem as `global_config_invalid`, with the
same hint.

## Deprecated keys

Nothing is deprecated yet. When a 1.x release renames a key, the old name
keeps working until 2.0. wineo reads it as the new one without touching the
file, and a command prints one line for the whole library:

```
warning: 3 configs use deprecated keys; see wineo validate
```

`wineo validate` reports each one as `deprecated_key`, like
``~/.config/wineo/environments/hades.toml: `old_name` is deprecated since wineo 1.4; use `new_name` ``,
and its fix renames the key in place, keeping comments and order. Until
then, `wineo set` and `--unset` take either name and edit the key the file
already has, so a file stays readable by releases from before the rename.
Setting both names in one file is an error that names both.

## Version bumps

New keys never change `schema_version`. A 1.x release may raise it only
when an existing value starts to mean something different, and then only in
the files that use the new meaning; every other file keeps
`schema_version = 1`, so an older 1.x can still read it. Every later 1.x
reads every older version by that version's rules, and 2.0 migrates older
files instead of refusing them. When settings from files with different
versions combine, each value follows the version of the file it came from.

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
`steamgriddb` with the asset ID, `manual` with the original file name, or
`gog` with the GOG game ID the art came from.
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
the whole file. While art.toml can't be read, desktop entries keep the icon
they already have and wineo says why, instead of treating the environment
as having no art. wineo edits art.toml in place, so hand-added comments
survive. See [Artwork](art.md) for the commands that read and
write it.
