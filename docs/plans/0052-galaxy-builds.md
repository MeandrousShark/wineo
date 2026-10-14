Status: draft

# 0052: Galaxy builds

## Goal

Install and update GOG Windows games through GOG's Galaxy content system,
so an update downloads only what changed, and a game can follow a beta
branch or roll back to an older build. Third of the three GOG plans, after
[0049](0049-gog-games.md) (account, migration, native updates) and
[0050](0050-gog-installs.md) (offline installers). It also closes 1.0
criterion 8 in [features.md](../features.md): Galaxy builds ship as a
regular 0.x release before 1.0, so their flags, JSON, and state get used
before they're frozen.

## Settled by the maintainer

From 0049 and 0050 (2026-10-10):

- **3, phased:** offline installers came first (0049, 0050); Galaxy builds
  for Windows come in this plan, with differential updates, beta branches,
  and rollback, "since only Galaxy builds offer those".
- The `wineo-gog` crate owns everything that talks to GOG, including the
  Galaxy downloader. Core owns what happens to games.
- **12A:** until this plan, a Galaxy-tracked game is compared by build and
  never updated. 12B (moving it onto offline installers) was turned down.
- **1A, 2A, 21A:** a GOG game is a normal Wine or Proton environment with
  `gog_id`; its installed state lives in its folder's state file, with a
  write-ahead journal.
- **14A:** a game mid-install or mid-update refuses to launch.
- **17A:** host root only.
- **19A:** `wineo update` installs newly owned DLC and retries failed or
  unavailable DLC.
- Not planned: Mac games, cloud saves, bonus content, posting playtime.
  Galaxy has no Linux builds (its content system answers 404), so native
  games stay on offline installers.

On 2026-10-14 the maintainer answered questions 1 to 11 below with A
each.

## What GOG offers

From reading heroic-gogdl 1.3.1, Heroic, and lgogdownloader 3.19
(2026-10-14). None of this is official documentation. Items marked
*inferred* weren't read in source; step 1 checks them live.

- **Builds.** `content-system.gog.com/products/<id>/os/windows/builds?generation=2`
  lists builds newest first (*inferred* from the docs and one game, Disco
  Elysium). Each has `build_id`, `version_name`, `branch` (null for the
  default branch), `generation` (1 or 2), `date_published`, and `link`. The
  same list mixes generation 1 and 2 builds. Private branches need a
  password (`&password=`), and `has_private_branches` says some exist.
  Rollback is installing an older `build_id`.
- **Generation 2 manifests.** The build's `link` is zlib-compressed JSON:
  `depots[]` (each with `productId`, `languages` that may include `*`,
  optional `osBitness`, and a manifest hash), `dependencies[]` (ids like
  `MSVC2017`, `DirectX`), `scriptInterpreter`, `installDirectory`, and
  `products[]`. Each depot manifest, at `cdn.gog.com/content-system/v2/meta/ab/cd/<hash>`,
  lists files (`DepotFile`: path with backslashes, chunks with `md5`,
  `compressedMd5`, sizes, and an optional whole-file `md5` or `sha256`,
  flags like `support` and `executable`), directories, and links.
- **Chunks.** Files are cut into chunks of about 10 MiB, zlib-compressed,
  and fetched by `compressedMd5` from a CDN whose URL template comes from a
  `secure_link` call per product. Links expire (*inferred*); gogdl never
  refreshes them.
- **Small-file containers.** Some depots bundle small files into one
  container (`sfcRef`). gogdl ignores it and those items still list their
  own chunks (*inferred* from lgogdownloader falling back to them).
- **Generation 1.** Older builds use a different, uncompressed format:
  byte ranges of one big `main.bin`, with no per-chunk hashes. gogdl keeps
  a second downloader for it.
- **Updates.** Tools diff the old and new manifests by lowercased path:
  new, changed, and deleted files. A changed file reuses matching chunks
  from the old copy. GOG also offers xdelta3 patches between some builds;
  gogdl falls back to chunks when there is none.
- **Setup.** A Galaxy download is bare files. The installer's own work
  (registry keys, redistributables) has to be done separately. For a
  `scriptInterpreter` game, Heroic runs GOG's script interpreter (ISI)
  from the dependency repository with the game's `goggame-<id>.script`,
  then each redistributable the manifest lists (VC++, DirectX, PhysX),
  silently, in the prefix. Games without the interpreter run their
  products' `temp_executable` instead. The ISI's registry writes are
  *inferred*.
- **Dependencies** come from `content-system.gog.com/dependencies/repository?generation=2`,
  a separate repository of depots in the same format. Ones under `__redist`
  are shared between games; the rest belong in the game folder.
- **A game installed by another tool** has no stored manifest. gogdl
  re-downloads every file. Heroic rebuilds a manifest from the build in
  `goggame-<id>.info`; lgogdownloader skips files whose size and MD5
  already match.

## Existing foundation

- **wineo-gog** fetches the build list (`Client::windows_builds`) and
  nothing else of Galaxy's: no manifest, depot, chunk, secure link, or
  zlib code, and no manifest mode in the fake GOG server (the CLI fake
  answers a placeholder link).
- **Core** has 0049 and 0050's folder state files, journals, locks, launch
  protection, download cache, space checks, Inno runs in a prefix, and
  launch targets from `goggame-<id>.info`. The state file already knows
  `Track::Galaxy` and a product's `build`; only Heroic migration creates
  that track today.
- **What this plan turns on.** Galaxy is refused or skipped in core
  (`UpdateError::Galaxy`, `Reason::Galaxy`, `installer_only`, the
  `adopt_build` gate, `by_build`), the CLI (`update --all` skips with a
  note), the app (**Update on GOG** with no action), and the docs
  (`gog.md`, `json-output.md`, `core-api.md`). Each becomes a working
  path, or a narrower refusal for what stays out of scope.
- **Not there yet:** redistributables. 0050's Windows installs never needed
  them, since Inno runs its own.

## Questions for the maintainer

Recommendations first. All answered A (2026-10-14); the design below
follows them.

### 1. Track for a new Windows install

- **A (recommended).** Galaxy, when GOG has a generation 2 build on the
  default branch, else the offline installer. `--track installer` picks the
  installer. This is what makes 0049's "Windows updates download only what
  changed" true for new installs.
- **B.** The offline installer stays the default; `--track galaxy` opts in.

### 2. Moving an installer-tracked game onto Galaxy

- **A (recommended).** `wineo update NAME --reinstall --track galaxy` moves
  it: wineo fetches the latest build's manifest, keeps files that already
  match it (question 4's check), downloads the rest, runs setup, and
  tracks builds from then on. `--track installer` moves a Galaxy game the
  other way with the offline installer. Files the old track left that the
  new one doesn't list stay, since wineo can't tell them from yours.
- **B.** No moves: a game stays on the track it was installed with.

### 3. Generation 1 builds

- **A (recommended).** Not supported. A game whose default branch has no
  generation 2 build installs from offline installers (question 1 picks
  that automatically), and "latest build" counts only generation 2
  builds. A Galaxy game sitting on a generation 1 build updates onto the
  latest generation 2 build as if it had no stored manifest (question 4).
  If a game only has generation 1 builds, it keeps today's **Update on
  GOG** with its reason.
- **B.** Support generation 1 too: a second downloader with range
  requests and no chunk hashes.

### 4. Games with no stored manifest

Disco Elysium came from Heroic, so wineo knows its build but has no record
of its files. The same holds after a track move.

- **A (recommended).** Fetch the manifest of the build in
  `goggame-<id>.info` and use it as the starting point. Files the update
  would keep are checked by size, and missing or differently sized ones
  are downloaded. A full hash check is `--repair` (question 9).
- **B.** Hash every file against the manifest first. Exact, but it reads
  the whole game, which takes minutes for a large one.
- **C.** Trust the old manifest without checking anything, like Heroic.

### 5. Setup in the prefix

- **A (recommended).** Do what Heroic does: download ISI and the game's
  redistributables from GOG's dependency repository into a shared
  `gog/redist` folder in wineo's data directory, and run them silently in
  the prefix after an install, after an update that changes the game's
  dependency list or scripts, and on `--reinstall` or `--repair`. The
  state file records which ran, so a new prefix (from a snapshot restore or
  a replaced prefix) gets them again on `--reinstall`.
- **B.** No setup: install only the game's files, and leave
  redistributables to winetricks and templates. Simpler, but games that
  need GOG's registry keys or a VC++ runtime fail to start.

### 6. Delta patches

- **A (recommended).** Out. Updates diff chunks only, which already
  downloads only what changed, file by file. It keeps the package's
  dependencies at glibc, libgcc, and xz.
- **B.** In: GOG's xdelta3 patches where offered, through a C library or
  binding, falling back to chunks.

### 7. Branches and rollback

- **A (recommended).** `--branch NAME` and `--build ID` on `import gog
  --game` and `update`. The state file remembers the branch, and `wineo
  update` follows it. `--build` pins the game to that build: `update
  --check` shows the newer build as available but `update --all` leaves a
  pinned game alone until `--branch` (or `--latest`) unpins it.
- **B.** Branches only, no rollback.

### 8. Private branches

- **A (recommended).** In: `--branch NAME` asks for the branch's password
  at a terminal, or reads `--password-stdin`, and keeps it in the keyring
  beside the GOG login (wineo's own, not part of the 1.0 contract). A
  wrong or revoked password says so.
- **B.** Public branches only in this plan.

### 9. Repair

- **A (recommended).** `wineo update NAME --repair` for Galaxy games: hash
  every file against the installed build's manifest, download what's
  missing or wrong, and run setup again. Files the manifest doesn't list
  stay. The `Repair` journal kind exists for this and is refused today.
- **B.** Later.

### 10. Where the file list lives

The folder state file is version 1 with strict fields, and a reader
refuses any other version. It isn't part of the 1.0 contract, but an older
wineo that can't read it can't launch the game.

- **A (recommended).** Keep the state file at version 1, which already
  has the Galaxy track and builds, and put the installed manifest (and a
  Galaxy journal's target) in a sidecar file beside it. 0.32 still reads
  the state, launches the game, and refuses to update it, as today.
- **B.** Version 2 of the state file, with the file list inside it. One
  file, but after a downgrade 0.32 refuses every Galaxy game's folder.

### 11. Desktop app

- **A (recommended).** Install and update on Galaxy, with the track in the
  install review, plus a **Version** picker in the game's panel for
  branches, a specific build, and **Repair**. It lands with the CLI's
  branches and rollback.
- **B.** Install and update only; branches, rollback, and repair are
  CLI-only for now.

## Proposed design

Part A is installs, updates, setup, and repair on the default branch. Part
B is branches, rollback, and private branches. They land in that order, so
the plan can split there if a review asks.

### wineo-gog: the content system

- Builds (existing), repository and depot manifests, the dependency
  repository, secure links, and chunk downloads, all in `wineo-gog`. zlib
  through `flate2`'s pure-Rust backend. Manifests get their own size cap,
  since a big depot manifest can pass the JSON cap of 16 MiB.
- A secure link is fetched per product (base game, each DLC, and the
  dependency store) and fetched again on a 401 or 403, instead of retrying
  a dead link like gogdl. CDN endpoints are tried in GOG's priority order,
  moving to the next on failure.
- Each chunk is checked twice: the compressed bytes against
  `compressedMd5` while downloading, and the decompressed bytes against
  `md5` before they're written. A whole-file `md5` or `sha256` is checked
  when the manifest has one.
- Small-file containers are ignored; files are built from their own
  chunks. Step 1's probe confirms those items carry chunks, and a
  manifest item with neither is refused with a clear error rather than
  written empty.

### Choosing a build

- The latest build is the first generation 2 build in GOG's list on the
  branch the game follows (the default branch unless question 7 set one).
  Step 1 also checks the newest-first order against `date_published` on a
  few games; if it doesn't hold, the plan sorts by date and says so here.
- Depots are kept when they belong to the base game or an owned DLC,
  match the language (or `*`), and match the prefix's arch where
  `osBitness` is given (gogdl downloads both).

### Installing

- `import gog --game ID` on the Galaxy track: resolve the build, fetch and
  filter manifests, check space, then create the environment and the
  folder's state with an `install` journal, as 0050 does.
- Unlike an Inno install, the manifest names every file, so the journal's
  `attempted` lists every entry wineo will write, like native installs,
  and cleanup after a failure is exact.
- Chunks download in parallel (4 at a time) and are written straight into
  place under temporary names, then renamed when a file is complete.
  Directories and links are created; links that would leave the folder are
  refused. Executable flags are ignored, since Wine doesn't need them.
- Paths: backslashes become slashes, and a path that matches an existing
  entry case-insensitively reuses that entry's name, like gogdl, since
  Windows games mix cases freely.
- The default target comes from `goggame-<id>.info`, as in 0050.

### Setup in the prefix (question 5)

- Dependencies download through the same chunk code into
  `<data>/gog/redist`, shared by every game and verified like game files.
  Ones outside `__redist` go into the game folder with the game's files.
- After the files, setup runs in the environment's prefix: ISI for each
  installed product when the manifest sets `scriptInterpreter`, else each
  product's `temp_executable`, then each listed redistributable with its
  own arguments, in the order the manifest lists them. Each run is
  supervised like 0050's installer runs, with a log in the run directory.
- A setup step that fails leaves the journal, so the game stays blocked
  and `wineo update NAME` retries setup without downloading again.

### Updating

- Diff the installed manifest against the target's by lowercased path.
  New and changed files download, reusing chunks that match the old
  copy. Deleted files are removed, then folders left empty by those
  deletions (gogdl added this because The Witcher 3 hangs on empty content
  folders).
- Changed files are staged under temporary names and renamed over the old
  ones, so the space check counts the staged copies, the chunk cache, and
  the downloads, minus deleted files.
- Setup reruns only when the dependency list or a product's script
  changed.
- DLC follows 0050: newly owned DLC installs with the update, and failed
  or unavailable DLC is retried.
- Interrupted updates resume from the journal and the sidecar. Files
  already renamed into place are checked against the target and kept.

### Games with no stored manifest (question 4)

- The installed build's manifest becomes the starting point, then the
  normal update runs with a size check on kept files. Heroic migration
  keeps the Galaxy track it already sets and stores nothing more until the
  first update.

### Repair (question 9)

- `wineo update NAME --repair` hashes each file against its chunks, then
  downloads and rewrites the ones that don't match, under a `Repair`
  journal, and reruns setup. Progress shows files and bytes checked.

### Branches, rollback, and private branches (part B)

- `--branch NAME` installs or updates from that branch and remembers it.
  `--build ID` installs that exact build and pins it. `--latest` returns
  to following the branch.
- `update --check` shows a pinned game's newer builds as available, with
  "pinned to build ID" as its note, and `update --all` leaves it alone.
- Private branch passwords are asked for at a terminal or read from
  `--password-stdin`, and stored in the keyring per game, beside the GOG
  login.

### State and journals (question 10)

- The state file stays at version 1. The installed manifest (filtered to
  what's installed), a Galaxy journal's target build and branch, and
  which setup steps ran go in a sidecar file beside it, replaced
  atomically and written before the state's commit like 0049's publishing
  order.

### Commands

- `import gog --game ID [--track galaxy|installer] [--branch NAME | --build ID] [--password-stdin]`.
- `update NAME [--reinstall [--track ...]] [--repair] [--branch NAME | --build ID | --latest] [--password-stdin]`.
- Reviews add the track, build, version name, branch, and setup steps.
  Progress shows bytes and files, as 0050's downloads do.

### Desktop app (question 11)

- The install review shows the track (Galaxy or Installer) when both are
  possible. Galaxy games update from the panel like installer games.
- A **Version** picker in the game's panel lists branches and recent
  builds, marks the installed one, and offers **Repair**.

### 1.0 contract impact

- New flags: `--track`, `--branch`, `--build`, `--latest`,
  `--password-stdin`, `--repair`.
- JSON: `applicable` becomes true for Galaxy updates. `update: galaxy`
  and `reason: galaxy` stay in the vocabulary for games whose only builds
  are generation 1 (question 3A); if 3 is B, they're removed like
  `windows_installer` was. New fields: `branch` and `pinned` on GOG games
  in `update --check --json` and `import gog --list --json`.
- No new exit codes.
- Not promised and changed: the folder state's sidecar, the redist
  folder, and the GOG keyring items.

## Implementation sequence

Part A:

1. A read-only live probe with the maintainer's login, on Disco Elysium's
   build `59781086834418085` and two or three other games: manifest
   shape, the secure link's `urls[]`, `sfcRef` items, build order, and
   how long a link lasts. Findings are recorded here before any code.
2. The fake GOG server gains a content system: builds, zlib manifests,
   depots, secure links, chunks, and the dependency repository, with
   failure modes (expired link, bad hash, missing chunk).
3. `wineo-gog`: manifests, secure links, chunk downloads, verification.
4. Core: Galaxy installs with the sidecar, journal, and exact cleanup.
5. Core: setup in the prefix (dependencies, ISI, redistributables).
6. Core: differential updates, games with no stored manifest, and resume.
7. Core: `--repair` and track moves.
8. CLI: flags, reviews, progress, JSON.
9. Desktop app: track in the install review and Galaxy updates.
10. Docs: `gog.md`, `json-output.md`, `core-api.md`, `stability.md` if it
    changes, and README.

Part B:

11. Branches and pinned builds in core and the CLI.
12. Private branch passwords.
13. The app's **Version** picker.
14. Docs for part B.

## Testing and acceptance

Unit and CLI tests run against the fake content system: an install
interrupted at each step, a chunk failing its hash, an expired link
refreshed mid-download, an update with new, changed, deleted, and
case-mismatched files, a game with no stored manifest, a failed setup step
retried, a repair that finds a damaged file, and a track move both ways. A
stub ISI and stub redistributables report their arguments and exit codes.

`--ignored` tests with `WINEO_TEST_GOG_LOGIN` install a small real Galaxy
game, update it across two builds by `--build`, and repair a file damaged
on purpose.

Acceptance, drafted after implementation, will include: Disco Elysium
going from **Update on GOG** to an update that applies, in the CLI, the
app, and `--json`; installing a small Windows game on Galaxy and checking
it starts; moving a game between tracks; rolling back and following a beta
branch; and repairing a damaged file.

## Scope limits

- Generation 1 builds (question 3), delta patches (question 6), Mac games,
  cloud saves, bonus content, and posting playtime aren't supported.
- wineo never deletes files a manifest doesn't list.
