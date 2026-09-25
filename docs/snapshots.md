# Prefix snapshots

Snapshots save a registered environment's configured prefix root and its
environment config as a local restore point.

```sh
wineo snapshot create Zomboid before-mods
wineo snapshot list Zomboid
wineo snapshot restore Zomboid before-mods
wineo snapshot delete Zomboid before-mods
```

Environment names and labels are required and case sensitive. Labels follow
the environment naming rules. `list` shows labels in stable order with their
creation times. A label can only be used once; delete it before reusing it.

Wineo stores each snapshot beside the configured prefix, under
`<prefix parent>/.wineo-snapshots/<environment>/<label>/`. It contains the
whole configured prefix root, a copy of the config text, and a manifest. This
also captures Proton's compatdata parent when that is the configured root.
Regular files must support reflinks. Creation or restore fails if the
filesystem cannot reflink any regular file; Wineo does not fall back to a full
copy. This applies on Btrfs too. Btrfs snapshots are not used, and Wineo does
not create or manage subvolumes. Create and restore require a prefix that is
not shared with another registered environment.

Restore reviews the saved and current runner, config, and prefix paths before
replacing the prefix and config. The environment must still be registered with
the same configured prefix. Wineo asks for confirmation in a terminal; use
`--yes` to accept the reviewed restore in scripts:

```sh
wineo snapshot restore Zomboid before-mods --yes
```

Wineo stages and validates the restored prefix before changing the live one.
If a restore is interrupted, ordinary commands refuse to use that environment
until recovery. Roll back the interrupted restore with:

```sh
wineo snapshot recover Zomboid
```

If the old prefix was a Btrfs subvolume, restore leaves it at the path printed
by Wineo. Remove that subvolume manually when it is no longer needed.

In a portable folder, links written by create and restore are relative and
the interrupted-restore marker sits beside the snapshots instead of in host
state; see [Portable layout](portable.md).

Snapshots do not include external game files or other files outside the
configured prefix root, external symlink targets, runners, downloads, runtime
records, logs, import logs, desktop files, or an environment's art store; see
[Artwork](art.md). A restore that drops a target keeps that target's art on
disk, since a later restore may bring the target back; `wineo art list` warns
about it. External files remain in place.
wineserver stays up for a few seconds after the last program exits and keeps
writing the registry, so `snapshot create` and `snapshot restore` first wait
for wine to exit in the prefix, printing `Waiting for wine to exit in NAME`
if it takes more than a moment. Wineo checks its own locks and run state, but
external launchers do not use Wineo locks; stop them before creating or
restoring a snapshot. Snapshots are
stored beside the live prefix and are not backups against filesystem or device
failure. They do not support automatic retention or cross-filesystem restore.

Deleting an environment with `wineo rm` leaves its snapshots on disk and prints
their location. The snapshots then require manual cleanup because Wineo no
longer has a registered environment to validate them. This also applies when
the prefix is removed with `--delete-prefix`.
