# Changelog

A finished plan bumps the minor version and fixes between plans bump the
patch version. See `AGENTS.md` for the rule.

## 0.15.0 - 2026-09-28

- Launch, stop, environment creation, winetricks, settings checks, status
  views, stats, and log tailing now live in `wineo-core` as operations a GUI
  can call, with progress events and a cancel token
  ([0034](docs/plans/0034-core-api-review.md)). See
  [Core API for frontends](docs/core-api.md).
- Winetricks and wineboot run in their own process group. Ctrl+C goes to
  wineo, which stops them, and a second Ctrl+C forces it.
- Canceling prefix creation prints "Canceled. Nothing was created." and exits
  with 130.

## 0.14.0 - 2026-09-28

- `tags` groups environments, like `game` or `work`, on Wine and native
  environments and in templates ([0033](docs/plans/0033-tags.md)). See
  [Tags](docs/config-files.md#tags).
- `wineo list --tag` filters by tag. Several `--tag` flags list only
  environments with all of them.
- `status NAME` and its JSON show tags, and `--tag` completes the tags in use.

## 0.13.0 - 2026-09-28

- `winetricks` lists the verbs an environment needs, and `start`, `run`, and
  `restart` install the missing ones first in one unattended call
  ([0032](docs/plans/0032-declarative-winetricks.md)). See
  [Winetricks dependencies](docs/config-files.md#winetricks-dependencies).
- What counts as installed is the prefix's own `winetricks.log`, so verbs
  installed by hand count too. A failed install fails the launch and the
  next one retries only the missing verbs.
- `status NAME` lists the verbs and marks missing ones, and
  `wineo validate --fix` installs them ahead of time.
- The winetricks runner moved into core.

## 0.12.0 - 2026-09-27

- `wayland`, `mangohud`, and a `[gamescope]` table are launch settings on the
  global config, environments, targets, and templates
  ([0031](docs/plans/0031-wayland-gamescope-mangohud.md)). See
  [Launch options](docs/launch-options.md).
- Launches run under `gamescope [flags] -- mangohud gamemoderun`, in Heroic's
  order. Under gamescope, MangoHud becomes `--mangoapp`, and Wayland adds
  `--expose-wayland`.
- On Wine, gamescope stays open until the prefix is idle, so a launcher that
  hands off to the game and exits still works.
- `status`, its JSON, and `validate` report the new settings and missing
  binaries. Runners without Wayland support are an error at launch.
- Heroic imports carry over gamescope, MangoHud, Wine Wayland, and HDR (as
  an `[env]` entry when Wayland is on).
- Docs note the `--backend sdl` workaround for gamescope's window vanishing
  on Hyprland.

## 0.11.1 - 2026-09-26

- `wineo trash` no longer mentions a prefix for entries that had none at
  removal, like native environments.

## 0.11.0 - 2026-09-26

- `wineo rm` moves an environment to recoverable trash instead of deleting
  it, and `--delete-prefix` trashes the prefix and snapshots too
  ([0030](docs/plans/0030-trash-and-prune.md)). See [Trash and removal](docs/trash.md).
- `wineo trash` lists entries, `wineo restore ID` brings one back under its
  original name and paths, and `wineo prune` permanently deletes by ID,
  `--older-than 30d`, or `--all`.
- Interrupted removal, restore, or prune shows up in `wineo validate`, and
  `validate --fix` recovers it.
- Portable folders keep their trash when moved on the same filesystem.

## 0.10.0 - 2026-09-26

- `wineo stats [NAME]` shows playtime, session counts, and per-target totals,
  including running sessions, with JSON output and optional session history
  ([0029](docs/plans/0029-playtime-stats.md)). See [Playtime statistics](docs/stats.md).
- `status` includes playtime, and lost runs count through their last heartbeat.
- Heroic imports carry over playtime; `stats --import-heroic` updates existing
  environments from Heroic's totals.
- Portable copies carry playtime, clones start fresh, and `validate` finds
  stray stats logs.

## 0.9.0 - 2026-09-26

- `wineo validate [NAME]` finds config mistakes and leftover state before a
  launch trips over them, and offers the fixes that are safe
  ([0028](docs/plans/0028-validate.md)). See [Validate](docs/validate.md).
- At a terminal, one review lists every fix. Enter applies them all, and
  numbers like `1 3 4` skip some. `--fix` applies them without asking.
- `--json` reports each problem with a stable `code`, plus `fixed` and
  `errors` for what `--fix` did.
- `wineo desktop` points stray entries at `wineo validate`.

## 0.8.0 - 2026-09-26

- `wineo new --native` is now a plain flag that picks the kind, and
  `--exe PATH` adds a default target named after the environment, for Wine
  and native environments alike
  ([0027](docs/plans/0027-new-exe-flag.md)).
- The old `--native <EXE>` form is gone. Use `--native --exe <EXE>` instead.
- `--native` without `--exe` makes an empty native environment and points at
  `wineo setup`.
- `-d`/`--desktop` on `new` gives the `--exe` target a desktop entry, and
  `import heroic` gains `-d` as the short form of `--desktop`.
- `wineo setup` picks the only target by default when editing, and the edit
  card says `Press Enter to finish editing`.

## 0.7.1 - 2026-09-25

- SteamGridDB pick lists in `art fetch`, guided `new`, and `import heroic`
  have a `Search for something else` entry, and a search with no matches
  asks for another term instead of failing when there's a terminal.
- Shell completion offers paths for path arguments like `new --native`,
  `--install-exe`, and `--dir`, which completed nothing before.

## 0.7.0 - 2026-09-25

- Native Linux games: `kind = "native"` environments launch a Linux binary
  or script directly, with no prefix or runner
  ([0026](docs/plans/0026-native-linux-games.md)). See
  [Native Linux games](docs/native.md).
- `wineo new --native <EXE>`, and guided `new` asks what kind of environment
  first.
- Native runs are tracked through their process group, so launcher scripts
  that hand off to the game stay tracked, and `stop` ends the whole group.
- `import heroic` imports native Heroic games, and skips one whose executable
  is missing.
- `status` shows the kind, and `status --json` has a `kind` field, with
  `runner` and `prefix` `null` for native environments.
- `docs/native-environment.toml.example` is a commented native config, and
  the Arch package installs it.

## 0.6.1 - 2026-09-25

- `snapshot create`, `snapshot restore`, `clone`, `portable copy`, and
  `rm --delete-prefix` wait for wine to exit in the prefix first, since
  wineserver lingers a few seconds after the last program and keeps writing
  the registry. A snapshot right after `exec` no longer fails with "reviewed
  environment changed".

## 0.6.0 - 2026-09-25

- `wineo new` and `wineo import heroic` offer to fetch SteamGridDB art for
  the environment they create, with `--art` for the unguided flows
  ([0025](docs/plans/0025-art-in-new-and-import.md)).
- `[art] fetch = "icon"` in the global config limits those flows to the icon.
- Prompts spell out their keys, like `Press Enter to create it, Esc to go
  back, Ctrl+C to cancel.`, and a review with no rows asks `Create it?`
  instead of `Choice:`.
- Guided `new` no longer lists "Download a runner or graphics tool" twice.

## 0.5.0 - 2026-09-25

- Desktop entries use icon art, the target's first, then the environment's,
  then the exe's ([0024](docs/plans/0024-desktop-entry-icons.md)).
- Art icons are converted to one PNG of up to 256x256, since some launchers
  never look in `512x512`. SVG is installed as is.
- `art set`, `art rm`, and `art fetch` update the affected desktop entries
  right away.

## 0.4.0 - 2026-09-24

- `start`, `run`, `exec`, `winetricks`, and `restart` build a missing prefix
  from a hand-written config, and `setup` offers to
  ([0023](docs/plans/0023-config-first-creation.md)).
- Launch checks that need no prefix run before the build, so a bad config
  fails fast.
- An example environment config, installed to
  `/usr/share/doc/wineo/environment.toml.example` by the Arch package.
- A relative `prefix` is refused outside portable mode instead of resolving
  against the current directory.

## 0.3.0 - 2026-09-24

- Review menus say what Enter does in the keys line, above a plain
  `Choice: ` prompt, instead of listing the accept action as `1)`
  ([0022](docs/plans/0022-review-menu-polish.md)).
- Adding a target returns to the menu that opened the program list.
- `setup`'s Finish and the installer path save without a second confirm.
- A README, and docs for creating an environment from a template.

## 0.2.0 - 2026-09-24

- Artwork store and `wineo art key`, `set`, `list`, `rm`, and `fetch`, with
  SteamGridDB fetching and the key kept in the system keyring
  ([0021](docs/plans/0021-artwork-framework.md), [docs](docs/art.md)).
- `wineo art fetch --asset` skips the fetch filters for an image you pick.
- The Arch package now depends on `dbus`.
- `stop` no longer gives up on a lost Proton run when the killed group's
  leader exits before the rest.
- Unit tests pass when run from a terminal, so `makepkg` builds without
  workarounds.

## 0.1.0

Everything from plans 0001 through 0020, before versions were tracked.
