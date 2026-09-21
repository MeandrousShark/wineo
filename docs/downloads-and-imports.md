# Downloads and imports

## Download a runner or graphics component

```sh
wineo download
wineo download ge-proton --list
wineo download ge-proton
wineo download proton-cachyos
wineo download dxvk
wineo download vkd3d
```

The guided command offers providers and releases. A provider without a version
selects its latest supported stable upstream release. Use `--version <tag>` for
an exact release and `--list --page 2` for older releases.

Downloads support Linux x86_64. CachyOS selection uses its baseline SLR build.
wineo verifies published checksums before extraction and installs each build in
its own directory. Existing directories are never replaced. An identical
completed download is a no-op. Cancellation removes incomplete staging data.

Local runner and component listings remain offline. Launching never downloads a
runner automatically.

## Follow the newest installed runner

```sh
wineo set my-game runner=GE-Proton-Latest
wineo set my-game runner=Proton-CachyOS-Latest
wineo runners
```

These aliases select the newest compatible stable build of their family that
is already installed. Downloading a newer build affects the next operation.
An active game or installer keeps its original runner, including cleanup.
Status shows the concrete runner used for that operation.

Selecting a concrete runner name keeps it pinned. Aliases do not check for
updates or fall back to another runner family. The two alias names are reserved;
do not create directories or symlinks with those names in the runner root.

## Import an existing prefix

```sh
wineo import
wineo import prefix /path/to/prefix
wineo import prefix /path/to/prefix -n my-game --runner GE-Proton-Latest
wineo import prefix /path/to/prefix -n my-game --runner wine --adopt
```

Copying is the default. `--dir /destination` changes the copy destination.
`--adopt` registers the original folder in place and cannot be combined with
`--dir`. The selected runner must match the Wine or Proton layout. A Proton
import includes the complete compatdata directory, even when its `pfx` path was
supplied. Use `--arch win32` or `--arch win64` only when the registry has no
architecture marker.

Import preserves prefix files and registry settings without running Wine.
Symlinks are preserved, with internal absolute links rebased to the copy.
External game folders, linked saves, and other external data stay where they
are. Review lists those dependencies. Stop applications using the source before
importing; a copy is not a live snapshot.

Fully specified imports register the prefix without prompting. Guided imports
offer target setup afterward, which can be skipped or revisited:

```sh
wineo setup my-game
```

If registration fails after copying, the completed copy is retained. The error
reports its location so it can be registered with `--adopt`.

## Import a manually added Heroic game

```sh
wineo import heroic --list
wineo import heroic
wineo import heroic --config-dir /path/to/heroic
wineo import heroic --game <id> -n my-game --runner <installed-runner>
```

wineo reads local Heroic settings once. It supports manually added Windows games,
with source selection when native and Flatpak libraries are both present.
Heroic files are never edited. Store libraries and credentials are not imported.

The guided flow can copy Heroic's runner, choose an installed runner, or download
a supported replacement. Runner installation finishes before prefix copying.
A successfully installed runner remains available if the import is canceled.

Review covers the executable, argument boundaries, working directory,
environment variables, gamemode, and supported compatibility toggles. Unsupported
wrappers, scripts, runtime settings, and other omissions require attention.
Noninteractive imports refuse unresolved settings; add `--interactive` to review
them. Import does not launch the game automatically.

## Removal

`wineo rm my-game` removes registration and wineo desktop entries, retaining the
prefix. `wineo rm my-game --delete-prefix` explicitly deletes the configured
prefix, including the original when it was adopted. External game files and
Heroic metadata are not removed with the prefix.
