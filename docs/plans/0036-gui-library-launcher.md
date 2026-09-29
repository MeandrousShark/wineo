Status: approved

# 0036: GUI library launcher

## Goal

Start `wineo-gui`, the desktop app, with the everyday part: a library of
every environment with its art, what's running, and how long it's been
played, plus Play and Stop. Creating, importing, and editing environments
stay in the CLI for now and come in later plans.

All eight [GUI entry criteria](../features.md#gui-entry-criteria) are met,
and the toolkit is iced, picked after hands-on spikes. This plan turns the
spike's library screen into a real app on top of `wineo-core`.

## Settled by the maintainer

- v1 is the library launcher only. Create and Heroic import come next, then
  settings, validate, and trash.
- A separate `wineo-gui` binary from a new `crates/wineo-gui`, built and
  installed by the same PKGBUILD. The CLI doesn't link iced.
- Keyboard navigation in v1; gamepad later, in its own plan.
- Keep the spike's round 2 look (palette, 180px poster cards, 420px detail
  panel on the right), and add a light palette that follows the desktop's
  light or dark preference.
- Tame iced's scrolling, which felt too fast in the spike, and hide the
  scrollbar while idle.
- The logo moves out of the banner, which it sometimes covered title text
  in. It sits under the banner instead (see decision 1).

## Existing foundation

- [Core API for frontends](../core-api.md): every operation blocks on a
  worker thread, never prints, takes a `cancel::Token`, and reports progress
  through an event callback. The supervisor re-enters the frontend's own
  binary, so `main` calls `supervisor::dispatch_if_supervise()` first.
- `report::overview` returns every environment with its run state, busy
  lock, unresolved installer, tags, and per-environment errors.
  `stats::overview` returns playtime for all of them.
- `launch::start` builds a missing prefix, reconciles components and
  desktop entries, installs missing winetricks verbs, and starts detached,
  reporting `LaunchEvent`s. `launch::stop` stops a run or cleans up a lost
  one.
- `art::resolve(art_dir, config, target, kind)` finds the grid, hero, logo,
  or icon art for a target, falling back to the environment's. Art can be
  PNG, JPEG, WebP, or SVG (plus ICO for icons).
- `log::Tail` reads a run's log and follows it live into any `Write`.
- The spike (`~/Projects/wineo-spike/iced`, iced 0.14) is throwaway and
  isn't copied in, but its notes carry over: `image::Handle::from_path`
  caches decoded textures by path, iced's `grid` only draws visible
  children, and iced ties widget state to tree position, so a view that
  changes shape around a focused `text_input` loses focus.

## Design

### Crate and binary

- `crates/wineo-gui`, binary `wineo-gui`, depending on `wineo-core` and
  iced 0.14 (`image`, `svg`, and whatever else steps need). It inherits
  `unsafe_code = "forbid"`, and fmt, clippy, and tests cover it like the
  other crates.
- `main` hands off to the supervisor before anything else, and passes
  `std::env::current_exe()` as `exe` to `launch::start`, so games started
  from the GUI keep running after it closes, the same as from the CLI.
- It honors `WINEO_PORTABLE_DIR` through core `Paths`, like the CLI. No
  `--portable` flag in v1.
- Wayland app id and desktop file are `wineo-gui`.

### Library

- The top bar has the name, the search field (filters by name,
  case-insensitive), and the scroll area below holds the grid, as in the
  spike.
- Every environment gets a card, including native games and environments
  whose config has errors. Those show a warning dot, and the panel shows the
  error text.
- A card shows the grid art for the environment's default target, or a
  placeholder with the name; the name; a state dot and word; and playtime
  ("3 h 12 m" as `wineo stats` shows it, or "never played").
- Art decodes off the UI thread, and the grid only loads what's visible.
- State refreshes by calling `report::overview` and `stats::overview` on a
  worker every 2 seconds, so a game started or stopped from the CLI shows
  up too. A later plan can switch to file watching.

### Detail panel

- The banner is the hero art, shown clean. Without a hero, it's the plain
  surface color.
- The logo sits at the banner's bottom-left, at most about 72px tall, over a
  gradient scrim when there's hero art. Without a logo, the name is a
  heading below the banner. (This was option 2 of decision 1; the plan first
  put the logo under the banner.)
- Rows: state, playtime, sessions, last played, and tags when present.
- Play and Stop, with the spike's enabled and disabled rules. An environment
  with more than one target gets a target picker next to Play, preset to the
  default (decision 2).
- While starting, the panel shows what `LaunchEvent`s report ("Building
  prefix...", "Installing corefonts...", "Starting...") and a Cancel button.
  Cancel calls `token.cancel()`. After that it becomes Force stop, which
  calls `token.force()`.
- Errors show in the spike's error box. `LostRun` adds a Clean up button
  that calls `launch::stop`. A busy environment says what it's busy with.
- A Log button swaps the rows for the run's log, followed live through
  `log::Tail` while the game runs (decision 3).

### Keyboard

- Arrow keys move the selection through the grid, following its current
  column count. Up and Down always go to the card visually above or below.
  They don't remember the column you came from, so Up from a lone card in
  a short last row goes straight up (maintainer's pick during acceptance).
- Double-clicking a card opens its panel and plays it, when Play is enabled
  (added by the maintainer during acceptance). Enter opens the panel, or plays when it's already open.
  Esc closes the panel, or clears the search when the panel is closed.
  `/` and Ctrl+F focus the search, and Down leaves it for the grid.

### Look

- Round 2 palette as the dark theme, plus a light palette with the same
  tokens. It follows the desktop's color-scheme preference (the XDG
  settings portal) and falls back to dark when there isn't one. Check what
  iced 0.14 offers before adding a portal dependency.
- Scrolling: scale down iced's wheel steps until it feels like a native app
  (the spike's gpui version is the reference). The scrollbar is thin and
  only shows while scrolling or hovered.
- Fonts and icons that ship in the binary (Lucide's "x", for instance) get
  an attribution file next to the crate's license notes.

## Decisions for the maintainer

The maintainer accepted every recommendation (2026-09-28).

### 1. Logo placement

**Settled:** option 2, the logo at the banner's bottom-left over a gradient
scrim. The maintainer compared both on real art during acceptance: under
the banner, the logo felt tacked on rather than part of the art. Option 1
and the `WINEO_GUI_LOGO` switch are removed, and option 2 becomes the only
layout.

**Recommended:** for acceptance, also build the Steam-style variant behind
a hidden environment variable (`WINEO_GUI_LOGO=scrim`) so both can be
compared on real art. The maintainer said they might like it too. The one
not picked is removed before the plan is marked done.

```
option 1 (default)                      option 2 (WINEO_GUI_LOGO=scrim)
+----------------------------------+    +----------------------------------+
|                                  |    |                               [x]|
|        hero.png (clean)       [x]|    |        hero.png                  |
|                                  |    |::::::::::::::::::::::::::::::::::|
+----------------------------------+    | [LOGO]  ::: gradient scrim ::::: |
  [ LOGO ~64px ]                        +----------------------------------+
  State            running                State            running
```

### 2. Environments with several targets

**Recommended:** a picker next to Play, preset to the default target, which
also switches the panel's art to that target's. Without it, only the
default target is playable from the GUI.

### 3. Logs

**Recommended:** in the panel, replacing the rows, with a Back button.
A separate log window is more room but a second window to manage.

### 4. App icon

wineo has no icon.

**Recommended:** a simple placeholder SVG for now, with a real icon as a
separate task. The desktop file and window use it.

### 5. GUI settings

**Recommended:** none in v1. No settings file, window size not remembered,
theme always follows the desktop. Add them when there's something worth
setting.

## Implementation sequence

1. **Skeleton.** The crate, supervisor hand-off, window, both palettes with
   the desktop preference, and a plain list of names from
   `report::overview`. The workspace builds, the PKGBUILD builds both
   binaries, and AGENTS.md's code boundaries mention the new crate.
2. **Library grid.** Cards, art off the UI thread, search, state dots,
   playtime, the 2 second refresh, and the scrolling and scrollbar fixes.
3. **Detail panel.** Banner, logo (both variants), rows, Play, Stop, target
   picker, start progress, Cancel and Force stop, errors and Clean up, and
   the log view.
4. **Keyboard.** Grid navigation, Enter, Esc, and search focus.
5. **Packaging and docs.** Desktop file, placeholder icon, attribution file,
   PKGBUILD install lines, `docs/gui.md`, README, and features.md.

## Testing

- The app's state and `update` logic is tested without a window: messages
  in, state and requested operations out. Check whether iced 0.14's test
  tooling helps, and fall back to testing `update` directly.
- Core operations are already covered, so the GUI tests focus on the GUI's
  own logic: search filtering, keyboard selection across column counts,
  enabled button rules, event-to-status text, and which target Play uses.
- Operations that start games are never run by agents. A smoke test opens
  the window against a temporary XDG root with fake environments, under
  `timeout`, and checks it doesn't panic.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`
  pass for the whole workspace, and the PKGBUILD's `check()` still passes.

## Verification and acceptance

The maintainer, on their real library:

- Art, states, and playtime match `wineo list` and `wineo stats`.
- Play a game, watch the state change, close the GUI, and check the game
  keeps running. Reopen it and stop the game from there.
- Start a game from the CLI and see the GUI notice within a couple of
  seconds.
- Cancel a start that builds a prefix or installs winetricks verbs.
- Compare the two logo placements, pick one.
- Scrolling feels right, and the scrollbar stays out of the way.
- Switch the desktop between light and dark.
- Navigate and launch with only the keyboard.

## Scope limits

- No creating, importing, editing, removing, or validating environments.
- No gamepad, no big-picture mode, no GUI settings file.
- No fetching art from the GUI. `wineo art fetch` fills it in.
- No macOS work. The app only needs to build and run on Linux.
- No changes to core behavior. If the GUI needs something core doesn't
  offer, it gets its own small commit, reviewed like a patch fix, or a
  question for the maintainer if it's a design change.
