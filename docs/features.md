# Features

Planned features that aren't scheduled yet. When one gets picked up, it gets its own numbered plan in `docs/plans/`.

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
| `--json` output for scripting | Automation | **2** | 0001 phase 2 | Additive, so it doesn't get harder later as long as output is built from structured data | Build `status` output from a struct in 0001's phase 2 so this stays nearly free. |
| `import` existing prefixes | Compatibility | **2** | 0004 | Important for actually migrating your existing setup | `new --dir` already adopts a prefix in place, so this is mostly about moving it into the default prefix dir. Proton prefixes (like Steam's) have their own layout and get updated when a different wine runs them, so importing those waits for umu. |
| Guided `wineo new` wizard | UX | **2** | Detect installed executables, 0005 | Great once the underlying manual workflow is stable | Picking a template is one of its steps. |
| Detect installed executables after an install | UX | **2** | 0001 phase 2 | Makes picking a target after an install much easier | Doesn't need the wizard: diffing `.exe` files around `--install-exe` works on its own, and the wizard can reuse it. Big installers drop lots of helper exes, so Start Menu shortcuts may be a better signal, and could feed desktop entries too. |
| Desktop entries | UX | **2** | 0001 phase 2 | A big part of apps feeling installed, once detached `start` exists | wineo writes its own entries that run `wineo start <name>`, because wine's don't always keep the settings needed to launch properly. Wine's are turned off in 0001's phase 2, so there are no entries until this lands. |
| Shell completion for environment and target names | UX | **3** | 0002 | Polish on top of static completion | Needs dynamic completion, which is more work than static completions. |
| Runner, DXVK, and vkd3d downloads | Compatibility | **3** | 0003, 0004, 0005 | Convenience on top of the drop-in directories | Covers GE-Proton and Proton-CachyOS alongside DXVK and vkd3d-proton, which 0005 leaves as drop-ins. Checksums and extraction get built once here for all of them. |
| `clone` a prefix | Portability/Data | **3** | | Useful but not fundamental | Use reflink copies where the filesystem supports them, and a normal copy everywhere else. |
| Prefix snapshots | Portability/Data | **3** | | Cheap where the filesystem supports it, so it can sit next to `clone` | Only offered on filesystems with reflinks or native snapshots, like btrfs. Everyone else waits for full backup and restore. |
| Portable layout | Portability/Data | **3** | Config schema versioning | Nice design feature, but standard layout should stabilize first | |
| Import from Lutris/Heroic/Bottles metadata | Compatibility | **3** | `import` existing prefixes, 0004 | Good convenience layer on top of generic `import` | Only if the complexity is minimal. |
| Config schema versioning | Portability/Data | **3** | | Probably becomes necessary before first stable release, not necessarily now | Portable layout and full backup and restore wait on this, since that's when configs start outliving the machine or wineo version that wrote them. |
| wineo-gui | UX | **4** | | Explicitly not the main product; let the CLI mature first | |
| Full backup and restore | Portability/Data | **4** | Config schema versioning | Great eventually, but this could become a whole subsystem | Covers filesystems without snapshot support. |
