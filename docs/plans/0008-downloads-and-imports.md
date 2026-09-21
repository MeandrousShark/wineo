Status: approved

# 0008: Downloads and prefix imports

## Goal

Download runners and graphics tools without manually extracting archives, and
bring existing prefixes into wineo without reinstalling their applications.
Import either the prefix alone or a manually added Heroic game's prefix and
supported launch settings. Reuse 0007's review and target setup flows.

## Decisions

Settled by the maintainer:

- 0007 is complete. Downloads and prefix importing are the next two features.
- Import supports both copying and adoption in place. Copying is the default.
- Downloads initially cover GE-Proton, Proton-CachyOS, DXVK, and vkd3d-proton.
  Installing winetricks or umu is outside this plan.
- Heroic import covers manually added games and their launch settings.
  Epic, GOG, and Amazon library integration is outside this plan.
- External game folders stay in place. Copying a prefix does not copy game
  files, saves, or other data stored outside that prefix.
- When Heroic's selected runner is missing from wineo, offer to copy that
  existing runner into wineo's runner root.
- Support an opt-in runner-family alias such as `GE-Proton-Latest`, so an
  environment follows newer installed releases of that family.

The maintainer approved this plan and requested runner-family latest aliases.

## Existing foundation

- Runners and components already have configurable drop-in roots, name/layout
  validation, and local listings. Downloads can publish into those roots.
- `new --dir` adopts an existing prefix. It does not copy one and currently
  runs a registry operation to disable Wine's menu builder during adoption.
- A Wine environment uses the prefix directory itself. A Proton environment
  uses the compatdata parent, with the Wine prefix in `pfx/`. Crossing runner
  kinds is refused; import must not silently convert layouts.
- Targets already support executable, argument list, working directory,
  default selection, display name, desktop entry, and gamemode. Environment
  variables belong to the environment, so import one Heroic game per wineo
  environment rather than combining games with different settings.
- Prefix and environment operation locks, config write locks, target review,
  and desktop reconciliation are reusable. There is no recursive copy or
  download subsystem yet.

## Downloads

### Commands and selection

Command contract:

```text
wineo download                              # guided provider/version selection
wineo download ge-proton --list              # available releases
wineo download ge-proton                     # latest supported stable release
wineo download ge-proton --version <tag>     # an exact upstream release
wineo download proton-cachyos --version <tag>
wineo download dxvk
wineo download vkd3d
```

Existing `runners`, `dxvk`, and `vkd3d` commands remain local listings and never
require a network connection. The downloader prints the concrete release,
asset, size, verification method, and destination before downloading, followed
by progress and the installed name. Fully specified commands do not prompt.
An exact tag never falls back to another release on failure.

Use the official upstream GitHub release APIs, with separate provider rules
for selecting a runnable archive. Exclude drafts, prereleases, source archives,
DXVK-native builds, and incompatible architectures from default selection.
List older releases with pagination. `latest` means the latest stable release
with a supported asset for that provider, not a lexical tag comparison.

Initially support Linux x86_64 release assets. Prefer the baseline x86_64
Proton-CachyOS SLR asset; optimized CPU variants and ARM builds are deferred.
Do not select the first asset by filename order. Reject ambiguous matches and
unsupported hosts with a useful explanation. Provider tests cover historical
and current asset names, including explicit architecture suffixes.

Download each version into a distinct directory under the configured runner
or component root. Derive a stable, valid installed name from the provider,
release, and variant so different builds cannot collide. Show that name in
the release listing. Do not install a mutable directory called `latest`.

Installing a build does not rewrite environment configs, replace an installed
version, or change an explicitly pinned runner choice. Runner aliases resolve
as described below. Existing component settings of `latest`
still resolve to the newest local build at the next reconciliation, and the
download result calls out this existing behavior. New environments using the
gaming template likewise keep its existing newest-installed-Proton behavior.

### Runner-family latest aliases

```text
wineo set hades runner=GE-Proton-Latest
wineo set hades runner=Proton-CachyOS-Latest
```

These opt-in aliases resolve to the newest compatible stable release installed
locally within that family. Resolve versions numerically, including release
dates where used, rather than lexically or by installation time. Installing an
older release later must not move an alias backward. Neither alias crosses
families, checks the network, or downloads anything during launch.

Keep the alias in the environment config. Resolve it to one concrete runner
at the start of each operation and retain that runner for the entire operation,
including installer cleanup and lost-run recovery. A newly downloaded version
affects the next launch, never an already running game. Runtime records and
status must identify the concrete version actually used, while config and
runner listings show the alias and its current resolution.

Use provider/version metadata from completed downloads, or unambiguous upstream
family/version names for manual drop-ins and copied Heroic builds. Validate the
runner layout before making it a candidate. Unknown family/version information,
prereleases, incomplete installs, and incompatible variants are not candidates.
If no candidate exists, fail with download/selection guidance rather than
falling back to system Wine or another Proton family.

Aliases are logical selectors, not mutable runner directories or symlinks.
Reserve the two alias names; an existing filesystem entry using either name
must produce a conflict requiring resolution, not silently change meaning.
Expose aliases in guided runner choices as well as `new`, `set`, and templates.
Existing configs naming a concrete version stay pinned. Choosing a concrete
older version remains possible, with the existing prefix-compatibility notice.

### Verification, extraction, and failure

- HTTPS only. Handle redirects, timeouts, rate limits, interrupted responses,
  and missing assets explicitly. Public downloads need no account or token.
- Verify the archive before extraction using the upstream checksum asset
  where provided, or the release API's SHA-256 asset digest. If both exist,
  verify both. Match a checksum to the exact selected asset. Missing, malformed,
  unsupported, or mismatching digests fail closed; do not offer an unchecked
  install flag. Existing manual drop-ins remain usable.
- Support tar archives compressed with gzip, xz, or zstd. These formats are
  used by the selected upstreams; do not assume every provider ships gzip.
- Extract into a private staging directory on the destination filesystem.
  Reject absolute paths, traversal, device entries, and link chains that write
  outside staging. Preserve executable permissions and valid internal links.
  Never execute archive setup scripts or binaries to validate the download.
- Validate the expected runner/component layout, then publish with an atomic
  rename that refuses an existing destination. Hold an install lock for that
  resolved destination so concurrent installs cannot overwrite each other.
- Record provider, release, asset identity, and verified digest in a small
  install receipt. An identical completed install is a no-op; an untracked or
  different existing directory is a collision, not permission to replace it.
  Receipts are optional for manual drop-ins and never determine launchability.
- Cancellation or failure removes only this attempt's staging files. A
  process killed during download leaves no visible installed build; a later
  retry can clean its abandoned staging area under the install lock. No
  download resume, updater daemon, build removal, or automatic garbage
  collection of installed versions in this plan.

Keep network/provider logic and installation in core, with progress events
consumed by the CLI. Use injectable metadata and HTTP fixtures for tests.
Ordinary tests never depend on live GitHub or download real runners.

Guided creation/import can offer the same downloader when a runner or build
is missing. Downloads are an explicit choice, happen before prefix mutation,
and return to the pending review. Do not add network access to ordinary
launches or silently download on a missing dependency. umu's existing runtime
downloads remain its responsibility.

## Generic prefix import

### Commands and review

```text
wineo import                                      # choose prefix or Heroic
wineo import prefix /path/to/prefix                # guided import
wineo import prefix /path/to/prefix -n hades --runner <name>
wineo import prefix /path/to/prefix -n hades --runner <name> --adopt
wineo import prefix /path/to/prefix -n hades --runner <name> --dir /destination
```

Copy to `<prefix_root>/<name>` by default. `--dir` changes the copy destination
and conflicts with `--adopt`. Fully specified generic imports register only
the prefix; `wineo setup <name>` adds targets afterward. Guided imports offer
the existing target picker after registration, with an explicit skip option.
No targets is a valid successful import, not an unfinished installation.

Reuse the terminal requirement, editable review cards, back/cancel behavior,
and Ctrl+C/EOF handling from 0007. Before writing, show the source, destination
or adoption choice, detected layout/architecture, selected runner, and what
files/settings will be imported. Noninteractive commands fail on missing
required information instead of acquiring prompts.

### Prefix data and layout

- Copy the complete prefix tree, including registry, installed applications,
  and prefix-local saves. For Proton, copy the complete compatdata directory,
  including `pfx/` and adjacent Proton metadata. This is not a save-only import
  or a merge into an existing wineo environment.
- Detect Wine versus Proton from the filesystem, then validate architecture
  against the selected runner. Accept a Proton `pfx/` path by identifying and
  reviewing its parent compatdata directory. Use that parent consistently for
  locks, copying, saved config, and removal. The copy destination is also the
  compatdata parent. If both direct and nested registry markers exist, require
  an explicit runner/layout choice rather than using marker order to decide.
- Import requires a readable `system.reg` and a `drive_c` directory at the
  resolved Wine-prefix location. Read architecture from the registry header;
  offer `--arch win32|win64` only to resolve an absent marker, never to override
  a conflicting detected value. Reject win32 Proton imports. These are
  structural checks, not a guarantee that the application or prefix is healthy.
- Require an existing, valid prefix. Do not run wineboot, initialize an empty
  source, translate Wine to Proton, apply a template, replace installed DLLs,
  or import creation defaults over existing data. Preserve registry settings.
  Factor out registration from `Environment::create` so import does not inherit
  its registry mutation. Normal launch-time menu-builder suppression remains.
- Require a compatible installed runner, selected explicitly or in the guided
  flow. A different version of the same runner kind is a visible choice and
  may update the prefix on its first launch. Never select a different runner
  kind merely because it is the global default.
- Preserve symlinks without following them while copying. Rebase absolute
  links that resolve inside the copied tree, including Proton layout links;
  keep external mappings such as `dosdevices/z:` as links. Review external
  dependencies, including linked DLLs and saves, so a copied prefix is not
  presented as self-contained. Do not copy the host filesystem through links.
- Leave external game folders and saves where they are. Keep executable and
  working-directory paths outside the prefix pointing to those locations,
  and identify them in review. Copying and organizing portable application
  folders remains deferred from 0007.
- Preserve file contents, modes, timestamps, and hardlink relationships where
  supported. A normal copy is sufficient; reflinks are an optional optimization,
  never a prerequisite. Reject unsupported special files with their paths.
  Do not attempt arbitrary rewrites of registry strings, arguments, or saves.

### Coordination, recovery, and removal

Resolve aliases before checking paths. Reject a source equal to, inside, or
containing the destination, and refuse any existing destination or registered
name. Never merge, overwrite, move, or delete the source. Adoption explicitly
shares the original files; subsequent launches and updates will modify them.

Acquire operation locks for the new environment and source/destination prefix
identities in a stable order. Refuse tracked running games, lost runs, and
unresolved installer operations affecting either prefix. Find every wineo
environment sharing a resolved source/destination prefix and inspect its
runtime state; checking only the new name would miss another environment's
lost run. Unreadable configs that prevent establishing this fail with a path
and explanation. Recheck while the prefix locks are held. Explain before import
that the source game and launcher must be idle: wineo locks do not coordinate
with Heroic or other external applications. Reject a source that changes during
inspection/copy when detected; do not claim this is an atomic live snapshot.

Copy into staging beside the destination, validate the finished tree, and
publish without replacement. Recheck the name and reviewed inputs under the
config lock before registering. Registration writes config atomically after
the data is ready. A copy failure/cancellation before publication leaves no
environment; a config failure after publication retains the complete copied
prefix and reports its path plus a retry using adoption. Never clean up a
published prefix as if it were temporary data.

If target selection is canceled after registration, keep the imported prefix
and config and print `wineo setup <name>`. Desktop reconciliation failures
report which config was saved and can be retried through setup. Normal import
does not launch the game automatically.

Keep existing removal semantics: `rm` removes registration and wineo's desktop
entries; `rm --delete-prefix` explicitly removes the configured prefix, including
an adopted original. Its shared-prefix and symlink protections still apply.
The import review explains the effect for the selected mode. Heroic metadata
and external files are not implicitly owned by wineo.

## Heroic import

### Discovery

```text
wineo import heroic                              # pick a manually added game
wineo import heroic --list                        # read-only candidate listing
wineo import heroic --config-dir /path/to/heroic
wineo import heroic --game <id> -n hades --runner <name>
```

Use a one-time, read-only adapter over Heroic's local metadata. Discover native
XDG and Flatpak configuration directories; if both contain libraries, show
their origins and require a selection. An explicit config directory supports
custom/portable installations. No Heroic process connection, login, store API,
or persistent synchronization is needed.

Read `sideload_apps/library.json`, the selected `GamesConfig/<id>.json`, and
the relevant settings in `config.json`. Import only manually added Windows
games with an existing prefix. Show native/browser entries and invalid entries
as unsupported with a reason. Handle IDs as data, never unchecked path segments.
An invalid game entry must not prevent listing valid ones.

Build supported Heroic schema fixtures from its public source and sanitized
examples, including legacy/unversioned and versioned settings. Reproduce
global/per-game precedence for supported fields, preserving explicit false,
empty values, and array replacement behavior. Unsupported schema versions
fail clearly and offer generic prefix import; never rewrite Heroic's files.

### Bringing over an existing runner

Resolve Heroic's runner metadata to a complete Wine or Proton build directory,
not just its executable. Offer a compatible already installed wineo runner,
copying the selected Heroic runner, or downloading a supported runner. Show
when the selected version differs from Heroic's. Never silently use the global
default or switch from Wine to Proton.

Runner copying uses the same staged publication, destination locks, collision
rules, and structural validation as downloaded installs. Copy complete build
contents and preserve/rebase internal links; report external dependencies that
keep it tied to Heroic or Flatpak. A Flatpak-only system runner is not a portable
build to copy: ask for an installed or downloaded replacement instead. System
Wine can map to wineo's system `wine` when the host runner is usable.

Local copies receive provenance identifying their source path and runner name,
not a claim of upstream checksum verification. Reject changed/incomplete source
trees. If the desired destination name already holds a different build or an
untracked manual drop-in, require choosing it explicitly or naming the copy
differently; a matching display name alone never establishes identical content.

Complete runner installation before prefix publication and registration.
Canceling later leaves a successfully installed runner available for other
environments and says so. Original Heroic runners are never changed or removed.

### Launch settings

The review separates settings carried over, settings needing a choice, and
unsupported behavior. Proposed initial mapping:

| Heroic information | wineo result |
|---|---|
| Title | Suggested environment/target identifiers and desktop display name. |
| Prefix and runner | Validated source layout and compatible wineo runner choice. |
| Executable or effective `targetExe` override | Default target, with copied-prefix paths rebased. |
| `launcherArgs` | Argument list parsed with Heroic-compatible quoting, never a shell command. |
| Working directory | Executable directory, matching sideload launch behavior; editable in review. |
| Effective `enviromentOptions` | Environment `[env]`, preserving effective values and explicit empty strings. |
| `useGameMode` | Explicit environment gamemode setting, including false. |
| Esync/fsync, FSR, FPS display/limit, NVAPI, and GPU selection | Tested Wine/Proton-specific environment translations where behavior is representable. |

Initial Linux toggle translations follow Heroic's effective launch environment:

| Setting | Translation |
|---|---|
| Esync/fsync enabled with Wine | `WINEESYNC=1` / `WINEFSYNC=1`. |
| Esync/fsync disabled with Proton | `PROTON_NO_ESYNC=1` / `PROTON_NO_FSYNC=1`. |
| FSR | `WINE_FULLSCREEN_FSR=1` or `0`; when enabled, `WINE_FULLSCREEN_FSR_STRENGTH` uses the effective sharpness. |
| FPS display/limit | `DXVK_HUD=fps` when enabled; `DXVK_FRAME_RATE` from the enabled limit. |
| Proton NVAPI | Enabled: `PROTON_ENABLE_NVAPI=1` and `DXVK_NVAPI_ALLOW_OTHER_DRIVERS=1`; disabled: `PROTON_DISABLE_NVAPI=1`. |
| Wine NVAPI | When enabled, `DXVK_ENABLE_NVAPI=1` and `DXVK_NVAPI_ALLOW_OTHER_DRIVERS=1`; report the external DLL dependency. |
| GPU selection | When enabled, `DRI_PRIME=1`, `__NV_PRIME_RENDER_OFFLOAD=1`, and `__GLX_VENDOR_LIBRARY_NAME=nvidia`. |

Do not synthesize other toggle variables when Heroic leaves them unset. These
translations preserve configuration intent, not a guarantee that a replacement
runner supports the same features. Wine NVAPI import does not install its DLLs.

Show original argument text and the resulting argument boundaries. Invalid
quoting requires correction; never fall back to splitting on spaces. Preserve
Heroic's effective precedence between explicit variables and translated
toggles. Show additional wineo global settings that will affect the imported
environment rather than promising identical behavior under different defaults.

Reject conflicting managed variables such as `WINEPREFIX` and `PROTONPATH`;
resolve them through the prefix/runner choices. Do not infer a store or umu
game ID from a manually added game's identifier. Existing DLLs and registry
overrides stay in the copied/adopted prefix; Heroic's auto-install component
toggles do not enable wineo component management or download newer DLLs.

Report wrappers, before/after scripts, gamescope, MangoHud, unsupported runtime
and architecture toggles, and Heroic-managed anticheat/runtime paths as needing
manual setup. Do not execute scripts, translate arbitrary wrappers into shell
commands, or copy launcher credentials. Flatpak-only paths require a host-side
replacement. Unknown launch-affecting fields are reported, not silently assumed
irrelevant. Guided import requires review of omissions; noninteractive import
fails on unresolved launch settings and prints how to use the guided flow.

One invocation imports one game. Reimport refuses an existing wineo name and
never merges configs. Games sharing a Heroic prefix are identified in review:
copying imports the entire shared prefix, while adoption continues sharing it.
An imported game can be managed independently with normal `setup` and `set`.

## Scope limits

No store libraries, credentials, cloud-save synchronization, launcher hooks,
automatic update checks, bulk library import, prefix format migration, merge
into an existing prefix, runner deletion, snapshots, general backup/restore,
new GUI, Lutris/Bottles integration, or system package installation.
Managed copying of external game/portable application folders is also deferred.

## Testing and acceptance

- Download fixtures for each provider, architecture/variant selection,
  pagination, unavailable versions, rate limits, checksum failures, truncated
  downloads, gzip/xz/zstd, hostile archive entries, legitimate links, layout
  validation, collisions, cancellation, retry, and concurrent installation.
- Generic imports for Wine and Proton, source `pfx/` normalization, architecture
  mismatch, external mappings, internal absolute links, paths with spaces,
  shared prefixes, destination nesting/aliases, running/lost operations,
  source changes, disk/write failures, cancellation, and config failure after
  publication. Verify source content remains unchanged for copy and adoption.
- Runner aliases for numeric version ordering, family isolation, manual and
  copied builds, missing candidates, reserved-name collisions, incomplete or
  incompatible builds, pinned configs, and installing an older release later.
  Download a newer version during a fake active run/installer and verify that
  cleanup and recovery retain the original concrete runner, while the next
  operation resolves the new one. Assert that alias resolution stays offline.
- Heroic fixtures for native/Flatpak/custom locations, schema handling,
  inherited settings, false/empty overrides, duplicate environment keys,
  quoting, runner mismatches, executable overrides, unsupported settings,
  shared prefixes, missing data, and name collisions. Check generated launch
  arguments/environment using fake runners, without starting Heroic.
- Local runner copying for Wine/Proton layouts, executable modes, internal and
  external links, Flatpak-only runners, name collisions, source changes,
  concurrent installs, cancellation, and a completed runner surviving a later
  canceled prefix import.
- CLI/PTY coverage for review/back/cancel, redirected input, scriptable modes,
  help/completion registration, missing-build download choices, and unchanged
  existing local listings and `new --dir` behavior.
- Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`.
  Use ignored real-runner tests only with disposable prefixes and isolated XDG
  roots. Download smoke tests are explicit, not part of the ordinary suite.
- Manual acceptance with a maintainer-chosen sideloaded Heroic game: review
  settings, import a copy, launch, check arguments/environment/saves, stop,
  revisit setup, and remove registration. Separately exercise adoption and
  `rm --delete-prefix` on disposable copies. Confirm the original Heroic
  library and source prefix still work after a copied import.

## Phases

1. Shared download/verification/extraction foundation, the four providers, and
   runner-family latest aliases.
2. Generic prefix copy/adoption, registration, review, and recovery.
3. Heroic discovery/settings adapter and runner copying, shared guided download
   integration, and complete manual acceptance.

Commit each logical change separately. Each phase ends with fmt, clippy, and
tests passing. Amend the plan and obtain maintainer approval before implementing
scope or command-contract changes.

## Research references

Checked on 2026-09-21. Pin source revisions when preparing schema/asset fixtures;
these moving upstream references are evidence, not a promise of a stable API.

- Official releases: [GE-Proton](https://github.com/GloriousEggroll/proton-ge-custom/releases),
  [Proton-CachyOS](https://github.com/CachyOS/proton-cachyos/releases),
  [DXVK](https://github.com/doitsujin/dxvk/releases), and
  [vkd3d-proton](https://github.com/HansKristian-Work/vkd3d-proton/releases).
  Current assets include architecture variants and gzip/xz/zstd compression.
- [GitHub release asset API](https://docs.github.com/en/rest/releases/assets)
  documents asset identity, size, download URL, and digest.
- Heroic [sideload library store](https://github.com/Heroic-Games-Launcher/HeroicGamesLauncher/blob/main/src/backend/storeManagers/sideload/electronStores.ts),
  [configuration paths](https://github.com/Heroic-Games-Launcher/HeroicGamesLauncher/blob/main/src/backend/constants/paths.ts),
  and [game settings loader](https://github.com/Heroic-Games-Launcher/HeroicGamesLauncher/blob/main/src/backend/game_config.ts)
  establish the files and inheritance rules.
- Heroic [settings definitions](https://github.com/Heroic-Games-Launcher/HeroicGamesLauncher/blob/main/src/common/types.ts),
  [sideload launch behavior](https://github.com/Heroic-Games-Launcher/HeroicGamesLauncher/blob/main/src/backend/storeManagers/storeManagerCommon/games.ts),
  and [launcher environment handling](https://github.com/Heroic-Games-Launcher/HeroicGamesLauncher/blob/main/src/backend/launcher.ts)
  establish the fields, argument parsing, and setting translations to test.
