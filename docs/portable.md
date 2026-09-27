# Portable folder

A portable folder holds Wineo's config, registered environments, and their
snapshots under one directory, so the whole thing can be moved to another
path, drive, or machine.

```sh
wineo --portable ~/usb/wineo new -n hades
WINEO_PORTABLE_DIR=~/usb/wineo wineo list
```

`--portable DIR` and `WINEO_PORTABLE_DIR` select the folder; the option wins
when both are set. Wineo resolves the selector to an absolute path at
startup and never searches the current directory or the executable's
location for one. An empty `WINEO_PORTABLE_DIR` is treated as unset; an empty
`--portable` is an error. A leading `~/` in either selector expands to the
home directory. Without a selector, Wineo uses the standard XDG locations as
before.

## Layout

```
DIR/config/config.toml
DIR/config/environments/
DIR/config/templates/
DIR/prefixes/
DIR/prefixes/.wineo-snapshots/<name>/
```

Wineo creates missing directories as needed when it writes to the folder.
Read-only commands, like `list` and `status`, do not create it.

## What stays on the host

Wineo still finds runners, DXVK, and vkd3d-proton in their existing host
locations unless the config points elsewhere; the portable folder does not
carry them. Runtime records, locks, and logs also stay in the host's XDG
state directory, but are isolated per portable root, keyed by its resolved
path, so an XDG environment and a portable one with the same name never share
state or a lock by accident. Prefix locks are the exception: they live in one
host-wide directory keyed by the resolved prefix path, so an XDG and a
portable environment that touch the same prefix still coordinate.

Because runners and host state are not part of the folder, moving it to
another machine can require updating runner paths and any external game
paths the config points at.

## Paths

Inside a portable folder, a path under the folder's root is stored relative
to that root and resolved from it, independent of the current directory.
Paths outside the folder, like external game or runner directories, are kept
absolute or as `~/...`. Windows target paths are never reinterpreted as Unix
paths.

A portable prefix must be a direct child of `DIR/prefixes/`; Wineo refuses a
new or edited prefix anywhere else, and refuses a `defaults.prefix_root`
other than `DIR/prefixes/` when one is set. Commands that show a prefix path
print the resolved absolute path, not the stored relative value.

## No desktop entries

Portable environments never get desktop entries or icons. `set`, `new`, and
imports refuse `desktop = true` on a portable target, and `wineo desktop`
refuses to run at all under `--portable`. A hand-edited `desktop = true` in a
portable environment's config is ignored, with a one-line warning when the
target starts:

```
No desktop entry for game in hades: desktop entries aren't supported for portable environments
```

## Moving the folder

Copy, import, clone, and snapshot create and restore write links that point
inside a copied prefix or its snapshots as relative links, so they keep
working after the folder moves. An interrupted snapshot restore's marker
lives inside the folder, beside that prefix's snapshots under
`DIR/prefixes/.wineo-snapshots/<name>/`, so it moves with the folder too.
`wineo snapshot recover NAME` still finds it at the new location.

Recovery identifies the original prefix by directory identity (device and
inode), not by path. If the folder is moved to a different filesystem while
a restore still needs recovery, that identity does not survive the move, and
recovery stops rather than guessing which copy is the original:

```
cannot identify original prefix at ... or ...
```

A plain move on the same filesystem does not have this problem.

Portable trash and pending trash recovery also rely on filesystem identity.
Move the folder only on the same filesystem while it contains trash history or
pending recovery. Before moving it across filesystems, run
`wineo validate --fix` to finish recovery and restore or prune its trash
entries. Portable manifests are authoritative; host-state recovery markers
are not a registry of environments. `wineo portable copy` copies the active
environment and snapshots, art, and playtime, but not trash history. See
[Trash and removal](trash.md).

## Copying an environment in

```sh
wineo portable copy hades --to ~/usb/wineo
```

`portable copy` copies an environment from the standard XDG layout into a
portable folder: its config, its full configured prefix tree, its art store,
its [playtime log](stats.md), and every snapshot for that prefix. The source is only read and is left
unchanged. Like `clone`, it first waits for wine to exit in the source
prefix, then prints its review, showing `art: copied` when
the environment has art (see [Artwork](art.md)), and then copies without a
confirmation prompt.

Wineo reviews the source and destination, runner, prefix layout, targets,
snapshots, external dependencies, and settings that change before copying,
including any literal source-prefix paths in arguments or environment
variables that need manual review. Copied targets are always written with
`desktop = false`, and the review lists that change. When `DIR/config/`
does not exist yet, the copy also brings over the source's global config and
user templates, rebasing a copied `defaults.prefix_root` into
`DIR/prefixes/`; otherwise the portable folder's own global config and
templates are kept as they are, even if one of them is missing.

The review reports whether the destination can reflink from the source. A
copy without reflink support still succeeds but takes full space; creating or
restoring a snapshot inside the folder still requires a reflink-capable
filesystem regardless. Wineo refuses to copy onto a name or path that
already exists in the destination, anything that would overlap the source,
and a source with an interrupted restore that still needs recovery.

Wineo stages the copy inside the folder and publishes it without replacing
anything, writing the environment config last. If a copy is interrupted
after some paths were already published, it reports which ones; move them
aside before retrying.
