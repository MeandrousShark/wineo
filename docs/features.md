# Features

Planned features that aren't scheduled yet. When one gets picked up, it gets its own numbered plan in `docs/plans/`.

| Feature | Priority | Notes |
| --- | --- | --- |
| Guided `wineo new` wizard | | |
| Templates, gamemode, DXVK, vkd3d | | |
| Custom runners in `runners/`, Proton through umu-launcher, runner downloads (GE-Proton, Proton-CachyOS) | | |
| wineo-gui | | |
| Portable layout with the config stored inside the prefix | | |
| `list` and `rm` verbs | | Decide how `list` relates to `status` without a name |
| `import` verb for existing prefixes, optionally moving them to the default prefix dir or leaving them as is | | `new --dir` already adopts a prefix in place |
| During guided setup, detect and list installed executables after the install, with the option to pick the default target | | |
| `import` from other launchers (Lutris, Heroic, Bottles) without tight coupling, picking up the default executable, runner info, and prefix path | | Only if the complexity is minimal |
| Running winetricks with `wineo winetricks <prefix>` | | |
| Shell completion for bash, fish, and zsh | | |
| Log management: default behavior and retention | | |
| `clone` a prefix | | |
| Prefix snapshots and backups | | Way later |
| Config schema versioning | | Undecided whether it's needed |
| `--json` output for scripting | | From 0001's open questions |
| Per-environment option to track only the launched process, for apps whose services keep the prefix alive | | From 0001's open questions |
