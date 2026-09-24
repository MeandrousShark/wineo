Status: done

# 0019: Portable layout

## Goal

Let one directory hold Wineo config, registered prefixes, and their snapshots
so it can be moved to another path or machine and selected explicitly. Keep
external games, runners, and downloaded components where they already live.
Support both new environments and copying existing ones into the directory.

## Existing foundation

- `Paths` currently maps config, data, and state to XDG directories. A single
  CLI-created `Paths` value is passed into all commands.
- Environment configs currently save prefix paths as absolute paths or `~/...`.
  Relative paths resolve from the command's working directory. Clone rebases
  internal target paths and links in a copied prefix; external paths stay put.
- Snapshots sit beside prefixes, but their manifests and interrupted-restore
  markers record absolute paths. Runtime records, locks, logs, desktop entries,
  and icons are tied to the host.

## Proposed decisions for maintainer approval

### Selecting and arranging a folder

- Add a global `--portable DIR` option and `WINEO_PORTABLE_DIR` environment
  variable. The option wins when both are set. Resolve the chosen root to an
  absolute path at command startup; no automatic search in the current
  directory or next to the executable. Without either selector, existing XDG
  behavior stays the same.
- Use `DIR/config/config.toml`, `DIR/config/environments/`,
  `DIR/config/templates/`, and `DIR/prefixes/`. Snapshots remain at
  `DIR/prefixes/.wineo-snapshots/<name>/`. Create missing directories as needed
  for a write; read-only commands do not create the folder.
- In portable mode, store paths inside the folder relative to its root and
  resolve them from that root, independent of the current directory. Keep
  absolute paths and `~/...` for deliberately external games and runners.
  Do not reinterpret Windows target paths as Unix paths. A portable prefix
  must be a direct child of `DIR/prefixes/`; reject new or edited prefixes
  anywhere else, and refuse a `defaults.prefix_root` other than
  `DIR/prefixes/` when it is set. Commands that show a prefix print the
  resolved absolute path, not the stored relative value.
- Continue to find runners, DXVK, and vkd3d-proton in their existing host
  locations unless the user config points elsewhere. Keep runtime state,
  locks, and logs in host XDG locations;
  isolate runtime state by portable root so same-named XDG and portable
  environments cannot share state or locks accidentally. Explain that moving
  the folder can require runner and external game paths to be updated.
- Key per-root host state by the resolved root path. Prefix locks are the
  exception: keep them in one host-wide directory keyed by the resolved
  prefix path, so XDG and portable environments that touch the same prefix
  still coordinate.
- An empty `WINEO_PORTABLE_DIR` counts as unset, an empty `--portable` is an
  error, and a leading `~/` in either selector expands to the home directory.

### New and existing environments

- `wineo --portable DIR new NAME` and the other normal environment commands
  operate within the selected folder. Include the portable selector in any
  detached supervisor invocation so it keeps using the intended folder.
- Portable environments never get desktop entries or icons. `set`, `new`,
  and imports refuse `desktop = true` on a portable target and `wineo
  desktop` refuses portable environments. A hand-edited `desktop = true` is
  ignored with a one-line warning on start. `portable copy` writes copied
  targets with `desktop = false` and lists that change in its review.
- Add `wineo portable copy NAME --to DIR` to copy an environment from the
  standard XDG layout into a portable folder. Keep the source unchanged. Copy
  its config, full configured prefix tree, and every snapshot for that prefix.
  When `DIR/config/` does not exist yet, copy the source global config and
  user templates too; otherwise keep the portable folder's versions, even if
  one of them is missing. Rebase the copied global
  `defaults.prefix_root` into `DIR/prefixes/`; preserve external runner and
  component roots. Refuse name or path collisions and any operation that would
  overlap the source. Review external paths and destination storage before
  copying.
- Use a staged copy and publish without replacement. Rebase internal Unix
  target paths, prefix symlinks, and portable config paths. When the
  destination is a portable folder (copy, import, or clone), write links that
  point inside the copied prefix or its snapshots as relative links so they
  survive a folder move. Snapshot create and restore in a portable folder
  follow the same rule. XDG copies and snapshots keep absolute rebasing. Preserve Windows
  paths and external paths. Do not rewrite arbitrary strings in arguments,
  environment variables, registry data, or files; report literal old-prefix
  paths for manual review. Reuse clone/import coordination, validation, and
  recovery behavior, including refusing active or unresolved operations.
- Copying snapshots may consume full space if source and destination cannot
  reflink. Their create and restore operations still require a reflink-capable
  destination under plan 0017's rules; report this before copying. Do not
  silently drop snapshots.

### Snapshot relocation

- Give portable snapshots a relative prefix identity in their manifests and
  use the selected portable root when validating it. Keep existing XDG
  manifests readable. A copied snapshot's saved config must point to the new
  portable prefix so restoring it after moving the folder does not revive an
  old absolute prefix path.
- Do not carry an interrupted restore marker into a portable copy. Refuse to
  copy while a restore needs recovery. Store portable restore marker paths
  relative to the portable root so recovery still finds the right files if
  the entire folder moves during an interrupted restore. Keep a portable
  restore marker inside the folder, beside that prefix's snapshots under
  `DIR/prefixes/.wineo-snapshots/<name>/`, so it moves with them. XDG
  markers stay where they are.

## Implementation sequence

1. Extend `Paths` and the CLI selector, including detached runs, runtime
   isolation, and command completion. Add root-relative path handling without
   changing XDG path interpretation.
2. Make normal creation, import, clone, set, target, and snapshot flows write
   and read portable paths. Keep desktop entries out of portable mode.
3. Implement staged copying from XDG into a portable folder, with snapshot
   relocation, external-dependency review, and explicit recovery messages.
4. Document setup, moving the folder, and external dependencies; update the
   feature backlog after implementation.

## Verification and acceptance

- Create a new portable environment, operate on it from unrelated working
  directories, move the whole folder, then list, launch, edit, snapshot,
  restore, and remove it from its new location.
- Copy an existing environment with targets and multiple snapshots. Verify
  the source is untouched, internal paths follow the destination, external
  game and runner paths remain explicit, and every snapshot survives a move.
- Test selector precedence, XDG behavior without a selector, same-named
  environments in different roots, path collisions, interrupted operations,
  non-reflink destinations, detached runs, and refused desktop entries.
- Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo
  test`, plus a real-prefix smoke test using a temporary portable folder.

## Scope limits

This moves Wineo's config, prefix trees, and snapshots. It is not a full
backup, and it does not copy external game directories, runners, downloaded
components, host runtime state, desktop entries, or icons. Wineo cannot
guarantee that external paths or a host's Wine installation exist on another
machine.
