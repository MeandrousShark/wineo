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
  the config makes the existing snapshots accessible again. If a prefix known
  to be present at removal has disappeared, refuse restoration with a clear
  explanation. Prefixes already absent at removal do not block restoration.
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

## Follow-up decisions settled with the maintainer (2026-09-26)

The maintainer selected all nine recommendations: 1A through 9A.

1. **Configs or prefixes already broken at removal time.** Retain raw configs
   and record whether the prefix was present, absent, or unknown. Restore
   config-first environments whose prefix was already absent. Restore broken
   configs byte-for-byte with a warning and defer desktop setup until they
   are fixed. Refuse restoration if a known retained prefix has disappeared.
2. **Recovery direction.** Roll back interrupted removal to the registered
   environment, complete interrupted restoration, and resume interrupted
   prune. Recovery stops at newly introduced collisions.
3. **Transient runtime data.** Discard resolved runtime state and logs. Never
   restore old process identities. Retain playtime separately.
4. **Portable copy.** Copy only the active environment and its snapshots,
   art, and stats, without trash history. Moving the entire portable folder
   carries its trash.
5. **Age selection.** Accept positive whole-day values such as `30d`, measured
   as elapsed 24-hour periods since removal. Older means strictly older than
   the cutoff; freeze that cutoff for preview and execution.
6. **Storage layout.** Keep entry metadata at `<wineo-data>/trash/<ID>/` and
   prefix payloads at `<prefix-parent>/.wineo-trash/<ID>/`. Move snapshots
   alongside the prefix payload when they share its filesystem. Separately
   mounted snapshots use a trash area on their own filesystem. Portable
   equivalents stay inside the folder with relative references. Refuse a
   payload move that cannot stay on its filesystem rather than copying it.
7. **IDs and manifests.** Use a timestamp plus random suffix, for example
   `20260926T183000Z-a1b2c3d4`, with collision checks. Use a versioned TOML
   manifest recording original locations, owned payloads, and recovery
   progress.
8. **Recovery entry point and locks.** Ordinary commands detect pending work,
   block conflicting actions, and explain how to recover. Recovery is explicit
   through `validate --fix`, not silently attempted by ordinary commands.
   Lock the trash entry, environment name, and affected paths before changing
   anything.
9. **Batch prune failures.** Continue past independent failures, retain their
   records, summarize results, and exit nonzero if any entry failed. Mark
   partially pruned entries as unrestorable.

## Storage and recovery protocol requirements

The manifest must distinguish owned payloads from references left in place.
It records the entry ID, format version, original environment name, removal
time, prefix inclusion and original presence, operation phase, and progress
for each payload. Payload records need original and retained locations plus
enough identity information to distinguish the original data from a path
reused after interruption. Unknown manifest versions are refused.

The logical states are:

- `removing`: record progress before changing sources; recovery rolls back
  retained payloads to their original locations without replacing new data.
- `trashed`: removal committed; eligible for restore or prune.
- `restoring`: recovery resumes returning the saved payloads and registration.
- `pruning`: recovery resumes deletion of owned payloads only. Once permanent
  deletion starts, the entry is not offered for restoration.

Write and sync intent before each destructive transition, then persist its
completion. Recovery must recognize a completed filesystem operation even if
the subsequent progress write failed. Retain the manifest until restoration
or permanent deletion has completed, including cleanup of payload containers.
Runtime cleanup and desktop reconciliation must be accounted for without
making discarded runtime logs a prerequisite for removal rollback.

Acquire operation locks before config locks, revalidate the source and
selection under those locks, and use a consistent order for multi-path locks.
Persistent recovery records must let normal operations detect affected names
and paths even while the config is absent. Trash entry locks remain in host
state, outside payloads that prune deletes.

Separately mounted snapshot storage must use an owned payload location within
that filesystem, not a sibling outside its mount. If no suitable location can
be used, fail before mutation. Btrfs cleanup failures retain the manifest and
remaining payload locations rather than claiming the entry was pruned.

## Proposed implementation sequence

1. Obtain approval of this draft before implementation. The behavioral
   decisions above are settled.
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
