Status: done

# 0024: Desktop entry icons from art

## Goal

Use the icon in an environment's art store for its desktop entries. Today an
entry only gets an icon when wine's menu builder can pull one out of the
exe, which needs a Start Menu shortcut. That means a game with no shortcut
gets no icon, even when `wineo art fetch` already downloaded a good one.

## Existing foundation

- `desktop::reconcile` (`desktop.rs`) runs on every launch and after `new`,
  `import`, `set`, `setup`, and `winetricks`. It renders the entry text and
  writes nothing if the file already matches. That comparison is what keeps
  a launch cheap.
- Exe icons go to `$XDG_DATA_HOME/icons/hicolor/<size>/apps/wineo-<env>-<target>.png`,
  at whatever sizes wine wrote. `Icon=wineo-<env>-<target>` is written when
  any size exists. Extraction only runs when the entry is being rewritten
  and no icon is on disk yet.
- `art::resolve` already returns a target's icon with the env-then-target
  fallback from 0021: the target's own file first, then the environment's.
- Stored icons can be PNG, JPEG, SVG, or ICO. The store keeps original bytes
  and never converts (0021). Real icons on the maintainer's machine include
  several 1024x1024 PNGs and one ICO.
- The system `hicolor/index.theme` lists sizes 16 to 512 plus `scalable`.
  The user's `~/.local/share/icons/hicolor` has no `index.theme` of its own,
  so a 1024x1024 directory there is ignored by icon lookup.
- `set.rs`'s `apply_desktop` reconciles entries after a config change. It
  takes the launch lock when it can, and passes no runner when the prefix is
  busy, so the entry is still written and the exe icon comes on a later
  launch.
- Portable environments never get desktop entries (0019). That does not
  change.

## Decisions (settled with the maintainer, 2026-09-24)

### 1. Art wins over the exe

A target's entry icon comes from, in order:

1. the target's own icon art (`art/<env>/<target>/icon.*`)
2. the environment's icon art (`art/<env>/icon.*`)
3. the exe, through winemenubuilder, as today

`wineo art rm hades icon` falls back to the next source in the list.

### 2. Conversion with the `image` crate

Add `image` to wineo-core with `default-features = false` and only the
`png`, `jpeg`, and `ico` features. Nothing new is needed at runtime.

- SVG is copied as is to `hicolor/scalable/apps/wineo-<env>-<target>.svg`.
- PNG, JPEG, and ICO are decoded. An ICO with several images decodes its
  largest one. A non-square image is padded to a square with transparency,
  centered, and never cropped.
- The result is written as one PNG (decision 3) with a Lanczos3 downscale.
- Decoding uses `image::Limits` capped at 8192x8192 pixels, so a small file
  that claims huge dimensions can't exhaust memory. The 32 MiB file cap from
  0021 still applies.

### 3. One size

Write one PNG, at the largest standard hicolor size that fits the square's
side: 256, 192, 128, 96, 72, 64, 48, 36, 32, 24, 22, or 16. An image
is never upscaled, except one smaller than 16 px, which goes to 16. The
desktop scales down from there.

Amended during acceptance: the cap was 512, but Noctalia's launcher never
looks in `512x512` under `~/.local/share/icons`, so a 512 px icon showed
blank. 256 is also the largest size wine and Steam write.

| Source      | Written to          |
|-------------|---------------------|
| 1024x1024   | `256x256/apps/`     |
| 300x300     | `256x256/apps/`     |
| 640x480     | `256x256/apps/`, padded to 640x640 first |
| 40x40       | `36x36/apps/`       |
| icon.svg    | `scalable/apps/`    |

### 4. Changes show up right away

`art set`, `art rm`, and `art fetch` reconcile the environment's desktop
entries when they finish, using the same helper as `set` (moved out of
`set.rs` into `commands/desktop.rs` so both can call it). They print the
usual `Updated the desktop entry for <target> in <env>` line only when an
entry changed. An environment with no desktop entries prints nothing extra.

A launch also catches changes, like an icon file swapped by hand.

## Proposed details for maintainer approval

### 5. How reconcile knows the icon changed

The entry gets one more key, written after `Icon=`:

```
Icon=wineo-palworld-palworld
X-Wineo-Icon=art:3f2a9c1e0b7d4e21
```

`art:` is followed by the first 16 hex digits of the art file's sha256
(`sha2` is already a dependency), and `exe` means the icon came from
winemenubuilder. With no icon, the key is left out. Because the key is part
of the rendered text, the existing "file already matches" check keeps
working: an unchanged art file costs one small read and hash per launch,
and no decode.

The key also tells reconcile where the icon files on disk came from. When
the art goes away and the old entry says `art:`, the art-made icon files are
removed and the exe extraction runs again. Today an icon already on disk
skips extraction, and that stays true for `exe`.

Whenever the source changes, every icon file for that entry is removed
first (all sizes, `.png` and `.svg`), so a switch from a 48 px exe icon to
a 256 px art icon, or from SVG to PNG, leaves nothing stale behind.
`remove_icons` learns about `scalable/apps/*.svg` for the same reason.

### 6. Art that can't be decoded

A corrupt or unsupported icon (for example, an ICO whose largest image is
a format `image` can't read) falls back to the exe icon. It prints one line
when the entry is written:

```
Icon art for palworld could not be read, using the exe's icon
```

The entry still records `X-Wineo-Icon=art:<hash>` for the bad file, so the
warning shows once per file, not on every launch. Replacing the file changes
the hash and tries again.

### 7. Existing entries on upgrade

Every existing entry lacks `X-Wineo-Icon`, so the first reconcile after
upgrading rewrites it once and prints `Updated the desktop entry ...` once
per entry. Entries whose environment has icon art switch to it at that
point. An entry that already has an exe icon on disk and no art gets
`X-Wineo-Icon=exe` without running wine again.

## Implementation sequence

1. **Icon conversion.** An `art_icon` module (or a section of `desktop.rs`
   if it stays small) that takes a resolved icon and writes the themed file:
   SVG copy, decode with limits, pad, pick the size, downscale, encode PNG.
   Written atomically. Unit tests with small generated PNG, JPEG, and ICO
   fixtures, plus a non-square one and a tiny one. Adds the `image`
   dependency.
2. **Reconcile.** Resolve the icon source in `desktop::write`, render
   `X-Wineo-Icon`, handle source changes and the fallback from decision 6,
   and extend `remove_icons` to SVG. Tests cover all three sources, the
   target override, art rm falling back, a bad file warning once, and the
   upgrade rewrite. None of them need wine: they pass no runner.
3. **Art commands.** Move `apply_desktop` into `commands/desktop.rs` and call
   it from `art set`, `art rm`, and `art fetch` (only when fetch changed the
   icon kind). Integration tests in `tests/art.rs` check the entry's `Icon=`
   after `art set` and `art rm`.
4. **Docs.** `docs/art.md` loses the line saying desktop icons are later
   work and gains a short "Desktop entries" section (precedence, one PNG at
   up to 256 px, SVG as is). Update the `features.md` artwork row to leave
   only the GUI work. README mention if it lists desktop entry features.

## Verification and acceptance

- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and the full
  `cargo test` under a pty with `--test-threads=1`.
- No new wine-dependent tests. The exe path is unchanged apart from the new
  key, and the `desktop_wine.rs` ignored tests still cover it.
- The maintainer's acceptance, on a machine with a display:
  - `palworld` (1024x1024 PNG art) shows its art icon in the launcher after
    one `wineo start`, installed at `256x256`.
  - `subverse` (ICO art) shows an icon, or prints the decision 6 line.
  - `wineo art rm <env> icon` on an env with a Start Menu shortcut brings
    the exe icon back right away.
  - `wineo art set <env> icon some.svg` installs it under `scalable`.

## Scope limits

- No full size ladder, no icon cache refresh commands
  (`gtk-update-icon-cache`), and no `Icon=` absolute paths.
- Grid, hero, and logo art stay unused until wineo-gui.
- No change to portable environments, which still get no entries.
- `wineo desktop` does not show where each icon came from.
