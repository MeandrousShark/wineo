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

After two code reviews of step 4 (2026-10-14), the maintainer bounded
races with other programs the same way. wineo places, moves aside, and
deletes with atomic Linux operations (a rename that refuses to replace,
and a rename to wineo's own name before hashing anything it might delete),
so it never overwrites or deletes an entry it hasn't checked, and the
folder lock keeps other wineo processes out. A program outside wineo
writing the same path in the moment between wineo's steps is an accepted
limit, not a defect. The 0.32.1 fix to the native installer publisher
follows the same rule.

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
  it's kept and listed. A changed game file you edited is moved aside to
  `<name>.wineo-saved` and listed, and the new one takes its place (the
  maintainer chose this over overwriting it as 0049 does, 2026-10-14, so
  nothing you changed is lost). A new path taken by something wineo doesn't track, or a type
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
  saved one and says `GOG refused the password for branch NAME.` (the
  probe in step 12 changed this: see its decisions)
  Logging out or switching accounts removes them all (answer d).

### Commands and flag rules

- `import gog --game ID [--track galaxy|installer] [--branch NAME | --build ID] [--password-stdin]`.
- `update NAME [--reinstall [--track galaxy|installer]] [--repair] [--branch NAME | --default-branch | --build ID | --latest] [--password-stdin]`.
- `update NAME --builds [--json] [--refresh]` lists the generation 2 builds
  GOG offers for a Galaxy game (maintainer, 2026-10-15), read-only.
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
- The Version picker also offers **Move to offline installers** for a
  Galaxy game (maintainer, 2026-10-15), and **Move to Galaxy** for an
  installer-track Windows game GOG has a Galaxy build of (maintainer,
  2026-10-16).

### 1.0 contract impact

- **New flags:** `--track`, `--branch`, `--default-branch`, `--build`,
  `--latest`, `--builds`, `--password-stdin`, `--repair`, with the
  conflicts above.
- **`update NAME --builds --json`:** `environment`, `id`, `track`,
  `branch` and `pinned` (what the game follows), `installed`,
  `private_branches`, and `builds`, each with `branch` (null for the
  default), `build`, `version`, `published` (GOG's timestamp), and
  `latest`. Ids are strings.
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

## Decisions during implementation

Steps 2 and 3 (2026-10-14):

- **Strict parsing.** Every Galaxy parser refuses a whole manifest over one
  unusable item, unlike product details, which skip odd entries: a skipped
  file would be a quietly incomplete install. A field that's present must
  have the expected type and range; `null` counts as absent. Unknown item
  types are refused. Every probed manifest parses.
- **Paths** are checked when parsed: both separators are accepted (Papers,
  Please uses `/`), and empty, `.`, and `..` parts, leading separators,
  `:`, and NUL are refused, in files, setup programs, and dependency
  executables. Link targets are kept as written for core to judge.
- **Size limits:** a chunk is at most 1 GiB (the largest seen is about 20
  MB, and a chunk is held in memory whole), other sizes at most 2^50
  bytes, and a chunk's compressed size must match exactly.
- **Refusals.** A CDN's 401 or 403, on a chunk or a manifest, is an HTTP
  status, never `Unauthorized`, so a dead CDN link can't make wineo renew
  the GOG login. A chunk refused again after one fresh link fails.
- **Bad data moves to the next CDN.** A hash, size, or unpacking failure
  counts as that CDN failing; when all fail, the last error is returned.
- **Expiry.** Only fastly's parameters carry `expires_at`. The earliest one
  given is used; with none, a link lasts until a CDN refuses it. One that
  can't be read refuses the link.
- **Not used:** the repository's `clientId` and `clientSecret` aren't kept,
  and `max_fails` is parsed but not acted on.

Step 4 (2026-10-14):

- **No Galaxy build.** An explicit Galaxy install of a game with only
  generation 1 builds says `GOG lists no Galaxy build of TITLE that wineo
  can install.` (wording approved by the maintainer). With no track
  chosen, such a game goes to the installer.
- **Language.** The record stores the depot's code (like `en-US`), or the
  requested tag when every depot is `*`. The pick goes exact code, then the
  same language in another region, then English, with a warning on a
  fallback. DLC follow the base game's code, then English; an owned DLC
  with no depot left is `unavailable` with a note.
- **Missing dependency.** One the build lists but GOG's repository lacks is
  warned about and skipped, as gogdl does. The script interpreter's id is
  `ISI`.
- **Store cleanup is conservative.** It keeps any version named by an
  `installed` or `pending`, whatever the digest or journal says. A record
  that doesn't parse names nothing, since store versions can be downloaded
  again.
- **`pending` is stricter than designed:** it also needs its selection
  digest to match what the journal would commit.
- **`attempted`.** A file that already matches the target is kept and not
  listed. Files whose MD5 only comes with the download are named in
  batches before they're placed.
- **Track choice** is a separate `track: Option<Track>` argument, not part
  of `install::Request`, so callers using `..Request::default()` stay on
  the installer. With no track, product details are fetched twice.
- **Store layout.** A store dependency's program path starts with
  `__redist/`. Locks are `gog-redist-<dep>-<hash>.lock` and
  `gog-redist-<dep>.lock`; dependency ids are `[A-Za-z0-9_.-]` without a
  leading dot.
- **Placing is atomic.** Files and links are placed with a rename that
  refuses to replace; whatever appeared is moved aside and settled. Before
  anything is deleted to make way, it's renamed to the operation's own
  `.wineo-aside-<op>` name, then deleted only when it's a file with a
  recorded hash or a link with a recorded target, else saved as
  `.wineo-saved`. Galaxy refuses a filesystem that can't rename without
  replacing; the native publisher keeps its fallback until 0.32.1.
- **Space under holds.** The store versions' shared holds are taken before
  the pending record, the journal, or the folder, and space is checked
  again with any version that cleanup removed since planning.
- **No graphical-session check** for a Galaxy install yet, since nothing
  runs in the prefix until step 5. `setup` in the record is a placeholder
  for step 5.

Step 5 probe (2026-10-14), Wine 11.19 and GE-Proton11-3 through umu
1.4.4, with SimCity 4 and The Witcher installed live through core's
Galaxy install:

- **Two live fixes first.** 14 of the 68 redistributables are game-local
  and list their files with a leading `/` (nGlide, language_setup, the
  GOG launcher shims, the DOSBox configurator); a redistributable's
  manifest strips one leading separator and keeps every other path rule,
  and game depots still refuse it. The redistributables' `open_link`
  answer has no parameters at all, just a plain URL per CDN with no token
  or expiry; a chunk's URL is that URL plus `/ab/cd/<hash>`.
- **Exit codes through the batch file** reach the code file exactly under
  both: 0, 1, 255, 256, 1638, 1641, 3010, 70000, 2147483647, -1, and
  -1073741819 (an access violation). The host process only gets the low 8
  bits (3010 arrives as 194, an access violation as 5), so the code file is
  the only signal. A missing program gives 9009.
- **Quoting.** A program started from the batch file gets the rest of the
  line verbatim: MSVC 2005's nested command line arrives byte for byte,
  and both MSVC 2005 runtimes install from it. Spaces, `&`, and
  parentheses inside quotes are fine. A `%` written in the batch file must
  be `%%`, or cmd drops it (`100% done` became `100 done`).
- **Non-ASCII paths break a batch file.** cmd reads it in the console code
  page, so a UTF-8 path like `SimCity™ 4` gave "File not found" (9009),
  even after `chcp 65001`, and SimCity 4's default folder has a `™`. Paths
  and the argument string passed in environment variables work under both,
  with `™`, accents, `%`, and `&`, and nested quotes intact: cmd expands
  each once and never expands inside the value.
- **Every step exits 0 with no window,** in 4 to 6 seconds (DirectX in
  19): ISI for SimCity 4; The Witcher's setup program, MSVC2005,
  MSVC2005_x64, and DirectX `/silent`. VC++ 2005 lands in WinSxS and
  DirectX's DLLs in system32.
- **A `temp_executable` gets ISI's arguments.** The Witcher's is an Inno
  "galaxy" installer whose `temp_arguments` is empty, so run as listed it
  would open its wizard. It gets the same arguments as ISI, as Heroic
  passes them (`/VERYSILENT /DIR= /Language= /LANG= /ProductId=
  /galaxyclient /buildId= /versionName= /nodesktopshorctut
  /nodesktopshortcut /lang-code= /supportDir=`), then any
  `temp_arguments` GOG lists.
- **What setup writes,** the same under both:
  - the registry: `HKLM\Software\Wow6432Node\GOG.com\Games\<id>` (build,
    exe, launch command, save folder, language, version), the game's own
    keys from its script, and compatibility layers for its play tasks;
  - the prefix: `ProgramData\GOG.com\supportInstaller\<id>\`, Start Menu
    shortcuts, a setup log in Temp, and the support data the script copies
    (SimCity 4's `{userdocs}` folder, done only once; The Witcher's seven
    adventure modules in `Public\Documents`). So the support folder is only
    read during setup, and a replaced prefix needs setup again;
  - the game folder: `gog.ico`, `support.ico`, `goglog.ini` (the "only
    once" markers), a `Launch <title>.lnk`, The Witcher's
    `GameuxInstallHelper.dll`, and an Inno uninstaller (`unins000.*`, never
    run by wineo). None is in the manifest, so they're the folder's extras
    under 0049's rules: kept by updates and left by an uninstall.
- **Plain Wine prefixes link the user folders** (Documents, Desktop,
  Downloads, Music, Pictures) to the real home, as Wine does by default;
  Proton's don't. A script's `{userdocs}` copy then lands in the real
  `~/Documents`. The maintainer keeps it that way: most games run on
  Proton, and other software's documents are easier to find in the real
  home than deep in a prefix.
- **Downloads went a file at a time per worker,** where the plan says
  chunks 4 at a time: The Witcher's 1.9 GB `textures00.bif` kept one core
  busy while the others waited at the round's end. Now up to 4 chunks are
  in flight across all files, each written at its offset in its file's
  temporary file; the worker writing a file's last chunk syncs it and
  checks its size and hashes by reading it back, and only then does the
  main thread take it. A round is at most 1 GiB or 1024 chunks, a file may
  span rounds, and the round's end waits for at most 4 chunks. Progress
  moves per chunk. A release build installs The Witcher's 14.4 GB in 1
  minute 41 seconds, against 16 minutes before (in a debug build).

Step 5 (2026-10-14):

- **The batch file is a constant,** pure ASCII and CRLF: `cd /d
  "%WINEO_SETUP_DIR%"`, then `"%WINEO_SETUP_PROGRAM%" %WINEO_SETUP_ARGS%`
  (or `"%SystemRoot%\system32\msiexec.exe" /i "%WINEO_SETUP_MSI%"
  %WINEO_SETUP_ARGS%`, spelled out since cmd looks in the current folder
  first), then the code file from `%WINEO_SETUP_CODE%`. The values go in
  that one run's environment only, as Z: paths. Its own path must be plain
  ASCII without cmd's special characters, or setup refuses with a message
  that says why.
- **What wineo adds to a line is quoted or checked.** GOG's own argument
  strings pass verbatim, as GOG wrote them. Paths are quoted, and a path
  with `"`, a line break, or a backslash in its Linux name is refused (a
  backslash would read as a separator). The language code and build id
  must be plain letters, digits, `-`, `_`, and `.`; the version name is
  quoted with its own quotes removed. A depot language that fails this is
  refused when setup starts, after the download: it only happens with a
  broken manifest, and nothing runs.
- **`cmd` is the prefix's,** never a file of that name in the current
  folder.
- **Which steps exist.** A product gets one only when the repository lists
  it. ISI's step is skipped when GOG's repository lacks ISI (warned at
  planning), with no fallback to the `temp_executable`, and a
  `temp_executable` that isn't among the product's support files is
  skipped with a warning.
- **Identity.** A `temp_executable` is identified by its full argument
  string, so a language, version, or folder change reruns it; ISI's follows
  the plan (build, script MD5 from the manifest, interpreter manifest), so
  a language change alone doesn't. Recorded steps count only with a
  `pending` matching the journal or an `installed` matching the committed
  selection; otherwise every step runs.
- **`/lang-code=`** is each product's own depot code, and `/Language=`
  and `/LANG=` its English name from a small table, English when unknown.
- **The graphical session** is checked at planning and before applying,
  even for a build with no steps. Missing support folders are created so
  `cd /d` can't land elsewhere.
- **Run files** are `gog-galaxy-setup-NN.bat`, `.code`, and `.log` in the
  run directory. `Installed` gains `needs_restart`, and progress a `Setup`
  stage; how they show is step 8's.
- **Live:** SimCity 4 installed under Wine into `SimCity™ 4 Délüxe & 100%`
  with ISI, and The Witcher under GE-Proton11-3 with its setup program,
  both MSVC 2005 runtimes, and DirectX, every step exiting 0 with its
  effects in the prefix.

Step 6 (2026-10-14):

- **Where an update starts.** `installed` when the sidecar's rules trust
  it; else the installed build's manifest fetched from GOG when that build
  is known and listed; else the target's (answer b, nothing deleted). The
  plan says which, and notes that setup's own prefix writes aren't counted
  for space.
- **Files the update doesn't change** are left alone when `installed` is
  trusted: present, missing, or resized (the maintainer, 2026-10-14;
  restoring them is `--repair`'s job). Only an update starting from a
  fetched manifest checks them by size (question 4).
- **An edited file at a changed path** is saved aside, never overwritten,
  so Galaxy's `overwritten` stays empty.
- **`pending` keeps the starting manifest,** so a resume after the new
  `goggame-<id>.info` is in place still knows what the old build shipped.
  It's evidence only: every delete or replace is still hash-checked at the
  aside. A retarget with `pending` lost starts from the target's manifest.
- **Deletions walk only the starting manifest,** so setup's extras in the
  folder are never touched, and a folder goes only once it's empty. Before
  a folder becomes a file or link, wineo's own entries in it are deleted
  on a hash match.
- **Chunk reuse** covers game files the starting manifest lists: each old
  chunk is read at its offset and checked against its MD5 first. Staged
  files count at full size for space.
- **Setup steps carry over** only from a trusted `installed`; otherwise
  every step runs. A new redistributable version alone, on the same build
  and products, doesn't make an update.
- **Languages** are picked as installs pick them, so a migrated game
  recorded as `en` matches `en-US` depots. A file kept on size alone with
  no whole-file MD5 is recorded without one, and a later delete checks it
  chunk by chunk.
- **A Galaxy update needs a login,** even to finish an interrupted one.
  `update NAME --reinstall` on a Galaxy game is refused until step 7. The
  CLI and app still refuse Galaxy updates until step 8; core's
  `galaxy::plan_update` dispatches by track.
- **Live:** Papers, Please installed at 1.2.76 under GE-Proton11-3 (the
  older build picked by a throwaway patch), then updated to 1.4.11 with the
  real code in 8 seconds: 108 old files deleted on hash matches, 53 added,
  4 changed, the old `assets` folder removed once empty, two edited files
  the new build drops kept and listed, an edited setup extra untouched,
  and ISI rerun for the new build.

Step 7 (2026-10-15), with two answers from the maintainer the same day:

- **A Galaxy `--reinstall`** (answer A) goes to the latest build like an
  update, installs newly owned DLC, and retries failed or unavailable DLC.
  Every file is hashed instead of trusting the record, and every setup
  step runs again, even when the build is already the latest. It starts
  from the stored manifest only when `installed` is trusted (so old files
  can be deleted on hash matches); otherwise it follows answer b and
  doesn't fetch the installed build's manifest the way an update does.
- **A repair never changes the build** (answer A). With a trusted
  `installed`, the stored manifest is both the target and the starting
  point, so a repair needs no build listing from GOG. Without one, it
  fetches the committed build's manifest and follows answer b; when GOG no
  longer lists that build, it says `GOG no longer lists build ID of TITLE.
  Run wineo update NAME to move to the latest.` An interrupted repair
  whose `pending` is lost refuses the same way instead of retargeting.
- **A damaged or edited file is saved aside** on a repair, as step 6
  settled for updates, never overwritten. wineo can't tell damage from an
  edit, so a damaged large file leaves a large `.wineo-saved` behind.
- **Hashing.** A repair or reinstall checks every game file, support file,
  folder, and link, with no size-only shortcut, and reuses the matching
  chunks of a damaged file, so same-size damage downloads only the bad
  chunk. `Progress::Verify` reports files and bytes checked, per file;
  plain updates don't emit it.
- **Setup.** The first attempt of a repair or reinstall clears the recorded
  steps, so every one runs; a resume keeps the steps already recorded.
- **Moving onto Galaxy** is a `Reinstall` on the latest generation 2 build
  that starts from the target (answer b). A Linux game says `Galaxy builds
  are only for Windows games.`, and one with only generation 1 builds gets
  step 4's wording. An installed DLC missing from the new build refuses the
  move, as it refuses an update.
- **Moving to the installer track** plans from an in-memory copy of the
  state with the track switched, builds cleared, and languages mapped to
  installer codes (`en-US` becomes `en`), so the installer side's own
  ownership check installs newly owned DLC and retries failed ones. After
  the commit, the sidecar and the support folder are removed; a crash
  first leaves them for the next installer-track run to remove.
- **Store cleanup** ignores a sidecar whose folder isn't on the Galaxy
  track (by its journal's target, else its committed track), so one left
  by a crash pins nothing. A state that can't be read still counts.
- **Journals win over options.** An interrupted `Reinstall` or `Repair`
  finishes as its own kind; the plan's dispatch follows the journal's
  target track, else the track a reinstall asks for.
- **For step 8:** `--repair` on an installer-tracked game uses the plan's
  "Only Galaxy games have branches." line for now, and `ProductGone`'s
  "which the install began with" reads oddly outside an install; both
  want repair- and move-specific copy. Galaxy plans have `kind()` but no
  wording of their own yet, so the CLI words them.
- **Known limit:** a damaged `goggame-<id>.info` makes `installed`
  untrusted, so its repair takes the answer-b route and needs the build to
  be listed.

Step 8 (2026-10-15), with the maintainer's answers the same day:

- **Checking.** A Galaxy game's latest build is the first generation 2
  build on the default branch. It's `available` and `applicable` when that
  differs from the installed build, including an unknown, unlisted,
  generation 1, or other-branch one, since answer b and the fetched start
  handle those. `update: galaxy` stays only when there's no generation 2
  build, with the note `GOG has no Galaxy build of this game that wineo
  can update. Move it to offline installers with: wineo update NAME
  --reinstall --track installer` (approved).
- **Version names** (approved, answer A): the `--check` table shows GOG's
  version names, falling back to the build id; JSON's `installed` and
  `available` stay build ids. Recovery reviews show the build id, since
  nothing fetches the builds list on resume.
- **DLC in `--check`.** Galaxy games now get the newly owned DLC check
  too. When there's DLC to ask about, the check fetches the latest build's
  manifest and runs the same selection an update does, so DLC the build
  lacks never shows as an update; a failure there is a warning.
- **Reviews, progress, and reports** follow the approved mockups: a
  `track` row (`installer -> Galaxy` on a move), versions with the build
  id when a move keeps the same name, `support` and `setup` rows, `check`
  and "only what's missing or damaged" on a repair, "up to SIZE, files
  that already match are kept" on a reinstall, and the plan's notes as
  lines after the rows. Progress says `Checking files`, `Downloading`, and
  `Running setup N of M: LABEL`; setup progress carries the step's name
  from core. A repair or reinstall reports "Saved these files aside, since
  they didn't match GOG's:", and a redistributable asking for a restart
  says Wine doesn't need one.
- **Messages** (approved): `--repair` on an installer game says `Only
  Galaxy games can be repaired. Move it with: wineo update NAME
  --reinstall --track galaxy`; a missing product outside an install says
  `Build VERSION of TITLE doesn't have LABEL, which is installed.`
- **Interrupted Galaxy installs** finish through `wineo import gog --game
  ID`, as core has required since step 4, and every hint names that
  command. Installer-track installs finish through `wineo update NAME`, so
  this differs; making `update NAME` finish a Galaxy install too would be
  a small core change. Left for the maintainer (decided while away).
- **No content system.** With no track asked for, a 404 from the build
  list means no Galaxy build, and the install goes to the installer. A
  check reads a 404 the same way (`update: galaxy`), and `update NAME`
  says the same note instead of a raw 404; other errors still fail.
- **`update --all` finishes journals whose check failed,** on both tracks:
  the journal is read from the folder, so a builds list that fails for a
  moment can't strand an update that could finish without it. A game with
  no journal whose check failed is still a failure.
- **Every failure that leaves a journal says how to finish** (a bound set
  after two review findings of the same class). When planning or applying
  an install, update, repair, reinstall, or move fails, isn't a cancel,
  and the folder's journal is still there, the error ends with the finish
  command once, on both tracks. The journal decides, not the error's type
  or the command that ran: a Galaxy install's journal names `wineo import
  gog --game ID` even after `wineo update NAME`, which says `NAME's GOG
  install didn't finish. Finish it with: wineo import gog --game ID`.
  Errors before planning that aren't about the game (usage, `--yes`,
  logged out) get no hint. Accepted limit (decided while the maintainer
  was away, after the fourth review round): when the environment's own
  config can't be read, its folder and journal can't be found, so the
  config error shows without the hint; once the config is fixed, the next
  run gives it.
- **Deferred:** the `branch` and `pinned` JSON fields land with branches
  in step 11. The app shows a clean error for a Galaxy update until step
  9, which also rewords `UpdateError::Galaxy` and the `OnGog` note for the
  app.

Step 9 (2026-10-15), with the maintainer's answers the same day:

- **Live test** (before step 9, on `zz-update-papers`, Papers, Please):
  `--repair` found a same-size damaged DLL that `--check` couldn't see,
  downloaded 488 KiB, and saved the damaged copy aside; a move to the
  installer track and back both finished, and the move back downloaded
  nothing, since every file the installer left matched the Galaxy build.
- **One wording for both front ends.** The Galaxy review rows, notes,
  changes, saved-aside heading, restart lines, and progress `Reporter`
  moved from the CLI into core (`gog::update::galaxy::present`), like the
  installer track's `ReviewLabel`. The CLI words labels in lowercase, the
  app in Title case; the CLI's output didn't change.
- **Track in the install review** (approved): a `Track` picker under
  `Platform`, `Galaxy` or `Offline installer`, shown when both are
  possible. Core's new `galaxy::track_for` makes the choice `plan` made
  inline, so the app learns which tracks exist before planning, and a
  failed Galaxy plan keeps the picker. When GOG can't say (the builds
  request fails on a fresh Windows install), the picker offers both with
  nothing chosen. A Galaxy install's review shows Version, DLC, Support,
  Setup, and Download, with no "Installs in" or estimated "Installed" row,
  since its language is a depot tag and its sizes are exact.
- **Galaxy updates in the app** follow the approved CLI copy. The page
  finishes any Update, Reinstall, or Repair journal, whichever front end
  left it. A retarget (question 12) shows as the plan's warning, and the
  Update button accepts it.
- **Finishing an interrupted Galaxy install** (answer A): the library
  card's Finish install opens the install page, which resumes it and runs
  the install's own finish (playtime, art, desktop entries, the program
  picker). Installer-track installs still finish on the update page. Core
  is unchanged, so the CLI keeps naming `wineo import gog --game ID`.
- **Finish hints** follow step 8's bound in the app on both tracks: any
  non-cancel failure, planning or applying, that leaves a journal ends
  with the page's finish hint once, chosen from the journal, and a cancel
  with a journal says the interrupted message. Logged out and an
  unreadable environment or global config get no hint (step 8's accepted
  limit; the install page loads the global config before it can look for
  a journal).
- **The Track picker's knowledge belongs to the platform it was learned
  for** (a bound set after two review rounds found it out of step with
  the page): it shows only for that platform, a Track choice counts only
  while it shows, and a Platform change learns it again.
- **The "no Galaxy build" note** stays as it is, CLI command included
  (maintainer, 2026-10-15). A "Move to offline installers" action in the
  app joins step 13's Version picker (amended in "Desktop app" and
  the implementation sequence).
- `UpdateError::Galaxy` can't be reached from either front end now; only a
  direct caller of core's installer-only `update::plan` gets it. Its
  "later release" wording is a step 10 core API docs item.

Step 10 (2026-10-15), with the maintainer's answers the same day:

- **Docs:** `gog.md` describes both tracks (installing, Galaxy updates,
  reinstalling and repairing, moving between tracks, interrupted
  operations, where support files and redistributables live, limits);
  `json-output.md` what `version`, `installed`, `available`, `unfinished`,
  and `update: galaxy` mean on the Galaxy track; `core-api.md` the Galaxy
  module (`track_for`, `plan`, `plan_update`, both plans, `Progress`,
  `GalaxyError`, the shared wording) and the changed `Status`, `Options`,
  `Updated`, and `FolderError`; `gui.md` the Track picker, Galaxy updates,
  and finishing; `stability.md` the new managed files; the README.
- **One finish command per journal, everywhere** (answer A): writing the
  docs showed an interrupted Galaxy install blocking a launch with "Finish
  it with wineo update NAME", which then pointed at `wineo import gog
  --game ID`. Launches and clones (`FolderError::MidInstall`), `rm`,
  unsetting `gog_id`, validate, and the hint after a migration reclaims a
  folder (CLI and app) now ask the journal through
  `Journal::galaxy_install()`, step 8's rule.
- **Stale wording** (answer A): the installer-only planners' Galaxy and
  repair refusals no longer say "a later release"; they name
  `galaxy::plan_update` and `galaxy::plan`, the only way they're reached
  being a direct call into core.
- **Version names in `import gog --list`** (answer A): the table shows
  GOG's name for a Galaxy build, else its id, like `update --check`; JSON
  keeps build ids.
- **The review's `download` row on Galaxy** is the build's uncompressed
  size, as approved for the CLI; the docs say what crosses the network is
  usually less.

Step 11 (2026-10-15), with the maintainer's answers the same day:

- **`--builds`** (answer A): `wineo update NAME --builds` lists GOG's
  generation 2 builds by branch, default first, each newest first, marking
  `installed`, `latest`, `pinned`, and `followed`, with `--json`. A game
  with private branches says so. Core's `galaxy::listing::list` is what
  step 13's Version picker will use.
- **Pinned in `--check`** (answer A): STATE `pinned` when a newer build is
  on the branch followed, with the note `Pinned to build ID (VERSION). Run
  wineo update NAME --latest to follow its branch again.`; pinned to the
  latest, STATE `up to date` with the same note.
- **Review rows** (answer A): `branch` when the game follows a non-default
  branch or the run changes it, `pin` when pinning or unpinning.
- **`--build`** (answer A) takes a build id, or a version name matching
  exactly one listed build; an ambiguous name lists the ids. A pin stored
  in the record resolves by id only.
- **A pin holds the build, not the DLC** (answer A): a pinned game installs
  newly owned and retried DLC its pinned build has, and stays pinned.
- **Calls shown to the maintainer, not objected to:** a change that moves
  no files (`--build` of the installed build, `--latest` on the latest) is
  a review and a record write, with no journal; an interrupted branch or
  pin change carries the intended follow in the record's `pending`, so
  the resume commits it; a retarget (question 12) of a pinned operation
  drops the pin and says so; `--build` naming a build on another branch
  follows that branch unless the build is listed on the one followed;
  `--reinstall` on a pinned game rebuilds the pinned build and keeps the
  pin; branch flags on a game with an unfinished operation are refused
  until it's finished; a pin survives another tool changing the installed
  build; a followed branch with no generation 2 build says how to return
  to the default branch.
- **Every writer or remover of the Galaxy record rechecks the follow** (a
  bound set after two review findings of the same class: a stale update,
  and a stale move off Galaxy, each overwriting a newer pin). An install,
  resume, update, reinstall, repair, follow-only change, or move off
  Galaxy compares the record's follow with the one it planned from, under
  the folder state lock before any write, and refuses with `Changed`.
  Adopting another tool's build rewrites only `installed` under the same
  lock; `forget` and validate's cleanup remove a whole state on purpose.
- **Accepted limit:** a game pinned to a build GOG no longer lists, on a
  branch with no generation 2 build left, shows `update on GOG` in
  `--check`, though a plain update would still work on the pinned build.
- **Also fixed:** `update --all --track galaxy` was accepted and ignored
  `--track`; it's a usage error now.
- **Live test** on `zz-update-papers`: `--branch "Legacy (64-bit)"` rolled
  back to 1.2.76 and moved the offline installer's leftovers aside;
  `--build 1.4.11.124` (a version name) returned to the default branch and
  pinned; `--latest` unpinned with no download.

Step 12 (2026-10-16), with the maintainer's answers the same day:

- **Probe** (approved, read-only): Disco Elysium's generation 2 build list
  answers 200 with the identical public list (5 builds, default branch
  only, `has_private_branches: true`) with no password and with a wrong
  one. GOG can't be asked whether a password was wrong; a branch that
  isn't there looks the same.
- **When wineo asks** (answer A): only when `--branch NAME` isn't in the
  public list and GOG says the game has private branches; otherwise step
  11's "GOG lists no branch" message. A refused or unknown branch says
  `GOG lists no branch NAME for TITLE, or refused its password.`, since
  the two can't be told apart.
- **Prompt** (answer A): `Branch NAME of TITLE is private. Password: `,
  hidden. `--password-stdin` reads one line, removing only the line
  ending, so spaces around a password stay. Without a terminal it names a
  command that works: `Give its password with --password-stdin.` when
  `--branch` was given, else the whole `wineo update NAME --branch BRANCH
  --password-stdin`.
- **`--builds` and checks** (answer A): saved private branches are listed,
  marked `private` (JSON `"private"` on every build). A game following a
  private branch uses its saved password everywhere; with it missing,
  `--check` says `can't compare` and how to enter it again, `update NAME`
  at a terminal asks, and `--all` reports that game as failed and never
  prompts. The app uses a saved password and otherwise fails naming the
  CLI command; its own password entry is step 13.
- **A branch password belongs to the account that saved it** (a bound set
  after two review rounds found cleanup missing when the keyring dropped
  out mid-operation). Keyring items are `service=wineo`,
  `account=gog-branch`, with attributes `user`, `game`, and `branch`;
  without a keyring, a 0600 `gog-branches.key` holds `{user: {game:
  {branch: password}}}`, like wineo's other key files. Only the current
  login's are read, listed, sent, or compared; another account's, or one
  with no owner, is never used and is removed by the next branch-password
  write. Logging out, switching accounts, and a refused renewal still wipe
  them all (answer d), as cleanup: a wipe the keyring can't finish
  can't leak a password to the next account.
- **A refusal removes only the password it sent:** under the credentials
  lock, while the same login is current, and in each backend only the
  copy equal to the refused password, so a newer one saved meanwhile
  stays. A password is saved only once GOG lists its branch with it, at
  planning, so declining the review keeps it.
- **Crash leftovers** of the fallback file are swept under the credentials
  lock whenever it's written or removed. Accepted limit, pre-existing:
  `gog.key`, `steam.key`, and `steamgriddb.key` sweep theirs on migration
  and removal but not when a crashed save is followed by another file
  save, since Steam and SteamGridDB saves share no lock to sweep under.
- **During an unfinished operation,** `--branch` naming the branch it
  already follows isn't a change: it's taken as the password, so the
  recovery hint's command works. Any other branch flag is still refused.
- **Accepted limits:** a followed public branch GOG removes, on a game
  with private branches, reads as a missing password; `--build ID` on a
  private branch whose saved password is now refused says GOG doesn't
  list the build, and the stale password stays until the next listing or
  update; a `--check` that finds a saved password refused removes it
  (dry runs don't).
- **Live check** on `zz-probe-witcher` (The Witcher has private branches):
  without a terminal it named `--password-stdin`; a wrong password gave
  the "or refused" message; nothing reached the keyring or a fallback
  file. A game without private branches never asked.

Step 13 (2026-10-16), with the maintainer's answers the same day:

- **Placement** (answer A): the Version picker is a view inside the
  panel, like Log. A **Version** row in the panel's details shows the
  installed build's name, with `(pinned)` or the followed branch, and a
  **Change** button; it and **Version...** in the Actions menu open the
  view. **Back** and Esc return to the details. The view has a Branch
  dropdown (saved private branches marked `(private)`), the branch's
  builds as quiet rows with their date and marks, **Enter a private
  branch** when GOG says the game has them, **Repair**, **Move to offline
  installers**, and **Review** as the one accent action.
- **What a row means** (answer A): the newest build on a branch follows
  that branch (`Choice::Branch`, `DefaultBranch`, or `Latest`); an older
  one pins it (`Choice::Build`). Pinning the newest build stays CLI-only.
- **Every action ends on the update page's review:** a branch or build
  choice, Repair (`repair`), and a track move (`reinstall` plus `track`).
  A change that moves no files (`Planned::Follow`) gets its own review
  with **Switch** in place of **Update** (answer 5), applied through
  `switch::apply`.
- **Move to Galaxy** (answer A): an installer-track Windows game's view
  offers it when `galaxy::track_for` says GOG has a Galaxy build. Native
  games get no Version row, since Galaxy has no Linux builds.
- **When the row shows** (answer A): for every GOG Windows game, with the
  track learned from a listing loaded when the view opens, not only from
  the startup check. Logged out and an unfinished operation open the view
  and say why; an unfinished operation keeps **Finish update**. In a
  portable folder the row doesn't show, like the panel's other GOG rows
  (maintainer, after the second review).
- **Private branches in the app** (answer A): **Enter a private branch**
  asks for the branch's name and its password (hidden), and lists it
  through `listing::list` with the entered password; a miss says `GOG
  lists no branch NAME for TITLE, or refused its password.` When an
  update needs the followed branch's password and none is saved, the
  update page asks `Branch NAME of TITLE is private.` with a Password
  field and **Continue**, and plans again, replacing step 12's error
  that named the CLI command. A refused password keeps the field up
  for another try.
- **Calls made during implementation** (approved by the maintainer the
  same day): the update page's main button names the action (**Update**,
  **Repair**, **Reinstall**, **Switch**, or **Finish**); finishing the
  update page checks that game with GOG again, so its Update row shows
  **Up to date** instead of disappearing; the app's Pin row names only
  the build, without the CLI's `until --latest`; and the Version row
  reads the installed build from the game's record when no check or
  listing has run, so it shows without a login. A version name counts
  only from a record that passes the sidecar rule; otherwise the row
  shows the build id.
- **A GOG status reply for a game counts only if nothing newer for that
  game has landed since it was asked**, whichever task sent it (the
  startup check, a recheck after an update, a listing). A bound set
  after two review rounds found the same class through two paths: each
  game has a generation, and a reply that lands after it moved on
  leaves the newer entry alone. A library rescan that finds a game's
  local GOG facts changed (a pin from the CLI, say) counts as newer
  too, and checks the game again.
- **Pin note in the app** (answer b, after the smoke test): the panel's
  note on a pinned game reads `Pinned to build ID (VERSION). Use Change
  to follow its branch again.`; the CLI keeps its `--latest` hint.
- **Track moves in the app's reviews:** a move to offline installers
  shows `Track  Galaxy -> Offline installer` after the folder, as the CLI
  does (found by the smoke test), and a move to Galaxy `Offline installer
  -> Galaxy`, in the Track picker's names.
- **Smoke test** (Xvfb, the real GOG login, a throwaway root holding a
  copy of `zz-update-papers`'s config and state): the Version row and
  Actions entry, branches in the dropdown, unpinning through **Switch**
  (the CLI agreed afterwards), and the reviews of a branch rollback,
  Repair, and Move to offline installers, which weren't applied.
- **Reviews:** seven qa-gpt rounds; rounds 2 to 5 each found one more
  path for a stale GOG reply, which the bound above now covers through
  one type the compiler enforces.

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
13. The app's **Version** picker, with **Move to offline installers** and
    **Move to Galaxy**.
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

## Acceptance checklist

Use a throwaway games folder so nothing lands in `~/Games`: `wineo set
--global defaults.games_root=~/wineo-0052-test` (unset it afterwards with
`wineo set --global --unset defaults.games_root`). Pick a small Windows
game you own that isn't installed (SMALL below), and use Papers, Please
(it has two legacy branches) and The Witcher (it has private branches)
where named. Log in first: `wineo import gog --login`.

1. `wineo update --check`: Disco Elysium shows GOG's version names, not
   `update on GOG`. `--check --json` has `"track": "galaxy"`, `"branch":
   null`, and `"pinned": null` for it.
2. `wineo import gog --game SMALL --dry-run`: the review shows `track
   Galaxy`, the version, the setup steps, any support files, and an exact
   download size, then `Dry run, nothing was installed.`
3. Install it for real and press Ctrl+C during the download: `wineo start
   NAME` refuses, naming `wineo import gog --game SMALL`. Run that: it
   picks up without downloading finished files again, runs setup, and the
   game starts.
4. Damage one of its files without changing its size (overwrite a few
   bytes of a `.dll` in the game folder). `wineo update NAME --check` still
   says up to date; `wineo update NAME --repair` finds it, downloads only
   that file, and saves the damaged copy aside as `.wineo-saved`. The game
   starts.
5. `wineo update NAME --reinstall --track installer`: the review shows
   `track  Galaxy -> installer`, the installer runs, and the game starts.
   Then `--reinstall --track galaxy`: the review says nothing is deleted,
   only files that don't match are downloaded, and the game starts.
6. `wineo update papers-please --builds` lists the default branch and
   both legacy branches with dates and marks. `--branch "Legacy (64-bit)"`
   rolls back to 1.2.76 and the game starts at that version. `--build
   1.4.11.124` returns to the default branch and pins it.
7. While pinned: `wineo update --check` shows the pin note, `wineo update
   papers-please` says it's pinned and exits 0, and `update --all` leaves
   it alone. `--latest` unpins with no download.
8. `wineo update the-witcher --builds` says GOG has private branches.
   `--branch beta` asks `Branch beta of The Witcher is private.
   Password:`; a wrong one says `GOG lists no branch beta for The Witcher,
   or refused its password.` and nothing is saved (`secret-tool search
   service wineo account gog-branch` finds nothing). If you have a real
   branch password, enter it: the branch's builds list, marked `private`,
   and `wineo import gog --logout` removes it.
9. In the app, Import from GOG: SMALL's review shows a **Track** picker
   with **Galaxy** and **Offline installer**. Installing on Galaxy shows
   its download and setup progress.
10. In the app, Papers, Please's panel shows a **Version** row. **Change**
    opens the picker with a **Branch** dropdown; picking `Legacy (64-bit)`
    and its build reviews the rollback, and **Update** applies it. Picking
    the default branch's newest build and pressing **Review** reviews the
    return. Esc closes the picker and keeps the panel.
11. Pin it from the CLI (`--build 1.4.11.124`): the panel says `(pinned)`
    and `Use Change to follow its branch again.` once the library refreshes.
    **Change**, pick the newest build, **Review**: the review says `Unpin`
    with **Switch**, and afterwards the pin is gone.
12. In the picker, **Repair** and **Move to offline installers** each open
    their review (the move shows `Track  Galaxy -> Offline installer`);
    press Back on both. On SMALL moved to the installer track (step 5), the
    picker offers **Move to Galaxy** instead.
13. In The Witcher's picker, **Enter a private branch** with a wrong
    password says the refused message and keeps the form.
14. Interrupt an update in the CLI with Ctrl+C, then open the game in the
    app: the panel shows **Finish update**, the picker says the update is
    unfinished, and **Finish update** finishes it.
15. Clean up: `wineo rm` the test environments, delete
    `~/wineo-0052-test`, and unset `defaults.games_root`.

## Scope limits

- Generation 1 builds (question 3), delta patches (question 6), Mac games,
  cloud saves, bonus content, and posting playtime aren't supported.
- Builds GOG no longer lists can't be installed.
- wineo never deletes a file whose content doesn't match what GOG shipped,
  and never touches files it didn't write.
- A crash at the wrong moment can make wineo forget a game's stored
  manifest, which costs a rehash or a download on the next update. It
  never costs a file (the bounding rule above).
- Another program writing a file in the install folder at the same moment
  wineo places it isn't guarded against beyond wineo's atomic operations
  (the race rule above).
