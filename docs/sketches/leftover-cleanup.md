# Clean up leftovers no environment owns

Find and remove prefixes and GOG folder records left behind after an
environment is gone. Backlog row in [features.md](../features.md): Core,
priority 2, depends on 0028, 0030. The row keeps its priority and status;
this page keeps the detail.

From the maintainer (2026-10-16), after removing a GOG game whose folder
had been deleted by hand: a plain `rm` leaves the prefix in place on
purpose, so once its trash entry is pruned the prefix (383 MiB here)
belongs to nothing, and its GOG folder records stay until `import gog
--forget`.

`validate` could warn about a directory in the prefix root that no
environment or trash entry points at, and about a folder record whose
folder is gone and that no environment owns, with fixes to trash the
prefix and forget the record. Trashing a prefix isn't batch-safe, like
snapshot recovery, so `--fix` alone wouldn't do it. Only the default
prefix root is scanned, and `.wineo-snapshots` and staging directories
aren't leftovers. Adds `validate` codes, which is a compatible change
after 1.0.
