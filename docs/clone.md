# Clone an environment

```sh
wineo clone Zomboid zomboid_modded
wineo clone Zomboid zomboid_modded --dir ~/wineo-copies/zomboid_modded
```

Both names are required. If the new name is missing, wineo prints the syntax
error and usage; it does not start a guided flow or write anything. Environment
names are case sensitive and must match the registered name exactly.

By default, wineo puts the copy under the configured prefix root at
`<prefix root>/NEW_NAME`. The default root gives
`~/.local/share/wineo/prefixes/NEW_NAME`. `--dir` chooses a different
destination. The destination must not exist or overlap the source. Clone copies
the prefix; it does not adopt, merge, overwrite, or create a new Wine prefix.

Wineo reviews the source, destination, runner, prefix layout, targets, external
dependencies, and settings that change before copying. It copies the entire
configured prefix root, including Proton compatdata and adjacent metadata.
When a prefix has many external links, the review shows the first 12 and their
total count; add `-v` to see every link.
Regular files use reflinks when supported, with a normal copy as fallback.
Symlinks, hardlinks, and file metadata are preserved. Runtime records, logs,
import logs, and desktop files are not copied.

The source and any external launcher using it must be idle during the copy.
Wineo checks its own operations, but external launchers do not use Wineo locks.
Clone does not run Wine or launch the game. External game folders and other
dependencies stay in place and are listed during review.

The clone keeps the source runner, architecture, compatibility settings,
environment variables, target names, arguments, and default target. Absolute
Unix target executable and working-directory paths inside the prefix are
rebased to the clone. Windows paths and paths outside the prefix stay unchanged.
The Heroic import ID is removed, and target desktop entries are disabled to
avoid duplicates. Literal source-prefix paths in arguments or environment
variables are reported for review. Registry contents and arbitrary strings are
not rewritten.

If registration fails after the copy, wineo keeps the copied prefix and reports
its location for recovery. Remove a clone registration with `wineo rm NAME`;
delete its copied prefix with `wineo rm NAME --delete-prefix`.

Clone copies local prefix data only. It does not copy external game folders,
downloads, runners, runtime state, or desktop entries, and it does not migrate
registry paths.

Cloning into a portable folder writes relative links instead of the absolute
rebasing described above; see [Portable layout](portable.md).
