# Trash and removal

Removing an environment moves its Wineo-managed data into recoverable trash.
Each removal gets a unique ID, so removing the same name again does not replace
an earlier entry.

The desktop app does the same from an environment's **Remove...** and its
[Trash page](gui.md#trash).

```sh
wineo rm hades
wineo rm hades --delete-prefix
wineo trash
wineo restore 20260926T183000Z-a1b2c3d4
wineo prune 20260926T183000Z-a1b2c3d4
wineo prune --older-than 30d
wineo prune --all --yes
```

Plain `rm` trashes the raw config, artwork, and playtime log. It leaves the
prefix and its snapshots in place. `--delete-prefix` also trashes the configured
prefix and its snapshots. Transient runtime state and runtime logs are
discarded, and Wineo desktop entries are removed. Native environments have no
prefix; their external executable folders and other external game files are
never part of the trash entry.

`rm` prints the entry ID along with the commands to restore or prune it.
After a plain `rm` it also lists the prefix and snapshots left in place,
which pruning does not delete.

`wineo trash` (or `wineo trash list`) shows each ID, name, how long ago it was
removed, and whether the prefix was included. Restore returns data to its original name and paths. It refuses
collisions rather than overwriting existing data. Restoring a plain `rm` entry
returns its config, art, and playtime while leaving the prefix and snapshots
where they were; restoring a `--delete-prefix` entry returns those too.

A prefix left in place must still match its recorded identity. Restore refuses
if it has disappeared or been replaced. A prefix already absent at removal
does not block restore. Broken configs are retained byte-for-byte and restore
with a warning; desktop setup waits until they are fixed. Desktop entries are
rebuilt from valid restored configs. A shortcut failure leaves the restored
environment registered and reports the problem.

`prune` permanently deletes selected trash entries. Select one ID, entries
strictly older than a positive whole-day age such as `30d`, or all restorable
entries. Selectors cannot be combined. Wineo previews the selection and asks
for confirmation; use `--yes` for noninteractive use. Without a terminal or
`--yes`, nothing is deleted. There is no automatic expiry. Plain-removal
prefixes and snapshots are references left in place, not prune payloads. A day
means 24 elapsed hours, and confirmation freezes the reviewed selection.
Running `prune` without a selector shows help and deletes nothing. Independent
failures do not stop other selected entries; any failure makes the command
exit nonzero and retain the records needed for recovery.

If removal, restore, or prune is interrupted, run `wineo validate` to inspect
the pending work and `wineo validate --fix` to recover it. Ordinary commands
block conflicting work until recovery; Wineo does not recover it silently.
Recovery rolls back unfinished removal and resumes unfinished restore or
prune. Partially pruned entries are not restorable. Small host-state recovery
markers keep pending operations visible if their trash metadata storage is
unavailable; reconnect that storage before recovering.

An interruption before metadata-copy staging ownership is recorded can leave
an empty `.wineo-stage-*` directory. Recovery leaves unowned staging directories
alone rather than risk deleting unrelated data.

Trash payloads stay on their existing filesystems; Wineo will not copy a large
prefix across filesystems to trash it. Portable trash moves with its folder
only on the same filesystem. Finish pending recovery and restore or prune
portable trash before relocating the folder across filesystems. `portable copy`
copies the active environment, not its trash history. Shared-prefix checks
only discover registrations in the selected storage root, so they cannot find
idle registrations in other roots.
