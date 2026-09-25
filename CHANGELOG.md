# Changelog

A finished plan bumps the minor version and fixes between plans bump the
patch version. See `AGENTS.md` for the rule.

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
