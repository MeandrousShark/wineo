Status: draft

# 0018: Config schema versioning

## Goal

Give Wineo's user-facing TOML files an explicit format version before portable
layouts make those files likely to outlive the installation that wrote them.
Keep existing files readable and add a version only when Wineo next changes a
file. Version 1 describes the current format; it does not change settings.

## Existing foundation

- Global settings live in `config.toml`, environment registrations in
  `environments/<name>.toml`, and user templates in `templates/<name>.toml`.
  All three reject unknown fields and currently have no version key.
- Most Wineo edits preserve comments and formatting with `toml_edit` and write
  atomically. New, imported, and cloned environments use separate writers.
- Snapshot manifests have their own JSON version. Component installation
  metadata and runtime operation records are internal formats, not user config.

## Proposed decisions for maintainer approval

### Files and versions

- Add a top-level integer `schema_version = 1` to newly written global and
  environment configs and the built-in gaming template. Accept that key in
  user templates too. Do not version snapshot manifests, component metadata,
  runtime records, or third-party files in this plan.
- A missing key means legacy version 0. Read it with the current parser and do
  not change the file on read. A successful Wineo edit that changes a legacy
  file adds `schema_version = 1` in the same atomic write. A no-op edit leaves
  it alone. User templates have no Wineo edit command, so they remain
  unversioned until the user edits them or a future command writes them.
- Reject a key with the wrong type, version 0 written explicitly, and versions
  newer than 1 with a clear path-specific error. Never rewrite or silently
  downgrade an unsupported version. Missing global config still means defaults
  and creates no file.
- Centralize version checking so every normal read and write follows the same
  rules. Future format changes can add explicit migrations rather than relying
  on permissive deserialization. Keep the current strict unknown-field checks.

### Existing write paths

- Add version 1 to `new`, generic and Heroic import, and clone output. Cloning
  a legacy environment produces a version 1 destination without changing its
  source. Settings edits, target edits, and Heroic ID backfills add the version
  only when they already need to save the file.
- Snapshots preserve the exact saved config text. Restoring a legacy snapshot
  can restore an unversioned config, which still reads as version 0 and gets
  version 1 on its next edit. Snapshot metadata keeps its separate version.
- Audit callers that currently replace a global-config load error with
  defaults. They must not make a write or launch using defaults when the file
  has an unsupported version. Read-only diagnostics may still show an error
  for that file.

## Implementation sequence

1. Add a shared schema check and version 1 parsing for the three user TOML
   formats, with a distinct error for unsupported versions.
2. Update every writer to emit version 1 on new files and on changed legacy
   files while preserving existing formatting and atomic writes.
3. Audit CLI and core load paths, including status and set flows, for version
   errors that must not be swallowed.
4. Document the key and update the feature backlog after implementation.

## Verification and acceptance

- Parse existing unversioned configs and templates without changing bytes or
  modification times. Check new files and each edit path for version 1.
- Check no-op edits, comments, formatting, snapshot restore, clone from a
  legacy config, malformed version values, and future versions. Failed or
  unsupported reads must not alter files or start an operation with defaults.
- Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo
  test`. No Wine installation is needed to test the config format.

## Scope limits

This plan introduces the first version and its compatibility rules. It does
not change the meaning of existing settings, add a migration command, or
convert third-party and internal metadata.
