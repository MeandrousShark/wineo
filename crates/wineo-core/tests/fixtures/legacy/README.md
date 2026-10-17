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

- `./` is a portable folder: `wineo --portable <copy of this directory>`.
- `config/config.toml`: the global config, with every key it had then.
- `config/environments/notepad.toml`: a Wine environment with one target and
  an `[env]` table. Its prefix is `prefixes/notepad`, which does not exist in
  a copy of this folder. The old code's own test wrote `~/prefixes/notepad`,
  but today's wineo refuses that in a portable folder (`prefix_refused`).
- `config/templates/mygame.toml`: a user template with every key a template had.
  Its `dxvk` and `vkd3d` picks are never resolved, since no builds exist.

Each file starts with a `# hand written` comment, so a test can tell that a
`wineo set` edit kept the comment and stamped `schema_version = 1`.

## Left out

No prefix, art, trash, snapshot, or playtime log: none of them existed in this
shape of release, or none can be shown without a prefix.
