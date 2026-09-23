# Features

Feature backlog and queued work. When a feature gets picked up, it gets its
own numbered plan in `docs/plans/`.

Completed: [0007: Guided game setup](plans/0007-guided-game-setup.md) and
[0008: Downloads and prefix imports](plans/0008-downloads-and-imports.md),
including launch settings for manually added Heroic games.
See [Downloads and imports](downloads-and-imports.md) for usage.
Completed: [0010: JSON output for status and list](plans/0010-json-output.md).
See [JSON output](json-output.md) for the command schema and examples.

Completed: [0014: Dynamic environment and target completion](plans/0014-dynamic-shell-completion.md).
See [Shell completion](shell-completion.md) for setup and supported values.
In progress: [0015: Arch package for Wineo and shell completions](plans/0015-arch-package.md).
Clone an environment is queued after 0015 and will get its own plan.

**Category**

- Core: makes Wineo meaningfully better at managing environments
- Compatibility: runners, winetricks, imports, graphics/runtime plumbing
- UX: guided flows, completion, desktop entries, GUI
- Automation: scripting/output behavior
- Portability/Data: clone, portable layouts, schema/migration, snapshots, backups

**Priority**

- 1 = next: should land soon, small, high value, or foundational
- 2 = soon: important, but depends on more core stability
- 3 = later: useful expansion or polish
- 4 = someday: explicitly nonessential or large scope

**Depends on** lists what has to be built first. `0001 phase 2` is the second half of the foundation plan, which has landed, so the rows naming it are unblocked.

| Feature | Category | Priority | Depends on | Why | Notes |
|---|---|---:|---|---|---|
| `clone` an environment | Portability/Data | **1** | | Queued next after 0015 | Use reflink copies where the filesystem supports them, and a normal copy everywhere else. Settle config and target behavior in its own plan. |
| Prefix snapshots | Portability/Data | **3** | | Cheap where the filesystem supports it, so it can sit next to `clone` | Only offered on filesystems with reflinks or native snapshots, like btrfs. Everyone else waits for full backup and restore. |
| Portable layout | Portability/Data | **3** | Config schema versioning | Nice design feature, but standard layout should stabilize first | |
| Import from Lutris/Bottles and Heroic store metadata | Compatibility | **3** | 0008 | Extends generic import and Heroic sideloaded-game import | 0008 covers manually added Heroic games only. Store integration and other launchers remain separate work. |
| Managed copying of portable application folders | Portability/Data | **3** | 0007, 0008 | Optional organization for applications used in place today | 0008 copies prefixes but leaves external game folders where they are. Storage layout and removal semantics need a later plan. |
| Config schema versioning | Portability/Data | **3** | | Probably becomes necessary before first stable release, not necessarily now | Portable layout and full backup and restore wait on this, since that's when configs start outliving the machine or wineo version that wrote them. |
| wineo-gui | UX | **4** | | Explicitly not the main product; let the CLI mature first | |
| Artwork from SteamGridDB | UX | **4** | wineo-gui | Posters, backgrounds, and icons for games, which a GUI can show | Could also fill in desktop entry icons for games whose exe has no shortcut to take one from. |
| Full backup and restore | Portability/Data | **4** | Config schema versioning | Great eventually, but this could become a whole subsystem | Covers filesystems without snapshot support. |
