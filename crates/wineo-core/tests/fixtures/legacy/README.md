# legacy fixture set

Hand-written, with no release behind it: files from before wineo wrote
`schema_version`. There is no tagged release that wrote them, so this is the
one set the generator script does not produce.

The shape comes from the code just before `schema_version` arrived
(`git show b73dcfc8^`): the `NOTEPAD` environment in `settings.rs`'s tests, the
`GlobalConfig`, `EnvironmentConfig`, and `Target` structs in `config.rs`, and
the `Template` struct in `templates.rs`. No file has a `schema_version` key, and
none uses a key added later (no `kind`, `tags`, `wayland`, `hooks`, and so on).
Release v0.2.0 reads all of them as legacy version 0.

## Layout

Two sibling roots hold the same three files, in the two shapes a real file had:

- `portable/` is a portable folder: `wineo --portable <copy of portable/>`. Its
  environment's prefix is `prefixes/notepad`.
- `xdg/` is a HOME: point `HOME`, `XDG_CONFIG_HOME`, and `XDG_DATA_HOME` at it
  and its `.config` and `.local/share`. Its environment's prefix is
  `~/prefixes/notepad`, the form the old code's own test used. Current wineo
  refuses that form in a portable folder (`prefix_refused`), which is why the
  portable root differs.

`xdg/prefixes/notepad` is a minimal hand-made prefix (`system.reg`, `dosdevices/c:`,
and empty `win.ini` and `notepad.exe`), because wineo refuses a `~/` prefix whose
parent folder is missing. `portable/` has no prefix, which is only a warning there.

Each root has:

- `config.toml`, the global config, with every key it had then.
- `environments/notepad.toml`, a Wine environment with one target and an `[env]`
  table.
- `templates/mygame.toml`, a user template with every key a template had. Its
  `dxvk` and `vkd3d` picks are never resolved, since no builds exist.

Each file starts with a `# hand written` comment, so a test can tell that a
`wineo set` edit kept the comment and stamped `schema_version = 1`.

## Left out

No art, trash, snapshot, or playtime log: none of them existed in this
shape of release, or none can be shown without a prefix.
