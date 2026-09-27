Status: draft

# 0030: Recoverable removal and prune

## Goal

Make both forms of `wineo rm` recoverable. Retain the environment's config,
art, and playtime together, optionally including its prefix and snapshots.
Permanent deletion becomes an explicit `wineo prune` operation. This is GUI
entry criterion 4.

## Existing foundation

- Removal currently lives in `crates/wineo-cli/src/commands/rm.rs`. Plain
  `rm` deletes config, runtime state, desktop entries, art, and stats while
  leaving the prefix and snapshots behind. `--delete-prefix` permanently
  deletes the configured prefix first, including the whole Proton compatdata
  root where applicable.
- Removal holds environment and prefix operation locks, refuses active or
  unresolved work, and checks for shared prefixes before deleting one.
  Config locking is separate. The config and prefix must be revalidated
  under the locks rather than trusting the initial read.
- `Paths` resolves XDG and portable storage. Config, data, and prefixes can
  live on different filesystems. Portable run state and locks remain in host
  XDG state.
- Snapshots live at `<prefix-parent>/.wineo-snapshots/<name>`. Existing
  snapshot commands need a registered environment, so snapshots left by
  removal are not accessible through those commands until restoration.
- Snapshot restore already uses durable recovery records. Trash needs the
  same ability to recognize interrupted work across several filesystem
  operations; a sequence of renames is not a single atomic transaction.

## Decisions settled with the maintainer (2026-09-26)

### 1. What removal retains

Both forms of `rm` create a unique trash entry:

| Data | `rm NAME` | `rm NAME --delete-prefix` |
|---|---|---|
| Raw config, including comments | Trash | Trash |
| Artwork and playtime log | Trash | Trash |
| Wine prefix | Leave in place | Trash |
| Environment's snapshots | Leave in place | Trash with the prefix |
| Desktop entries | Remove | Remove |

Native environments use plain `rm`; they have no prefix to trash. External
game files, executables, runners, and shared component builds are not owned
by the trash entry.

Each removal gets a distinct ID. Reusing an environment name must never
replace an earlier trash entry. Removal reports that ID and how to restore
it, plus any prefix and snapshots left in place.

There is no automatic expiration. A failure to trash data never falls back
to permanently deleting it.

### 2. Commands

```sh
wineo trash list
wineo trash restore <ID>
wineo prune <ID>
wineo prune --older-than 30d
wineo prune --all
```

The trash list shows the ID, original environment name, removal date, and
whether the prefix was included. IDs select entries without ambiguity when
several have the same name.

`prune` requires an explicit selection. No arguments show help and delete
nothing. ID, age, and all-entry selectors are mutually exclusive.

Before permanent deletion, show the selected entries and ask for
confirmation. `--yes` permits scripted use; without a terminal or `--yes`,
do not delete anything. Prune never follows references to a prefix or
snapshots that plain `rm` left in place.

### 3. Restore behavior

- Restore to the original environment name and storage paths. Refuse
  collisions rather than replacing existing data. Renaming on restore is
  outside this plan.
- Restore raw config, art, and stats together. For `--delete-prefix` entries,
  restore the retained prefix and snapshots too.
- For plain `rm`, the prefix and snapshots stay where they were. Restoring
  the config makes the existing snapshots accessible again. If the retained
  prefix has disappeared, refuse restoration with a clear explanation.
- Rebuild desktop entries from the restored config using normal desktop
  entry checks. Do not overwrite an unrelated shortcut.
- Restoration does not launch the environment.

### 4. Storage

Use Wineo-managed trash rather than desktop/XDG Trash. A manifest associates
the pieces of each entry and records their original and retained locations.

- Small metadata lives under Wineo's data directory, resolved through
  `Paths`.
- Prefixes and snapshots remain on their existing filesystems in
  Wineo-owned trash directories beside their original storage. Trashing a
  large prefix must not require copying it across filesystems.
- Portable trash stays inside the portable folder. Its location references
  are relative so that listing, restoring, pruning, and interrupted-operation
  recovery still work after the folder moves.
- Operation lock files stay in their existing host-state locations. They
  are coordination infrastructure, not removable trash contents.

Small metadata may need a durable copy before removing its source when
config and data roots are on different filesystems. Large payload movement
must either use its same-filesystem location or fail without deleting it.

### 5. Interrupted operations

Removal and restoration record durable progress before moving or removing
the affected data. A crash must leave enough information to recover without
guessing which copy is authoritative.

`validate` reports interrupted work and offers recovery. Normal operations
must not treat a partially removed or restored environment as ready to use,
and existing stray-data fixes must not remove data owned by a pending trash
operation.

Prune touches only payloads owned by the selected entry. If permanent
deletion fails partway through, retain the recovery information and report
what remains. Do not present that entry as fully restorable.

## Constraints for the implementation design

- Put removal, listing, restore, prune, and recovery operations in
  `wineo-core`; the CLI owns arguments, presentation, and confirmation.
- Preserve raw config bytes and existing art/stats contents rather than
  rewriting them as part of removal or restoration.
- Keep the current active-run, lost-run, unresolved-operation, shared-prefix,
  symlink-root, and prefix-layout checks when moving a prefix.
- Recheck reviewed state after acquiring operation and config locks. A
  concurrent config edit must not cause the wrong prefix to be moved.
- Coordinate trash entry operations as well as environment names and both
  original and retained prefix locations. Restore, prune, removal, and launch
  must not race over the same data.
- Validate manifest versions, ownership, and paths before acting. A damaged
  manifest or substituted symlink must not turn prune into deletion outside
  its owned payloads. Links inside a prefix are removed as links, not followed.
- Treat missing or unmounted storage as unavailable, not proof that a payload
  was successfully deleted. Do not discard the only recovery record.
- Native executable folders and paths referenced inside Wine prefixes are
  not additional payloads to copy or delete.

## Remaining decisions for review

The main behavior above is agreed. These edge cases need a maintainer
decision before implementation:

1. **Configs or prefixes already broken at removal time.** Plain `rm` can
   currently unregister an unreadable config, and config-first environments
   can legitimately have no prefix yet. Recommend retaining those configs
   unchanged and recording whether the prefix was present, absent, or unknown
   at removal. Apply the missing-prefix restore refusal when a known retained
   prefix has disappeared, while allowing a config that never had one to be
   restored. Define how an unreadable config is restored without losing the
   existing ability to remove it.
2. **Recovery direction.** Recommend rolling back interrupted removal to the
   registered environment, completing interrupted restoration, and resuming
   interrupted prune. Recovery must refuse newly introduced collisions.
3. **Transient runtime data.** Recommend cleaning up resolved runtime state
   and logs rather than restoring old process identities. Playtime is already
   retained separately. Confirm whether old logs should instead be retained.
4. **Portable copy.** Recommend copying only the active environment and its
   snapshots, art, and stats, not unrelated trash history. Moving the whole
   portable folder still carries its trash.
5. **Age selection.** Recommend positive whole-day values such as `30d`,
   measured as elapsed 24-hour periods since removal. Preview and execution
   use the same cutoff.

The implementation plan also needs a concrete manifest schema, trash directory
layout, ID format, and locking/recovery state transitions documented here
before approval. In particular, account for separately mounted snapshot
storage, original-path reuse, and existing Btrfs snapshot cleanup behavior.

## Proposed implementation sequence

1. Settle the remaining decisions and document the storage and recovery
   protocol in this plan. Obtain approval before implementation.
2. Add core trash records, path resolution, entry locking, and durable
   remove/restore/prune operations. Move removal policy out of the CLI.
3. Wire `rm`, `trash list`, `trash restore`, and `prune` to those operations,
   including confirmation, clear partial-failure output, and ID completion.
4. Integrate interrupted-operation checks and recovery with `validate` and
   ordinary environment operations.
5. Document the commands, retained-data rules, portable behavior, and recovery
   limitations. Update the README and feature roadmap.

## Verification and acceptance

- Plain removal and restore preserve raw config, art, and stats; the prefix
  and snapshots stay untouched. Pruning this entry leaves them untouched too.
- Prefix-inclusive removal and restore round-trip Wine and Proton layouts
  and snapshots. Prune deletes only the retained payloads.
- Native removal never touches its executable folder. Restore rebuilds the
  expected desktop entries without overwriting unrelated entries.
- Repeated removal under the same name produces independent IDs. Restore
  collisions leave existing data and trash unchanged.
- Active, lost, unresolved, shared-prefix, and symlink cases preserve the
  required refusals. Concurrent changes are caught under the correct locks.
- Exercise config/data roots on different filesystems and separately mounted
  prefix storage. No large-payload cross-filesystem copy fallback occurs.
- Portable trash and pending recovery remain usable after moving the folder.
- Inject failures at each durable transition of remove, restore, and prune.
  Recovery preserves retained data and never deletes newly created data at an
  original path.
- Missing mounts, damaged manifests, substituted paths, and partial prune
  failures leave actionable records rather than reporting success.
- CLI tests cover explicit selectors, age boundaries, confirmations,
  noninteractive `--yes`, and cancellation without mutation.
- The maintainer removes and restores a real environment using both forms,
  verifies playtime/art/snapshots, and prunes a disposable trash entry.
- Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`.

## Scope limits

- No automatic expiry, restore under a new name, or desktop trash integration.
- No permanent-delete bypass on `rm`; `prune` owns permanent deletion.
- No garbage collection of runners, downloads, components, arbitrary game
  folders, or snapshots left behind by plain `rm`.
- No new backup or cross-filesystem copying subsystem for large payloads.
- No GUI implementation.
