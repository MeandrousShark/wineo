Status: approved

# 0052: Galaxy builds

## Goal

Install and update GOG Windows games through GOG's Galaxy content system,
so an update downloads only what changed, and a game can follow a beta
branch or roll back to a recent build. Third of the three GOG plans, after
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
each, and allowed a read-only probe with their GOG login.

After the first plan review (2026-10-14), the maintainer settled four
more points:

- **a.** Galaxy follows 0049's file rules exactly: an old file is deleted
  only when its content still matches what GOG shipped, your files are
  moved aside to `<name>.wineo-saved`, and files wineo never wrote are
  never touched.
- **b.** When the installed build's manifest can't be had (a generation 1
  build, no `buildId`, a build GOG no longer lists, or a game moving over
  from the installer track), the target build's manifest is the starting
  point and nothing is deleted. A file that already matches the target is
  kept; anything else at a target path is moved aside and downloaded fresh.
- **c.** Plain `wineo update NAME` on a pinned game changes nothing,
  exits 0, and says how to unpin.
- **d.** Logging out, or logging in as another account, removes saved
  private-branch passwords.

After the second plan review (2026-10-14), the maintainer answered
questions 12 and 13 with A.

After the third plan review (2026-10-14), the maintainer bounded the
sidecar's crash windows with one rule: the sidecar is evidence, never
authority. `installed` counts only when it matches the state's selection
digest and the info file's hash, `pending` only with a matching journal,
nothing is ever promoted, and deleting a file or overwriting it as tracked
needs a content hash match; whatever can't be shown falls back to answer
b. A window that only ends in answer b (a lost manifest, an extra download
or rehash) is an accepted limit; one that deletes or overwrites without a
hash match is a defect.

## What GOG offers

From reading heroic-gogdl 1.3.1, Heroic, and lgogdownloader 3.19, then a
read-only probe on 2026-10-14 with the maintainer's login: Disco Elysium,
SimCity 4, The Witcher Enhanced Edition, Papers, Please, HuniePop 2, and
Prison Architect, plus the dependency repository. None of this is official
documentation. Items marked *probed* were checked live.

- **Builds.** `content-system.gog.com/products/<id>/os/windows/builds?generation=2`
  lists builds with `build_id`, `version_name`, `branch` (null for the
  default branch), `generation` (1 or 2), `date_published`, `public`,
  `tags`, and `link`. *Probed:*
  - The list holds only the newest few builds of each branch: 5 of Disco's
    31, 10 of Prison Architect's 393. `limit`, `page`, and `offset` change
    nothing, so older builds can't be reached.
  - Within a branch, builds are newest first. Across the list, the default
    branch comes first and named branches after, so the list as a whole
    isn't in date order (Papers, Please), and one build can be listed on
    two branches.
  - Old games list generation 1 builds among generation 2 ones (SimCity 4,
    The Witcher, Papers, Please), but every probed game's latest default
    build is generation 2.
  - `has_private_branches` is true for four of the six. Private branches
    need `&password=`.
  - Disco's installed build, `59781086834418085`, is its latest.
- **Generation 2 manifests.** The build's `link` is zlib-compressed JSON:
  `depots[]` (each with `productId`, `languages` that may be `*`, optional
  `osBitness`, and a manifest hash), `dependencies[]`, `scriptInterpreter`,
  `installDirectory`, `products[]` (each with `temp_executable` and
  `temp_arguments`), and an `offlineDepot`. Each depot manifest, at
  `cdn.gog.com/content-system/v2/meta/ab/cd/<hash>`, lists `DepotFile`s
  (backslash paths, chunks with `md5`, `compressedMd5`, and sizes, an
  optional whole-file `md5` or `sha256`, and flags), `DepotDirectory`s,
  and `DepotLink`s. *Probed:* the largest depot manifest was 1.4 MB
  unpacked; `osBitness` splits depots into 32-bit and 64-bit sets in two of
  six games; the `offlineDepot` holds only a `project.json` and is skipped.
- **Files that aren't game files.** *Probed:* `goggame-<id>.info`,
  `.hashdb`, `.script`, and `.ico` are ordinary depot files. Files flagged
  `support` don't belong in the game folder: SimCity 4 has 433 (graphics
  rules, a `userdocs\` template tree), and The Witcher's setup program
  (`temp_executable`) is one. Tools keep them in a separate support folder
  per product and hand it to setup, which copies what it needs.
- **Chunks.** Files are cut into chunks of up to 10 MiB, zlib-compressed,
  and fetched by `compressedMd5`. *Probed:* `secure_link` (per product)
  returns two CDNs, fastly (priority 10) and gcore (`fallback_only`), each
  a URL template with a token that expires about 24 hours later. Chunks
  downloaded from fastly matched both their compressed and unpacked MD5s.
- **Small-file containers.** Depots bundle small files into a container
  (`sfcRef`). *Probed:* every one of 2,376 such items in the six games also
  lists its own chunks, so the container can be ignored, as gogdl does.
- **Generation 1.** Older builds use another format: byte ranges of one
  big `main.bin`, with no per-chunk hashes.
- **Updates.** Tools diff the old and new manifests by lowercased path. A
  changed file reuses matching chunks from the old copy. GOG also offers
  xdelta3 patches between some builds.
- **Setup.** A Galaxy download is bare files; what an installer would do
  happens separately. For a `scriptInterpreter` game, Heroic runs GOG's
  script interpreter (ISI) with `/DIR`, `/supportDir`, `/ProductId`,
  `/buildId`, `/versionName`, and language arguments; ISI reads the
  game's `goggame-<id>.script`. Games without it run each product's
  `temp_executable` from the support folder. Then each listed
  redistributable runs silently in the prefix. What ISI writes (registry
  keys, copied support files) isn't documented; step 5 checks it in a
  throwaway prefix.
- **Dependencies** come from `content-system.gog.com/dependencies/repository?generation=2`.
  *Probed:* 68 entries, each with `dependencyId`, `executable` (`path` and
  `arguments`), `internal`, a manifest hash, and sizes. ISI is one
  (`__redist/ISI/scriptinterpreter.exe`). Some have no executable at all
  (DOSBox, ScummVM, nGlide, `language_setup`); some are `.msi` packages
  (XNA, PhysX Legacy) that need `msiexec`; MSVC 2005's arguments nest
  quoted commands. Games list ids like `MSVC2017`, `DirectX`, and
  `language_setup`.
- **A game installed by another tool** has no stored manifest. gogdl
  re-downloads every file; Heroic rebuilds a manifest from the build in
  `goggame-<id>.info`.

## Existing foundation

- **wineo-gog** fetches the build list (`Client::windows_builds`) and
  nothing else of Galaxy's: no manifest, depot, chunk, secure link, or
  zlib code, and no manifest mode in the fake GOG server (the CLI fake
  answers a placeholder link).
- **Core** has 0049 and 0050's folder state files, journals, locks, launch
  protection, download cache, space checks, Inno runs in a prefix, and
  launch targets from `goggame-<id>.info`. 0049's publishing rules
  (`update/publish.rs`) already delete old files only on a hash match and
  move collisions aside.
- **The version 1 state file already fits Galaxy.** `Track::Galaxy`
  exists; a product records its `build`; a journal's `target` names the
  track and each product's build; and `attempted` lists each entry an
  operation will write with its type and MD5. Only Heroic migration
  creates the Galaxy track today.
- **What this plan turns on.** Galaxy is refused or skipped in core
  (`UpdateError::Galaxy`, `Reason::Galaxy`, `installer_only`, the
  `adopt_build` gate, `by_build`), the CLI (`update --all` skips with a
  note), the app (**Update on GOG** with no action), and the docs
  (`gog.md`, `json-output.md`, `core-api.md`). Each becomes a working
  path, or a narrower refusal for what stays out of scope.
- **Not there yet:** redistributables and support files. 0050's Windows
  installs never needed them, since Inno does that work itself.

## Questions for the maintainer

Recommendations first. Questions 1 to 11 were answered A (2026-10-14).
Questions 12 and 13 came from the second plan review and were answered A
the same day. The design below follows every answer.

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

### 12. Recovery when the target build is gone

Found in the second plan review. An interrupted install or update whose
target build GOG no longer lists, with the sidecar's copy of its manifest
also lost, can't be finished as it started.

- **A (recommended).** Retarget to the latest build of the branch the game
  follows. The review shows the new target and asks (`--yes` accepts), and
  files already published are treated like answer b: kept when they match
  the new target, moved aside otherwise, nothing deleted.
- **B.** Refuse, and leave the game blocked until `--reinstall` or `wineo
  rm`.

### 13. Reading Windows exit codes

Found in the second plan review. A Windows program's exit code reaches
Linux cut to 8 bits, so VC++'s 3010 ("restart needed") arrives as 194 and
can't be told apart from a real 194.

- **A (recommended).** Each setup step runs from a small batch file wineo
  writes into the run directory: it runs the program, then writes
  `%ERRORLEVEL%`, the full code, to a file beside it. That file decides
  success; a missing file is a failure. The batch file also keeps nested
  arguments like MSVC 2005's out of Wine's command-line quoting.
- **B.** Read the 8-bit code and treat 194, 105, and 102 as 3010, 1641,
  and 1638. Simpler, but a real failure with those codes would pass.

## Proposed design

Part A is installs, updates, setup, and repair on the default branch. Part
B is branches, rollback, and private branches. They land in that order, so
the plan can split there if a review asks.

### wineo-gog: the content system

- Builds (existing), repository and depot manifests, the dependency
  repository, secure links, and chunk downloads, all in `wineo-gog`. zlib
  through `flate2`'s pure-Rust backend. Manifests get their own size cap
  (64 MiB unpacked), well above the probed 1.4 MB.
- A secure link is fetched per product (base game, each DLC, and the
  dependency store), kept until a minute before its `expires_at`, and
  fetched again on a 401 or 403 instead of retrying a dead link like
  gogdl. Endpoints are tried by priority, with `fallback_only` ones only
  after the others fail.
- Each chunk is checked twice: the compressed bytes against
  `compressedMd5` while downloading, and the unpacked bytes against `md5`
  before they're written. A whole-file `md5` or `sha256` is checked when
  the manifest has one.
- Small-file containers are ignored; files are built from their own
  chunks. A file item with no chunks but a nonzero size is refused with a
  clear error rather than written empty.
- Secure-link URLs, their tokens, and private-branch passwords never
  appear in output, errors, logs, or `Debug` output, following 0049's rule
  for tokens; tests assert it for each error path.

### Choosing a build

- A game follows a branch: the default branch unless question 7 set
  another. Its latest build is the first generation 2 build GOG lists on
  that branch. Builds GOG no longer lists can't be installed, so rollback
  reaches only the few recent builds GOG returns, and `--build` names one
  of those: `GOG no longer lists build 5978... of Disco Elysium.`
- Depots are kept when they belong to the base game or an owned DLC,
  match the language (or `*`), and match the prefix's arch where
  `osBitness` is given.

### Files: game, support, and redistributables

- **Game files** go into the install folder. Backslashes become slashes,
  and a path that matches an existing entry case-insensitively reuses
  that entry's name, since Windows games mix cases freely.
- **Support files** (flag `support`) go into a per-game support folder,
  `gog/support/<folder key>/<productId>/`, in wineo's data directory,
  beside the folder's state file. Setup reads them from there. The folder
  is removed with the game's state.
- **Redistributables** live in a shared store, `gog/redist/` in wineo's
  data directory, one folder per dependency and manifest hash
  (`<dependencyId>/<hash>/`), so a newer version never replaces files a
  running setup is using. A version is downloaded into a temporary
  folder, verified, and renamed into place under that dependency's lock.
  Running one takes the same lock shared. After a successful install or
  update, versions no Galaxy game's record names are removed, each only
  when its lock can be taken at once.
- **Game-local dependencies,** those whose executable path doesn't start
  with `__redist` (including ones with no executable, like DOSBox and
  ScummVM), are part of the game: their files go into the install folder
  and join the installed manifest, with the same ownership, update,
  deletion, and repair rules as game files. They're never run as setup.
- **Store references.** An operation names every store version it will use
  in its sidecar's `pending` before downloading, and holds each version's
  lock shared from the moment the version is found or published until the
  operation ends, so nothing between download and setup can lose it.
  Cleanup takes a version's lock exclusively without waiting, rereads
  every Galaxy game's `installed` and `pending` under it, and removes the
  version only when none names it.
- ISI comes from the same store.

### The Galaxy record (question 10)

The state file stays at version 1 and stays authoritative. Its journal's
`target` already names the target build, so the sidecar,
`<folder key>.galaxy.json` beside it, holds only what version 1 has no
room for:

- `installed`: the filtered manifest of the committed selection (every
  file, support file, directory, and link with its chunks and hashes), with
  the selection's digest (below) and the hash of `goggame-<id>.info` at
  commit.
- `pending`: the target's filtered manifest, the journal `operation` it
  belongs to, the digest of the selection the operation will commit, and
  the store versions it uses.
- `follow`: the branch followed and the pinned build, if any.
- `setup`: which setup steps finished, each with what identifies it
  (below).

How the two files stay consistent:

- **Write order.** Before an operation's journal is written, the sidecar
  is replaced with `pending` set for the new operation. The journal comes
  next (0049's write-ahead rule), then the work. Before the state's commit,
  the sidecar is replaced again with `installed` set to the target's
  manifest and `pending` kept. Then the state commits, clearing its
  journal. The next sidecar write drops `pending`. Each write is a
  synced atomic replace, as the state file's is.
- **The selection digest** is a hash of the state's whole `committed`
  record: game id, platform, language, track, and every product with its
  build and status. A DLC-only change, a language change, or a retried DLC
  changes it, even when the base build stays the same.
- **Reading.** `installed` counts only when its digest matches the state's
  current `committed` and the info file's hash still matches.
  `pending` counts only when its `operation` matches the state's journal;
  with no journal it is dropped, whatever its target. Nothing is ever
  promoted from `pending`: a crash between the commit and the cleanup
  already finds `installed` matching the new `committed`, since it was
  written before the commit, and a `pending` whose journal never got
  written belongs to an operation that never started, even when its
  selection is the current one (a repair or reinstall). Anything else is
  stale and ignored.
- **When the sidecar is missing, corrupt, or stale,** the game is treated
  as having no stored manifest (answer b for repairs and track moves,
  question 4's manifest fetch for updates). Since b never deletes, losing
  the sidecar can't cost a file. A lost `follow` falls back to the default
  branch, unpinned, and the next check says so.
- **Another tool's build.** When `adopt_build` records a build that
  `goggame-<id>.info` reports, the digest and the info file's hash both
  change, so `installed` is ignored. This wineo also removes it under the
  state lock; an older wineo that adopts a build can't, but the info file's
  hash still catches it. If another tool goes from build A to B and back
  to A, an old record of A can match again. That's safe: every deletion
  is gated by a content hash (answer a), so a stale record can at worst
  keep a file it should have replaced, which `--repair` finds.
- **Track moves.** A move to the installer track commits the new track
  first, then removes the sidecar; a sidecar left behind by a crash is
  ignored, since only the Galaxy track reads it. A move onto Galaxy is an
  ordinary Galaxy operation that writes its record as above.
- **Recovery** finishes from the journal and `pending`: files already
  published are checked against `pending`'s hashes and kept, the rest are
  fetched again. Offline recovery succeeds only when nothing is left to
  fetch.
- **Recovery without `pending`** (lost, corrupt, or stale): the journal's
  `target` names each product's build, so wineo fetches those builds'
  manifests again and finishes the same target. When GOG no longer lists
  a target build, question 12 applies.

### Installing

- `import gog --game ID` on the Galaxy track: resolve the build, fetch and
  filter manifests, check space, then create the environment and the
  folder's state with an `install` journal, as 0050 does.
- The manifest names every file, so the journal's `attempted` lists every
  entry wineo will write, as native installs do, and cleanup after a
  failure is exact.
- Chunks download 4 at a time into temporary files in the target folder,
  which are renamed into place when complete. Links that would leave the
  folder are refused. Executable flags are ignored; Wine doesn't use them.
- The default target comes from `goggame-<id>.info`, as in 0050.

### Setup in the prefix (question 5)

- **Steps,** in order: ISI for each installed product when the manifest
  sets `scriptInterpreter`, with Heroic's arguments and the product's
  support folder as `/supportDir`; otherwise each product's
  `temp_executable` with its `temp_arguments`, run from its support
  folder; then each listed redistributable with its own arguments, `.msi`
  packages through `msiexec /i`. Each run is supervised like 0050's
  installer runs, with a log in the run directory.
- **Exit codes (question 13).** Each step runs from a batch file in the
  run directory that writes the program's full exit code to a file. 0 is
  success. For redistributables, 3010 and 1641 (a restart is needed) and
  1638 (a newer version is already installed) are success, and the first
  two are listed as "needs a restart" in the result, since Wine has
  nothing to restart. Anything else, or no code file, is a failure: the
  journal stays, the game stays blocked, and `wineo update NAME` retries
  from the first unfinished step without downloading again.
- **Recording.** A step is recorded in the sidecar's `setup` after it
  exits successfully. A crash between a step finishing and its record
  reruns that step, which is safe since these are installers built to run
  again.
- **What identifies a step,** so a change reruns it: for ISI, the product's
  build, the hash of its `goggame-<id>.script`, and ISI's own dependency
  manifest hash, so a new interpreter reruns it; for a
  `temp_executable`, its path, arguments, and file hash; for a
  redistributable, its dependency id and manifest hash. An update reruns
  the steps whose identity changed, plus any new ones. `--reinstall` and
  `--repair` rerun all of them, which also sets up a replaced prefix.
- **Shared prefixes.** Setup runs under the environment's operation lock,
  so two GOG games can't set up one prefix at once.

### Updating

- Diff the installed manifest against the target's by lowercased path.
  New and changed files download, reusing chunks that match the old copy
  (read and checked against their `md5` before reuse).
- **0049's publishing rules apply unchanged (answer a).** An old file is
  deleted only when its content matches a hash recorded for it; otherwise
  it's kept and listed. A changed game file you edited is overwritten and
  listed. A new path taken by something wineo doesn't track, or a type
  change over a changed entry, moves yours aside to `<name>.wineo-saved`
  and lists it. Folders emptied by deletions are removed (gogdl added this
  because The Witcher 3 hangs on empty content folders).
- **Space.** Changed files are staged beside the originals and renamed
  over them, and deletions happen only after publishing, so the check
  counts each filesystem's peak with no credit for deletions: staged new
  and changed files plus the chunk cache in the game folder's filesystem,
  new support files in the data directory's, and new redistributable
  versions in the store's, summed when they share one. Setup's own writes
  in the prefix can't be known ahead, so the review leaves them out and
  says so.
- DLC follows 0050: newly owned DLC installs with the update, and failed
  or unavailable DLC is retried.

### Games with no stored manifest (question 4, answer b)

- **The installed build is known and listed** (Disco Elysium): its
  manifest becomes the starting point, and files the update keeps are
  checked by size. Old files are deleted only on a hash match, as above,
  so a size check can't cost a file.
- **Otherwise** (generation 1, no `buildId`, an unlisted build, or a move
  from the installer track): the target's manifest is the starting point.
  Each existing file at a target path is hashed; a match is kept, anything
  else is moved aside and downloaded. Nothing is deleted, so files of the
  old build that the new one doesn't list stay. The review says so.

### Repair (question 9)

- `wineo update NAME --repair` hashes every file of the installed
  manifest, including support files, rewrites the ones that don't match
  under a `Repair` journal, and reruns setup. A file you changed is
  rewritten and listed, as an update would. Progress shows files and
  bytes checked.
- Same-size damage that question 4's size check lets through is found
  here; a test shows both.

### Concurrency

- A Galaxy install, update, repair, or track move holds the folder's state
  lock exclusively and the folder's in-use lock (0049), so it waits for or
  refuses a running launch, and a launch refuses while it runs.
- A snapshot restore or prefix replacement during setup is refused by the
  environment's operation lock, as in 0050.
- Two games updating to the same redistributable version share it; two
  different versions get separate folders.

### Branches, rollback, and private branches (part B)

- `--branch NAME` installs or updates from that branch and follows it.
  `--default-branch` returns to the default branch. `--build ID` installs
  a listed build and pins the game to it. `--latest` unpins and installs
  the latest build of the branch followed.
- **Pinned games (answer c).** `update --check` shows a newer build on the
  branch as `available` with `applicable: false` and reason `pinned`.
  `update --all` skips the game, and plain `update NAME` changes nothing:
  `disco is pinned to build 5978... (WINkay16062026_memfix). Run wineo
  update disco --latest to follow its branch again.` Exit 0.
- **Private branches.** `--branch NAME` on a private branch asks for its
  password at a terminal, or reads `--password-stdin`, and saves it in the
  keyring per game and branch (`account=gog-branch`, attributes `game` and
  `branch`). Entering it again replaces it. A refused password removes the
  saved one and says `GOG refused the password for branch NAME.`
  Logging out or switching accounts removes them all (answer d).

### Commands and flag rules

- `import gog --game ID [--track galaxy|installer] [--branch NAME | --build ID] [--password-stdin]`.
- `update NAME [--reinstall [--track galaxy|installer]] [--repair] [--branch NAME | --default-branch | --build ID | --latest] [--password-stdin]`.
- Conflicts are usage errors (exit 2): `--branch`, `--default-branch`,
  `--build`, and `--latest` exclude each other; `--repair` excludes
  `--reinstall`, `--track`, and the branch flags; `--track` on `update`
  needs `--reinstall`; `--password-stdin` needs `--branch`; `--all` takes
  none of `--repair`, `--track`, or the branch flags; branch flags and
  `--track installer` exclude each other.
- On a game on the installer track, the branch flags and `--repair` say
  `Only Galaxy games have branches. Move it with --reinstall --track
  galaxy.`
- Reviews add the track, build, version name, branch, support files, and
  setup steps. Progress shows bytes and files, as 0050's downloads do.

### Desktop app (question 11)

- The install review shows the track (Galaxy or Installer) when both are
  possible. Galaxy games update from the panel like installer games.
- A **Version** picker in the game's panel lists branches and the builds
  GOG returns for each, marks the installed one and a pin, and offers
  **Repair**. A private branch asks for its password in the panel.

### 1.0 contract impact

- **New flags:** `--track`, `--branch`, `--default-branch`, `--build`,
  `--latest`, `--password-stdin`, `--repair`, with the conflicts above.
- **JSON,** on GOG games in `update --check --json` and `import gog --list
  --json`:
  - `branch`: a string, or `null` for the default branch and for games on
    the installer track.
  - `pinned`: the pinned build id as a string, or `null`.
  - `applicable` becomes true for Galaxy updates wineo can apply, and is
    false for a pinned game, with the new `reason` value `pinned`.
  - `update: galaxy` and `reason: galaxy` stay, for a Galaxy game whose
    branch has no generation 2 build (question 3A).
- **No new exit codes.** A pinned `update NAME` exits 0.
- **Not promised:** the sidecar, the support and redistributable stores,
  and the GOG keyring items (the login and branch passwords), as with the
  rest of GOG's managed files in [stability.md](../stability.md).

## Implementation sequence

Part A:

1. Done (2026-10-14): the read-only probe, recorded in "What GOG offers".
2. The fake GOG server gains a content system: builds, zlib manifests,
   depots with support files and links, secure links with expiry, chunks,
   and the dependency repository, with failure modes (expired link, bad
   hash, missing chunk, a fallback-only CDN).
3. `wineo-gog`: manifests, secure links, chunk downloads, verification.
4. Core: the sidecar and its consistency rules, Galaxy installs with exact
   cleanup, support files, and the redistributable store.
5. Core: setup in the prefix. Before writing it, run ISI for SimCity 4 and
   The Witcher in a throwaway prefix and record what it writes (registry,
   copied support files, exit codes) here, and check the batch file's
   full exit codes and nested quoting under both Wine and Proton, since
   host-side stubs can't show either.
6. Core: differential updates on 0049's publishing rules, games with no
   stored manifest, the space check, and resume.
7. Core: `--repair` and track moves.
8. CLI: flags, reviews, progress, JSON.
9. Desktop app: track in the install review and Galaxy updates.
10. Docs: `gog.md`, `json-output.md`, `core-api.md`, and README.

Part B:

11. Branches, pinned builds, and `--default-branch` in core and the CLI.
12. Private branch passwords.
13. The app's **Version** picker.
14. Docs for part B.

## Testing and acceptance

Unit and CLI tests run against the fake content system. A stub ISI and
stub redistributables report their arguments and exit with chosen codes.
They cover:

- **Installs:** interrupted at each step; a chunk failing its hash; an
  expired link refreshed mid-download; a fallback-only CDN used after the
  first fails; support files landing in the support folder; 32-bit and
  64-bit depots filtered by arch.
- **The sidecar:** a crash between each sidecar and state write, in both
  orders; a missing, corrupt, and stale sidecar; a build adopted from
  another tool; a sidecar left after a track move.
- **Updates:** new, changed, deleted, and case-mismatched files; a deleted
  file you modified (kept and listed); an untracked file at a new path
  (moved aside); a type change; a game with no stored manifest of each
  kind; a low-space update with large deletions, with the folder and the
  data directory on separate filesystems.
- **Unstarted operations:** a repair and a reinstall of the current
  selection, each with no prior sidecar and a file that doesn't match,
  interrupted after `pending` but before the journal; the file must be
  handled by answer b afterwards, not treated as tracked.
- **Selection changes:** a DLC-only update interrupted before its
  journal, between the journal and the commit, and after the commit; an
  adopted build going A to B to A; a lost `pending` during an install and
  during a rollback, with the target still listed and not (question 12).
- **Setup:** each exit code class through the batch file, including a
  full code above 255 and a missing code file; a crash between a step and
  its record; a changed dependency version, a changed script, and a new
  ISI rerunning only their steps; two games installing different versions
  of one dependency at once; cleanup running between another game's
  download of a version and its setup.
- **Game-local dependencies:** installed into the folder, updated, and
  repaired like game files.
- **Repair:** same-size damage that an update's size check keeps, then
  `--repair` fixing it.
- **Concurrency:** a launch during an update, an update during a launch,
  and a snapshot restore during setup.
- **Part B:** each flag conflict; a pinned game under `update NAME`,
  `--all`, and `--check --json`; a refused, replaced, and logged-out
  password; no URL token or password in any error.

`--ignored` tests with `WINEO_TEST_GOG_LOGIN` install a small real Galaxy
game, update it across two listed builds with `--build`, and repair a file
damaged on purpose.

Acceptance, drafted after implementation, will include: Disco Elysium
going from **Update on GOG** to an update that applies, in the CLI, the
app, and `--json`; installing a small Windows game on Galaxy and checking
it starts; moving a game between tracks; rolling back and following a
branch; and repairing a damaged file.

## Scope limits

- Generation 1 builds (question 3), delta patches (question 6), Mac games,
  cloud saves, bonus content, and posting playtime aren't supported.
- Builds GOG no longer lists can't be installed.
- wineo never deletes a file whose content doesn't match what GOG shipped,
  and never touches files it didn't write.
- A crash at the wrong moment can make wineo forget a game's stored
  manifest, which costs a rehash or a download on the next update. It
  never costs a file (the bounding rule above).
