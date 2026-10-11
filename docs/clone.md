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
The review counts links that point outside the prefix, mostly Wine's drive
mappings in `dosdevices`; add `-v` to list each one.
Regular files use reflinks when supported, with a normal copy as fallback.
Symlinks, hardlinks, and file metadata are preserved. Runtime records, logs,
import logs, and desktop files are not copied.

The source and any external launcher using it must be idle during the copy.
Before reviewing, wineo waits for wine to exit in the source prefix, since
wineserver lingers for a few seconds after the last program exits. Wineo checks
its own operations, but external launchers do not use Wineo locks.
Clone does not run Wine or launch the game. External game folders and other
dependencies stay in place and are listed during review.

The clone keeps the source runner, architecture, compatibility settings,
environment variables, target names, arguments, and default target. Absolute
Unix target executable and working-directory paths inside the prefix are
rebased to the clone. Windows paths and paths outside the prefix stay unchanged.
The Heroic import ID and `gog_id` are removed (a clone shares the GOG game's
folder but doesn't own it), and target desktop entries are disabled to
avoid duplicates. Literal source-prefix paths in arguments or environment
variables are reported for review. Registry contents and arbitrary strings are
not rewritten.

A clone of a GOG game still runs from the original game folder, so it
follows that game's state. While the game is mid-update, launching the clone
is refused with the name of the game to finish, and while the clone runs,
the game can't be updated. wineo finds the folder through the paths a
launch names: the program, its working directory, and arguments that are
whole absolute or Windows paths. A program Wine finds by bare name, an
argument like `--data=/path`, or a launcher that finds the game some other
way isn't seen.

If registration fails after the copy, wineo keeps the copied prefix and reports
its location for recovery. Remove a clone with `wineo rm NAME`; this trashes
its config and art, leaving the copied prefix in place. Add
`--delete-prefix` to trash the copied prefix and snapshots too. See
[Trash and removal](trash.md).

Clone also copies the source's art store to the new name, and the review shows
`art: copied`; see [Artwork](art.md). Playtime isn't copied, so the clone
starts at zero; see [Playtime](stats.md). `wineo rm` retains a clone's art in
trash with the rest of its registration.

Clone copies local prefix data only. It does not copy external game folders,
downloads, runners, runtime state, or desktop entries, and it does not migrate
registry paths.

Cloning into a portable folder writes relative links instead of the absolute
rebasing described above; see [Portable layout](portable.md).
