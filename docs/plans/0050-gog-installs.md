Status: approved

# 0050: GOG installs

## Goal

Install GOG games from wineo: native and Windows games from GOG's offline
installers, Windows games' updates, and DLC bought after installing. Second
of three GOG plans. It builds on [0049](0049-gog-games.md), which adds the
GOG client, the account, Heroic migration, native updates, and the rules
for game folders, their state files, and recovery. Galaxy builds come in 0051.

## Settled by the maintainer

This plan split from 0049 after that plan's third review (2026-10-10). Its
answers carry over from 0049's questions:

- **1A, 2A, 21A:** a GOG game is a normal Wine, Proton, or native
  environment with `gog_id`, and its installed state is in its folder's
  state file, with a write-ahead journal.
- **3, phased:** offline installers here; Galaxy builds in 0051.
- **7A:** new games install to `<games_root>/<title>`, with a new
  `defaults.games_root` setting, default `~/Games`.
- **8A, 18A:** native packages are extracted, never run, with split files
  assembled by wineo (0049's native packages).
- **20A:** a native game's products are always rebuilt together, base game
  first, so a fresh install with DLC is one rebuild.
- **9A:** uninstalled games appear in the import grid, not the library.
- **11:** owned DLC, a language setting (default English), and GOG art are
  in.
- **12A:** Galaxy-tracked games are left to 0051.
- **14A:** a game mid-install or mid-update (a journal) refuses to launch.
- **17A:** host root only.
- **19A:** `wineo update` also installs newly owned DLC, retries failed
  DLC, and rechecks unavailable DLC.

## Existing foundation

- **0049:** the `wineo-gog` crate (client and downloads), folder state
  files with their journals, locks, launch protection, native package extraction with
  split files and space checks, native updates with DLC precedence, and
  recovery.
- **Installer runs.** `wineo new --install-exe` and the desktop app's
  installer step build a prefix and supervise an installer until its whole
  process group exits (`installer.rs`). It passes no arguments today, and
  the CLI and the desktop app each have their own create-and-install code
  (merged into one in step 2a).

## Proposed design

### Install folders

- New games install to `<games_root>/<title>`, the title made safe for a
  folder name (path separators, control characters, and trailing dots and
  spaces removed). `--dir` overrides.
- A fresh install, Windows or native, into a folder that exists and isn't
  empty is refused, unless its state file has this environment's own
  `install` journal. 0049's no-overlap rule applies.

### Installing

- One core create-and-install function, used by the CLI and the desktop
  app, replacing today's two copies.
- **Packages first.** Every package the install needs, the game and each
  owned DLC, is resolved and downloaded before anything changes.
- **Space.** Before downloading, the review shows the download size and an
  installed-size estimate labeled as one. After downloading, native
  packages are checked with their real unpacked sizes like 0049's updates.
  Windows installers don't list unpacked sizes, so their check stays an
  estimate (twice the installer size), and the review says so. When the
  download cache and the game folder share a filesystem, the sum is
  checked.
- **Order:** download, then create the environment and the folder's state
  file with an `install` journal and no `[committed]` yet, install, then
  commit. 0049's journal, publishing order, durability, and recovery rules
  apply; a crash leaves the journal, and `wineo update NAME` finishes the
  install.
- **Native installs** name every entry they'll write in `attempted`, like
  0049's updates, so cleanup after a failure is exact.
- **Windows installs and updates** can't: an Inno installer runs its own
  scripts and programs, so its package doesn't say every file it will
  write. Their journal records the operation, the resolved target, and the
  packages, and `attempted` holds only the install folder itself.
  Recovery reruns the full installer of the journal's (or the current)
  version over the folder and checks the outcome (exit code 0 and the
  installed `goggame-<id>.info`'s version). wineo never deletes files
  after a failed Windows install or update, since it can't tell the
  installer's files from yours; `--reinstall` is the repair. The docs say
  so.

### Windows installers

- A Windows installer is a `setup_*.exe` plus `.bin` parts. The parts
  download into one folder with their original file names, since the
  `.exe` finds its `.bin` parts by name, and are verified before the
  installer runs.
- Build the prefix, then run the Inno installer with
  `/SP- /SILENT /SUPPRESSMSGBOXES /NORESTART /NOCANCEL /LOG="<log>"` and
  `/DIR="Z:\..."`, the install folder as a fully qualified Wine path.
- A Windows install needs a graphical session; without one it refuses
  before downloading.
- **Language.** Inno's `/LANG` picks one of the installer's own
  `[Languages]` entries, whose names vary by installer, and a GOG installer
  may or may not use that choice for the game's content. So wineo passes
  `/LANG=` from a table of common names and also runs the installer with
  the matching locale, which Inno falls back to. Afterward it reads the
  language from `goggame-<id>.info`. When that differs from the request,
  the review and the state file say so without guessing why: `Requested
  English; the installed game reports French.`
- **Exit codes:** 0 is success. 1, 3, and 7 mean setup never started
  changing files, so wineo offers the installer's own wizard once. 2 is a
  cancel before anything changed. 4, 5 (a cancel or an abort after a file
  error, both partway), 6, 8 (needs a restart), and any other nonzero code
  are failures partway: the journal stays for `wineo update` to finish, and
  there's no wizard.
- The default target comes from `goggame-<id>.info`'s primary file play
  task (path, working dir, arguments). A game with only a URL task, or no
  info file, gets no default target and the program list, like `new`.

### Native installs

- 0049's native package handling, into a fresh folder. The target is
  `start.sh`.

### DLC

- Each owned DLC installs after the game from its own installer, in the
  game's language, falling back to English, else marked `unavailable`
  with a note (for example, no Linux installer). Ownership comes from the
  account's owned products, intersected with the game's DLC list from
  product details.
- A DLC that was never available, or failed before changing files, leaves
  the game launchable. One that failed partway leaves the journal, so the
  game stays blocked until `wineo update` finishes it.
- Native DLC joins 0049's install order and precedence.
- Per question 19, `wineo update` also installs newly owned DLC, retries
  `failed` DLC, and rechecks `unavailable` DLC, each listed in its review.

### Windows updates

- An installer-tracked Windows game is compared by installer version, as
  0049 shows it. Applying an update runs the newer installer over the game
  with the same arguments, or GOG's patch installer when it starts from the
  installed version exactly, then reruns each installed DLC that also has
  a newer version.
- A failed patch is retried with the full installer, since a patch needs an
  exact starting version. 0049's recovery rules apply.
- `wineo update NAME --reinstall` reruns the current installers for a
  Windows game, which also repairs a replaced prefix's registry entries
  and redistributables. Historical installers aren't guaranteed through
  GOG's download API, so it uses the current ones. Galaxy-tracked games
  still refuse it until 0051.
- Inno answers some overwrite prompts with their defaults when message
  boxes are suppressed, so acceptance checks that an update actually
  changed the game's files and its `goggame-<id>.info`, not only that the
  installer exited 0.

### Commands

- `wineo import gog --game ID` now installs a game that isn't installed;
  0049's migration still applies when Heroic has it.
- Install options: `--platform linux|windows` (default: Linux when a native
  installer exists), `--language TAG` (a locale tag, default from the global
  `gog.language`, `en-US`), `--runner` and `--arch` (defaults from the
  global config, like `new`), `--dir`. When `--game` would migrate, these
  are refused, since the game is already installed: `Heroic has rimworld
  installed, so --game migrates it. Leave out --platform and --dir.`
- `import gog --list --json` adds `installed` to `state`.
- `wineo update --all` now covers installer-tracked Windows games and
  still lists Galaxy-tracked ones as skipped.

### Desktop app

- **Import from GOG** shows every owned game, with a review offering the
  platform, language, runner, and install folder, then download and
  install progress.
- Settings gains **Games folder** (`defaults.games_root`) and **GOG
  language** (`gog.language`).
- **Update** now works on installer-tracked Windows games.

## Decisions during implementation

Settled by the maintainer on 2026-10-12, before step 1:

- **`gog.language` and `--language` take locale tags** (`en-US`, `fr-FR`,
  `pt-BR`, `zh-Hans`), not GOG's codes, so they read like other software's
  settings. Default `en-US`. wineo maps a tag to GOG's installer code in one
  place: a small table where GOG differs from the language subtag (`zh` to
  `cn`, `ja` to `jp`, `cs` to `cz`, `pt-BR` to `br`, and so on), else the
  subtag itself (`de-AT` to `de`). State files keep recording GOG's code.
- **Which tags count** (2026-10-12, during step 1's review): RFC 5646's
  `langtag` form with a 2 or 3 letter language, so script, region, variants,
  and extensions all work, but grandfathered and private-use-only tags
  don't, and a reserved 5 to 8 letter language is refused so `english`
  stays a typo.

- **Step 2 runs in two parts** (2026-10-12): 2a moves `new --install-exe`
  and the desktop app's installer step onto one core create-and-install with
  a pluggable install step, changing nothing users see; 2b adds the GOG
  native install on top. Any place the two existing flows disagree visibly
  comes back as a question rather than being picked silently.

- **The installer flows behave the same** (2026-10-12, folded into step 2
  as 2a-ii). 2a found the three flows (`new --install-exe`, guided `new`, and
  the desktop app's installer step) differing in two places, and the
  maintainer chose one behavior for all three:
  - After the installer, every flow scans for programs and asks which to use,
    whatever the exit code (a nonzero one is reported too), once Wine has
    settled. A canceled installer keeps the environment and skips the
    picker, pointing to `wineo setup NAME`.
  - Every flow sets up the configured components (DXVK, vkd3d-proton) in the
    prefix before the installer runs, as `wineo start` does before a
    launch.

Provisional, chosen overnight on 2026-10-13 while the maintainer was away
(each can be overruled; see the board's "Waiting on you"):

- **Owned DLC ids** come from `embed.gog.com/user/data/games`, the account's
  full owned-ids list, since `getFilteredProducts` may list games only. To be
  checked against a real account in step 3's real-GOG test and acceptance.
- **Each product records its own installed language**, an optional field
  that defaults to the folder's, so a DLC that fell back to English still
  compares and updates in a game installed in another language. Matching
  for updates uses the product's own language.
- **An install writes the folder's state, with its `install` journal,
  before the environment's config.** A crash between the two leaves a
  journal that installing again (or `wineo update NAME`) resumes, rather
  than a config whose state is lost.

Step 2b's choices (2026-10-13):

- **Installs live beside create-and-install, not inside it.** A native GOG
  install has no runner or prefix, so it is its own core module that shares
  0049's download, space check, and rebuild with updates, rather than
  another kind of install step.
- **Resuming uses the journal first.** Installing again, or `wineo update
  NAME`, finishes an interrupted install from the journal's recorded target
  and cached packages, without asking GOG, even offline; only missing
  packages are fetched again, and no product is ever dropped on resume.
- **A pending install keeps its name**: installing the same name into
  another folder is refused until the first one is finished.
- **DLC follow the language the game actually installed in**, then
  English.
- **Known gap for step 6:** an install interrupted before its config was
  written can only be finished, not abandoned; `import gog` needs a way to
  drop it.

Carried from 0049's acceptance (2026-10-12):

- Sizes GOG lists are rounded to whole MiB, so they're estimates; a file's
  exact size comes from its checksum or the download (0.30.0).
- DLC are labeled `NAME (ID)`, and a review's confirm prints one keys line.

## Implementation sequence

1. `defaults.games_root` and `gog.language` settings, safe folder names,
   and the fresh-install folder rules.
2. Core create-and-install for native games and DLC, on 0049's packages,
   state files, and journals. Move the CLI and the desktop app's existing
   installer flow onto it.
3. Windows installs: installer arguments, language with read-back, exit
   codes, the wizard fallback, and targets from `goggame-<id>.info`.
4. Windows updates, patches, and `--reinstall`.
5. Newly owned, failed, and unavailable DLC in `update`.
6. CLI install options and JSON.
7. Desktop app: the full Import from GOG grid and install flow, and the
   new settings.
8. Docs: `gog.md`, config-files, json-output, and README.

## Testing and acceptance

Unit and CLI tests use 0049's fake GOG server and fixtures, plus a stub
Windows installer that reports each Inno exit code. They cover an install
interrupted at each step, a fresh install into a non-empty folder, a DLC
failing before and partway, a DLC unavailable for the platform, newly
bought DLC, a Windows update by patch and by full installer, and a failed
patch retried with the full installer.

The stub can't check real Inno path and language handling, so `--ignored`
tests install real small GOG games with `WINEO_TEST_GOG_LOGIN`: a native
one, a Windows one, and a Windows one with several languages, installed in
a non-English language and checked through its installed files.

Acceptance checklist, drafted after implementation, will include:
installing a small native game and a small Windows game from the CLI and
the desktop app; installing in another language; installing a game with
DLC; updating a Windows game and checking its files changed; and
interrupting an install and finishing it.

## Scope limits

- Galaxy builds, differential updates, beta branches, and rollback come in
  0051.
- Not planned: cloud saves, bonus content, posting playtime to GOG, Mac
  games, and GOG games in portable roots.
