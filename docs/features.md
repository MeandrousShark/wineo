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

| Feature | Category | Priority | Why | Notes |
|---|---|---:|---|---|
| `list` and `rm` verbs | Core | **1** | Basic manager functionality; cheap and immediately useful | Decide how `list` relates to `status` without a name before 0001's phase 2 builds that. `rm` needs a call on whether it deletes the prefix, and should refuse while the environment is running. |
| Log management: default behavior and retention | Core | **1** | You're already launching real apps; lifecycle/log semantics should stabilize early | 0001's phase 2 keeps the current run's log and the one before. Retention is what's left, likely with a size cap, since wine's fixme output can make one long session's log huge. |
| Per-environment process tracking mode | Core | **1** | Directly affects correctness of `run/start/status/stop` semantics | First try recording when the app exits separately from when the prefix goes idle, so `status` can report leftover background processes. That may make a per-environment option unnecessary. |
| Running winetricks with `wineo winetricks <prefix>` | Compatibility | **1** | Close to a wrapper around `exec`, and real apps often need winetricks components before they'll install | Once custom runners exist, winetricks has to be pointed at the environment's runner. |
| Shell completion for verbs and flags | UX | **1** | Cheap, delightful, and especially valuable for a CLI-first tool | Static completions are tiny with `clap_complete`. |
| `--json` output for scripting | Automation | **2** | Additive, so it doesn't get harder later as long as output is built from structured data | Build `status` output from a struct in 0001's phase 2 so this stays nearly free. |
| Custom runners in `runners/` | Compatibility | **2** | Major capability and probably one of Wineo's eventual killer features | |
| Proton through umu-launcher | Compatibility | **2** | Major capability, and needed before importing Proton prefixes | |
| `import` existing prefixes | Compatibility | **2** | Important for actually migrating your existing setup | `new --dir` already adopts a prefix in place, so this is mostly about moving it into the default prefix dir. Proton prefixes (like Steam's) have their own layout and get updated when a different wine runs them, so importing those waits for umu. |
| Guided `wineo new` wizard | UX | **2** | Great once the underlying manual workflow is stable | |
| Detect installed executables after an install | UX | **2** | Makes picking a target after an install much easier | Doesn't need the wizard: diffing `.exe` files around `--install-exe` works on its own, and the wizard can reuse it. Big installers drop lots of helper exes, so Start Menu shortcuts may be a better signal, and could feed desktop entries too. |
| Desktop entries | UX | **2** | A big part of apps feeling installed, once detached `start` exists | wineo writes its own entries that run `wineo start <name>`, because wine's don't always keep the settings needed to launch properly. Wine's are turned off in 0001's phase 2, so there are no entries until this lands. |
| Templates, DXVK, vkd3d | Compatibility | **2** | Important, but easier once runner semantics settle | Proton bundles DXVK and vkd3d-proton, while plain wine needs them installed separately. |
| gamemode | Compatibility | **2** | Trivial to add, but mostly matters for games, so it can come with the gaming work | Wraps the launch command in `gamemoderun`. |
| Shell completion for environment and target names | UX | **3** | Polish on top of static completion | Needs dynamic completion, which is more work than static completions. |
| Runner downloads (GE-Proton, Proton-CachyOS) | Compatibility | **3** | Convenience on top of custom runners and umu | Checksums and extraction make this bigger than the rest of the runner work. |
| `clone` a prefix | Portability/Data | **3** | Useful but not fundamental | Use reflink copies where the filesystem supports them, and a normal copy everywhere else. |
| Prefix snapshots | Portability/Data | **3** | Cheap where the filesystem supports it, so it can sit next to `clone` | Only offered on filesystems with reflinks or native snapshots, like btrfs. Everyone else waits for full backup and restore. |
| Portable layout | Portability/Data | **3** | Nice design feature, but standard layout should stabilize first | |
| Import from Lutris/Heroic/Bottles metadata | Compatibility | **3** | Good convenience layer on top of generic `import` | Only if the complexity is minimal. Needs generic `import` and umu first. |
| Config schema versioning | Portability/Data | **3** | Probably becomes necessary before first stable release, not necessarily now | Land it before or with the portable layout, since that's when configs start moving between machines and wineo versions. |
| wineo-gui | UX | **4** | Explicitly not the main product; let the CLI mature first | |
| Full backup and restore | Portability/Data | **4** | Great eventually, but this could become a whole subsystem | Covers filesystems without snapshot support. |
