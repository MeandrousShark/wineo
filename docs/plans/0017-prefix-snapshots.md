Status: draft

# 0017: Prefix snapshots

## Goal

Save and restore a registered environment's Wine prefix and config together.
Snapshots are local, space-efficient restore points. They do not copy external
game files or replace full backup and restore.

## Existing foundation

- Clone already inspects a registered Wine or Proton environment, coordinates
  prefix and config locks, checks that the source is idle and unchanged, and
  copies a complete prefix through a staging directory.
- The shared tree copier preserves hardlinks, symlinks, modes, and timestamps.
  It rebases absolute internal links and attempts a reflink for each regular
  file, but currently falls back to a full copy when reflinks are unavailable.
- Environment configs are TOML files with no schema version. The config may
  contain target paths inside or outside the prefix.
- The feature backlog limits snapshots to filesystems that support reflinks or
  native snapshots. Full backup and restore is separate work.

## Proposed decisions for maintainer approval

### Scope and commands

- Add `wineo snapshot create ENV LABEL`, `wineo snapshot list ENV`,
  `wineo snapshot restore ENV LABEL`, and `wineo snapshot delete ENV LABEL`.
  Require explicit names rather than choosing a current environment or
  generating a label. Complete ENV and existing LABEL values in the shell.
- A snapshot captures the whole configured prefix root, including Proton's
  compatdata parent, plus the environment config text. Restore puts back both
  the prefix and the saved config, including its runner, targets, and settings.
  Runtime records, logs, import logs, and desktop files are not included.
- Preserve config formatting and comments by saving the original text. Restore
  requires the environment to still be registered and its configured prefix
  to resolve to the path recorded in the snapshot. It refuses to overwrite a
  different prefix if the environment has since moved.
- Use the environment name and snapshot label as the snapshot identity. Refuse
  an existing label and validate labels with the environment name rules. List
  labels in stable order with creation times. Do not report a misleading
  physical size for reflinked data.

### Storage and filesystem support

- Store snapshots beside the configured prefix, under
  `<prefix parent>/.wineo-snapshots/<environment>/<label>/`. Each snapshot has
  a copied `prefix/`, saved `config.toml`, and a small versioned manifest with
  the environment name, resolved original prefix path, layout, and creation
  time. This location keeps the copy on the prefix's filesystem, including
  when the prefix lives outside Wineo's default data directory.
- Extend the existing copier with a strict reflink mode for snapshots. Every
  regular file must be reflinked; a single unsupported reflink aborts the
  staged snapshot with a clear filesystem error. Clone and import retain their
  current full-copy fallback. Reuse the copier's hardlink, symlink, metadata,
  cancellation, source-change, and staged-validation checks.
- Use reflinks in this first version. Native filesystem snapshot APIs can be
  added later. Do not silently fall back to a full copy, and do not claim that
  snapshots are backups. Snapshot storage and the live prefix can fail
  together; external symlink targets and external game files remain external.
- Publish the completed snapshot without replacing an existing label. A
  canceled or failed create removes only its staging directory. Keep the
  source environment and any existing snapshots unchanged.

### Coordination and restore safety

- Refuse create and restore when another registered environment shares the
  prefix. Refuse active, lost, or unresolved runs and conflicting Wineo
  operations. Explain that external launchers do not use Wineo locks and must
  be idle. Recheck the config and prefix after review and during the copy.
- Restore first validates the selected snapshot, then reflink-copies its prefix
  into a staging directory beside the live prefix. Rebase internal absolute
  symlinks back to the live path. Do not mutate the selected snapshot.
- Show the saved and current config identity and the paths affected before
  restore. Prompt for confirmation in a terminal; `--yes` explicitly accepts
  the reviewed restore for scripts. Refuse a noninteractive restore without
  `--yes`.
- Preserve the current prefix and config until the restored prefix has been
  staged and validated. Use an atomic directory exchange for the live and
  staged prefixes, then replace the config atomically. A durable restore marker
  and retained old prefix/config make an interruption detectable. While the
  marker exists, ordinary commands must refuse to use that environment.
  Provide `wineo snapshot recover ENV` to roll back an interrupted restore.
  Clear the marker only after the new prefix and config are both committed;
  remove the old prefix after the marker is clear. If atomic exchange is not
  available, fail before changing the live environment.
- If restore fails after exchange, roll back under the same locks. If automatic
  rollback fails, retain the marker and both copies, and print their paths plus
  the recovery command. Never delete the selected snapshot during restore.
- Delete removes only the named snapshot after checking its manifest and
  location. It must not follow external symlinks or touch the live prefix.
  Removing an environment leaves its snapshots on disk and reports their
  location; an orphaned snapshot requires manual cleanup because no registered
  environment remains to validate it.

## Implementation sequence

1. Add snapshot paths, manifest handling, and strict reflink mode to the shared
   copier without changing clone or import behavior.
2. Add core inspection, create, list, delete, restore, and interrupted-restore
   recovery with prefix/config coordination and source rechecks.
3. Add CLI commands, completion, review and progress output, cancellation,
   restore confirmation, and user documentation.
4. Add focused core and CLI tests, run a disposable real-prefix smoke test,
   and update the feature backlog after the behavior is verified.

## Verification and acceptance

- Tests cover Wine and Proton roots, saved config fidelity, internal absolute
  links, hardlinks, external dependencies, reflink success and unsupported
  filesystems, collisions, shared prefixes, running/lost state, source changes,
  cancellation, and deletion that leaves the live environment unchanged.
- Restore tests cover changed settings and prefix contents, path mismatch,
  noninteractive confirmation, failure before exchange, failure after exchange,
  and interruption between exchange and config publication. Recovery must
  leave a matching original prefix/config and preserve the selected snapshot.
- CLI tests use isolated XDG roots and synthetic prefixes. Run `cargo fmt`,
  `cargo clippy --all-targets -- -D warnings`, and `cargo test`; run applicable
  ignored tests and independent QA. A real-prefix smoke test must use a
  disposable environment and delete only its test snapshot and prefix.

## Scope limits

Snapshots are local restore points for one registered environment. They do not
include external game folders, runners, downloads, runtime state, or desktop
entries. They do not provide portable archives, cross-filesystem backups,
retention policies, automatic schedules, or native filesystem snapshots.
