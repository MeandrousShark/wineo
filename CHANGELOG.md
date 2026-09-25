# Changelog

A finished plan bumps the minor version and fixes between plans bump the
patch version. See `AGENTS.md` for the rule.

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
