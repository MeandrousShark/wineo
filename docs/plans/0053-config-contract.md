Status: approved

# 0053: Config contract

## Goal

Make the config half of the 1.0 promise real: 1.0 criterion 4 (a config
written by any 1.x release loads in every later 1.x, and an older one
loads or migrates cleanly, proven by upgrade tests on files written by
earlier releases) and criterion 5 (a renamed or removed key or flag keeps
working in 1.x with a warning, and `wineo validate` reports it). Both are
in [features.md](../features.md#10-criteria). [Stability](../stability.md)
already states the promise; this plan decides what happens at its edges and
adds the tests and machinery behind it.

It also settles what a newer file does in an older wineo, since every
config struct uses `deny_unknown_fields`.

## Settled by the maintainer

- **1.0 is a promise, not features** (2026-10-06). After it, anything that
  breaks the public contract waits for 2.0, through a deprecation path that
  `validate` reports. Human output, the GUI, and `wineo-core`'s Rust API
  aren't contract.
- **Stability's borderline cases** (2026-10-10): desktop entries and the
  snapshot and trash layouts are internal; keyring item attributes and the
  settings merge order are stable; the `gaming` template's settings and
  shell completion output aren't promised.
- **Boring 1.0** (2026-10-12): 1.0 adds no features, only the promise.
- **The JSON gaps** (criterion 3, 2026-10-12): closed in 0.30.1, so this
  plan covers criteria 4 and 5 only.

On 2026-10-16 the maintainer answered the questions below:

- **1: A2**, with the hint worded "A newer version of wineo may have
  written it", which reads better aloud than "a newer wineo". The
  unsupported-version hint starts the same way, "A newer version of wineo
  wrote it", since only a newer release writes a version this one doesn't
  know.
- **2: C.** Unknown global keys refuse; any other fallback to defaults
  warns.
- **3: the recommended rule.** 1.x may bump `schema_version` for a change
  in meaning, stamping only files that need it; later 1.x reads every older
  version, and 2.0 migrates.
- **4: A.** Lenient internal files, and cleanup that keeps everything when
  a record is unreadable.
- **5: A, with warning placement b.** Build the config-key half now and
  document the rest. Outside `validate`, a command prints a single summary
  line covering every config that uses a deprecated key.
- **6: A.** Fixtures written by real old releases.
- **7: the recommended review**, read-only and cross-family.
- **8: B.** Library state lives in a lenient `library.toml`.

## Existing foundation

- **Versioned configs** ([0018](0018-config-schema-versioning.md)):
  `config.toml`, `environments/<name>.toml`, `templates/<name>.toml`, and
  `art/<env>/art.toml` carry `schema_version = 1`. Every tagged release from
  v0.2.0 on writes it. `check_schema` (`config.rs:936`) accepts a missing
  key (legacy) or `1` and refuses anything else with
  `unsupported schema_version N in PATH`. Reads never rewrite a file; a real
  edit adds the key to a legacy file in the same write.
- **Additive keys never needed a bump.** `kind`, the launch options,
  `hooks`, `tags`, `winetricks`, external runners, and `gog_id` all landed
  as new keys. Because every config struct denies unknown fields, an older
  wineo refuses such a file (`unknown field kind`) instead of launching
  without the setting. This also catches typos like `gamemod = true`.
- **No key or flag has ever been renamed or removed**, so nothing is
  deprecated today. Shorthands like `[art] fetch = "all"`, `gamescope =
  false`, and `hooks = false` are documented forms and stay.
- **A broken global config usually means defaults.** `load_or_default`
  (`config.rs:754`) turns every error except an unsupported version into
  all defaults, with no warning, in 18 callers (`set`, desktop entries,
  winetricks, trash restore, Settings, the app's checkup, and more). A key
  from a newer wineo, or a typo, in `config.toml` therefore makes those
  commands act on the default prefix and runner roots without saying so.
- **Internal formats.** Of wineo's own files, only the GOG folder state and
  the Galaxy record deny unknown fields. Trash manifests, snapshot
  manifests, run state, playtime logs, and receipts are lenient, and
  fields added to them during 0.x used `serde(default)`. Six internal
  families carry a version: trash manifests and pending markers, snapshot
  manifests and restore markers, GOG folder states, and Galaxy records.
  A Galaxy record that doesn't parse, or has another version, reads as no
  record (`record.rs:412`), and redistributable cleanup treats it as
  naming nothing (`record.rs:586`).
- **No fixtures or deprecation code exist.** Tests check legacy and
  unsupported versions with inline strings (`config/tests.rs:573-700`,
  `cli.rs:169-232`), never files an old release wrote.
- clap 4.6 has hidden aliases but no deprecation warnings of its own.

## Questions for the maintainer

Answered on 2026-10-16 (see "Settled by the maintainer"). The design
below follows every answer.

### 1. A newer config in an older wineo

The promise is forward: newer wineo reads older files. The other direction
happens too: a portable folder opened on another machine, a downgrade
after a bad upgrade, or two installs sharing a config. The same three
scenarios in each option:

- **Typo:** `gamemod = true` in an environment.
- **Portable:** a folder written by 1.5, where an environment uses `foo`
  (added in 1.5), opened with 1.2.
- **Rollback:** upgrade to 1.5, set `foo` in the app, go back to 1.4.

**A. Refuse, as today.** Keep `deny_unknown_fields`, and document that a
key from a later 1.x makes an earlier one refuse that file.

Today's error, from `wineo status hades`:

```
error: invalid config ~/.config/wineo/environments/hades.toml: TOML parse error at line 3, column 1
  |
3 | gamemod = true
  | ^^^^^^^
unknown field `gamemod`, expected one of `schema_version`, `kind`, `prefix`, ...
```

```
Typo:      the error above, exit 1
Portable:  the same error for `foo` on that environment; others work
Rollback:  same as portable
```

**A2. Refuse, with a hint (recommended).** A, and the unknown-field error
says where a key could have come from. 1.0 is the oldest 1.x anyone will
run, so the hint has to ship in 1.0 to help later.

```
Typo:      the error above, then:
           A newer version of wineo may have written it; update wineo or remove the key.
Portable:  the same for `foo`; other environments work
Rollback:  same as portable
```

An unsupported `schema_version` (question 3) gets the same kind of hint:

```
error: unsupported schema_version 2 in ~/.config/wineo/environments/hades.toml
A newer version of wineo wrote it; update wineo to use this file.
```

**B. Ignore unknown keys, with a warning.** Older wineo loads the file and
skips what it doesn't know. `validate` reports `unknown_key`.

```
Typo:      warning: unknown key gamemod in hades.toml, ignored
           (the game launches without GameMode)
Portable:  launches without foo, with a warning on stderr
Rollback:  same as portable
```

Typos stop failing, and a skipped `hooks`, `gamescope`, or `runner`-like key
changes what a launch does.

**C. Stamp the version a file needs.** wineo knows the release each key
arrived in and writes `wineo = "1.5"` into a file when it uses a key newer
than 1.0. An older wineo reads that first.

```
Typo:      unknown field `gamemod` (no stamp, so it's a typo)
Portable:  hades.toml needs wineo 1.5 or newer (this is 1.2)
Rollback:  same as portable
```

Clearest messages, but the stamp is a new top-level key that has to land
before 1.0, every new key needs a `since` entry, and every writer keeps the
stamp current.

**Recommendation: A2.** It keeps typo detection and safe launches, and the
hint covers the downgrade case for a fraction of C's upkeep.

### 2. A broken global config

Today a key the running wineo doesn't know in `config.toml` makes most
commands quietly use all defaults (see foundation). Scenario: `config.toml`
has `prefix_root = "/games/prefixes"` and a key `foo` from 1.5, and 1.2
runs `wineo set hades gamemode=true` and then `wineo new tool`.

- **A. Refuse an unknown key like an unsupported version.**
  `load_or_default` returns the error for unknown fields as it does for
  `SchemaVersion`. Other breakage (bad syntax, a wrong type) still means
  defaults, silently.
  `set` and `new` refuse with question 1's message.
- **B. Keep defaults, but warn.** Every fallback prints one line:
  `warning: using default settings: unknown field foo in config.toml`.
  `new tool` creates its prefix under the default root after warning.
- **C. Both (recommended).** Unknown fields refuse (nothing acts on
  settings it couldn't read), and any other fallback to defaults warns.

`status` keeps warning instead of failing, as it does for an unsupported
version. This is a behavior change for the changelog.

### 3. Version bumps in 1.x

New keys never need a bump: question 1 covers older readers. A bump is for
a change in what an existing value means, which `deny_unknown_fields` can't
catch. One backlog row is that case today: the "Started" hook and launch
variables in [features.md](../features.md). `args = ["{prefix}"]` is a
literal string now and would expand afterwards, so a config with braces
would change behavior on upgrade, and an older wineo would launch with a
literal `{install_dir}` instead of refusing.

**Recommended rule:**

- A 1.x release may bump `schema_version` for a change in meaning, never
  for a new key or a rename (question 5 covers renames).
- Every later 1.x reads every older version by that version's rules, so the
  forward promise is unchanged.
- A file is stamped with the new version only when it uses something that
  needs it. Every other file keeps `1`, so a release with v2 still writes
  files an older 1.x can read.
- `validate` reports an older-version file only where the newer rules would
  read it differently (for variables, literal braces in `exe`, `args`, or
  `cwd`), and `--fix` upgrades it, escaping those braces. A file the newer
  rules read the same way needs nothing.
- 2.0 migrates an older version instead of refusing it, or tells you to run
  `wineo validate --fix`.
- In a merge, each value follows the version of the file it came from, so a
  v2 template feeding a v1 environment expands only the template's values.
  The merge order is stable, so this rule is too.
- An older 1.x refuses a newer version with question 1's hint. 1.0 has to
  ship that hint, since it's the oldest reader.
- The same rule covers art.toml.

Scenario: 1.6 adds launch variables as v2, `hades.toml` uses
`{install_dir}`, and `tool.toml` doesn't.

```
1.6 writes:  hades.toml with schema_version = 2; tool.toml stays 1
1.5 reads:   hades.toml refused with the hint; tool.toml works
1.7 reads:   both, each by its own version's rules
validate:    nothing for tool.toml; a v1 file with literal braces gets a
             warning, and --fix escapes them and stamps it 2
```

The details (escape syntax, which keys expand, the `validate` code) belong
to that feature's plan. This plan sets the rule and makes 1.0's refusal
useful.

Side effects to confirm:

- The built-in `gaming` template's text hardcodes `schema_version = 1`
  (`templates.rs:28`), which is fine: it uses nothing newer.
- An unsupported version keeps exiting 1, the general failure code, with
  no code of its own. A script can't do anything different with it than
  with any other refusal.

### 4. wineo's own files across versions

Internal files aren't promised, but criterion 9 (upgrading a real library
needs no manual fixes) depends on them, and a downgrade can lose data.
Scenario: 1.5 adds a field to the Galaxy record, then 1.4 runs `wineo
update` on another Galaxy game. 1.4 can't parse the newer record, reads it
as no record, and its redistributable cleanup removes a redistributable the
1.5 game still needs.

- **A. Lenient and fail-safe (recommended).** Drop `deny_unknown_fields`
  from the GOG folder state and Galaxy record, so new fields use
  `serde(default)` like every other internal file; keep bumping their
  `version` only for changes an older reader can't skip, which waits for
  2.0. Cleanup that reads every record (redistributables, and anything else
  found in step 2) keeps everything when any record is unreadable, instead
  of treating it as naming nothing.
- **B. Strict, never changed.** Keep the denies and add no field to these
  files during 1.x without a version bump. Cleanup stays as it is.
- **C. As today.** Accept downgrade risk for internal files.

A also closes the same hole for a corrupt record today.

### 5. The deprecation path

Nothing is deprecated yet, so the first real use comes in some 1.x
release. The parts:

- **Config keys:** a table of deprecated keys (file kind, old path, new
  path, release deprecated). Before deserializing, a pass over the parsed
  document moves the old key to the new one in memory and records it. The
  file isn't touched on read. Both keys set at once is an error naming
  both. `validate` reports a new stable code, `deprecated_key` (warning),
  with a fix that renames the key in place through `toml_edit`, keeping
  comments. A wineo edit to that file leaves the old key alone, since
  renaming it would make the file unreadable to a release from before the
  new name. Edits resolve the spelling through the same table: `wineo set`
  with either name updates the key the file already has (the old one, if
  that's what's there), and `unset` with either name removes it, so an edit
  never creates the both-keys error.
- **Where the warning shows** (answered b) for configs, scenario
  `wineo list` with 22 environments, 3 of them using a deprecated key:
  - a: one line per file, three lines like `warning: hades.toml: old_name is deprecated; use new_name`
  - b (recommended): one summary line: `warning: 3 configs use deprecated keys; see wineo validate`
  - c: nothing outside `validate` and the app's checkup
  
  Warnings go to stderr, so `--json` output is unaffected.
- **Flags and commands:** a hidden alias, plus a stderr warning when the
  old spelling is used: `warning: --old is deprecated; use --new`. The
  exit code is unchanged. A removed flag keeps its behavior until 2.0.
- **Variables:** wineo reads both spellings of a renamed `WINEO_*`
  variable and warns on the old one; hooks get both until 2.0.
- **JSON:** a renamed field is emitted under both names until 2.0, which
  [JSON output](../json-output.md)'s compatible-changes rule already
  allows.

What to build now:

- **A (recommended).** Build the config-key half now, tested with a
  test-only table entry: it's the hard part, and `deprecated_key` has to be
  in 1.0's list of stable `validate` codes. Document the flag, variable,
  and JSON procedure in `config-files.md` and `stability.md`, and build
  each the first time it's needed (a few lines of clap).
- **B.** Build every part now, each tested with test-only entries.
- **C.** Document the procedure only, and build everything with the first
  deprecation.

### 6. Upgrade fixtures

- **A. Files written by real old releases (recommended).** Build a set of
  tagged releases in a worktree, have each one write configs and state
  through its own commands into a portable root (so paths are relative and
  the files are machine-independent), and commit what they wrote. Proposed
  tags, each the release a config shape arrived in: v0.2.0 (first
  versioned), v0.7.0 (native), v0.14.0 (launch options, winetricks, tags),
  v0.24.0 (external runners), v0.28.0 (hooks, Steam), v0.31.0 (GOG
  installs), v0.34.0 (Galaxy builds). A generator script in `scripts/`
  makes them repeatable.
- **B. Hand-written fixtures** for each era, checked against git history.
  Cheaper, but a fixture only proves what its author thought old wineo
  wrote.
- **C. Start at the last 0.x.** Generate one set from the release before
  1.0, then one per 1.x release.

With A or B, every 1.x minor release adds its own set, as a new step under
"Releasing" in `docs/workflow.md`. A legacy, unversioned file has no
tagged release behind it, so it stays hand-written either way.

Old releases can't reach GOG offline, so GOG folder states and Galaxy
records come from v0.34.0's writers in tests rather than the generator.

### 7. A naming review before the freeze

1.0 freezes every config key, flag, command, and JSON field name until
2.0. **Recommended:** add a read-only review step, by a reviewer from the
other model family, that lists names that are inconsistent or unclear
(say, one command's `--dir` where another means the same thing with
another word, or mixed plurals). You pick
which to change. Renames before 1.0 could just break, but going through
question 5's path keeps your own configs working and gives the mechanism a
real first use.

### 8. Where library state lives

Library views (priority 1, before 1.0) and completion status (priority 2)
add state the app writes often: favorites, hidden, status, and notes per
environment, plus global sort, zoom, and saved filter presets. Their rows
assume new keys in the env configs. Under question 1 each such key makes
the whole file unreadable to an older wineo, and the global settings would
land in `config.toml`, where question 2 makes an unknown key refuse `set`
and `new` everywhere. The same three scenarios in each option:

- **Rollback:** in 1.5 you star hades and change the grid zoom (both new
  in 1.5), then go back to 1.4.
- **Hand edit:** you type `favourite = true` by hand.
- **Remove and restore:** `wineo rm hades`, then `wineo restore hades`.

**A. In the configs, as the rows assume.** Per-environment keys in
`environments/<name>.toml`, global ones in a `[gui]` table in
`config.toml`.

```
Rollback:  hades.toml refused (unknown field favorite); config.toml
           refused too, so set and new fail for every environment
Hand edit: refused as a typo
Restore:   comes back with the config, for free
```

**B. One lenient `library.toml` (recommended).** A new file in the config
dir, so it's backed up and travels with a portable folder. It holds
per-environment tables keyed by name plus the global view settings, and
carries `schema_version = 1` under question 3's rule. Unknown keys are
ignored with `serde(default)` and `validate` warns about them. Edits go
through `toml_edit` like every config, so 1.4 changing a star keeps 1.5's
keys.

```
Rollback:  hades launches; 1.4 shows no star and the default zoom,
           and 1.5 still has both
Hand edit: ignored; validate warns about an unknown key in library.toml
Restore:   core moves hades's entry into the trash entry and back
```

**C. Split by kind.** Status and notes, which are about the game and
hand-written, go in the env configs; view state (favorites, hidden, sort,
zoom, presets) goes in `library.toml`.

```
Rollback:  fine for the star and zoom; a status key added in 1.5 would
           still make hades.toml unreadable to 1.4
Hand edit: refused in the env config, ignored in library.toml
Restore:   notes come back with the config; core handles the star
```

**Recommendation: B.** The rule is that env configs say how to run
something and `library.toml` says how you see it. Strictness pays off for
keys where skipping one changes a launch, and only costs for keys where it
hides a star. The price is keeping entries in step through `rm`, trash
restore, and `clone`, all in core. Tags stay in the env configs, since
they shipped in 0.14.

This plan only records the rule in `stability.md` and `config-files.md`;
the library views plan builds the file. One constraint carries over: an
environment's `library.toml` entry rides in its trash entry as an additive
manifest field, never a new payload kind. Payload kinds are a strict enum
(`trash.rs:132`), and an older wineo that can't read one manifest refuses
to load every environment (see scope limits).

## Proposed design

Follows the answers to every question.

### Unknown keys (questions 1 and 2)

Configs deserialize on two paths. `parse_toml` (`config.rs:963`) handles
the global config, environments, and art.toml. Templates call
`check_schema` and then deserialize separately with `table.try_into()`
(`templates.rs:250`). An unknown-field error from either path gains the
hint line, and so does `check_schema`'s unsupported-version error.
`load_or_default` returns unknown-field errors like
`SchemaVersion`, and its other fallbacks return a warning for the caller to
print. Each of the 18 callers is checked: CLI commands print the warning on
stderr, the app shows it where it already shows checkup problems, and
`status` keeps going. Portable copying's `destination_global`
(`portable.rs:792`) discards every error with `unwrap_or_default()` today,
so it passes the refusal on instead.

### Internal formats (question 4)

Remove `deny_unknown_fields` from `gog/folder.rs` and
`gog/update/galaxy/record.rs`, and add `serde(default)` where a field is
optional. Redistributable cleanup (`record.rs:586-635`) stops when any
record fails to read. Step 2 lists every other cleanup or scan that reads
all of a kind of file, so each one is either fail-safe or recorded as a
limit.

### Deprecated keys (question 5)

A `deprecated` module in core holds the table and the in-memory pass,
which runs on the parsed table right before deserializing, on both paths
above. Its result rides with the loaded config, so `validate` can report it
and the CLI can print the summary once per command. The `validate` fix
shares the rename with the pass, applied to a `toml_edit` document.
Settings edits (`settings.rs:571`) write the key exactly as given today, so
they resolve a deprecated key's spelling through the same table before
writing or removing it.

### Fixtures and tests (question 6)

`crates/wineo-core/tests/fixtures/<version>/` holds a portable root per
release: `config/` with a global config, environments of every kind it
supports, and templates, plus art.toml, a trash entry, a snapshot, and a
playtime log where that release had them. An upgrade test per set copies it
to a temporary portable root and checks, with the current wineo:

- every config loads, and `validate --json` reports no errors
- reading leaves every file's bytes and modification time unchanged
- a real edit (`wineo set`) succeeds and stamps a legacy file
- playtime totals, trash entries, and snapshots read back as written
- `status --json` and `list --json` show what the files say

Trash manifests record the entry directory's device and inode
(`trash.rs:108`, `trash.rs:174`), and loading checks them
(`trash/manifest.rs:59`), so a committed entry never matches after a
checkout or copy. Fixture setup rewrites only that identity to the copy's
through a test-only helper, leaving every other byte as the old release
wrote it; the production check stays as strict as it is.

## Implementation sequence

1. Unknown keys: the hints for unknown keys and unsupported versions on
   both parse paths, `load_or_default`'s split, and every caller's warning,
   including portable copying (questions 1, 2, and 3).
2. Internal formats: the denies, fail-safe cleanup, and the inventory of
   scans that read every file of a kind (question 4).
3. Deprecated keys: table, pass on both parse paths, spelling resolution
   in settings edits, summary warning, `deprecated_key` and its fix, with a
   test-only entry (question 5).
4. Fixture generator and the committed sets (question 6). The primary
   builds the old tags, since that needs the network.
5. Upgrade tests over the sets.
6. Naming review (question 7): a list for the maintainer. Any renames
   picked go through step 3's path.
7. Docs: `stability.md` (questions 1, 3, 4, and 8, and the deprecation
   period), `config-files.md` (art.toml in the versioned list, a
   deprecated-keys section, the downgrade note, and where library state
   goes), `validate.md` and
   `json-output.md` (`deprecated_key`), `workflow.md` (fixtures at each
   release), and the features rows and criteria 4 and 5.
8. Acceptance checklist.

Steps 1 to 3 touch different files and can run in parallel worktrees.

## Testing and acceptance

- Unit tests for the pass: old key, new key, both, nested tables, a
  deprecated key inside a target table, a template, and a file that isn't
  rewritten.
- Edit tests on a file with the old spelling: `set` and `unset` with each
  spelling, an unrelated edit that leaves the old key alone, and a no-op
  edit that leaves the file untouched.
- `validate --fix` renames a deprecated key, keeping comments and order.
- Unknown keys in a template get the hint, like every other config.
- CLI tests: the summary line on stderr with `--json` stdout unchanged;
  `set` and `new` refusing an unknown global key; a portable copy refusing
  one; each fallback warning.
- A downgrade test: a Galaxy record with an extra field and one with a
  newer version both keep their redistributables, and so do a corrupt
  record and one that parses but fails validation.
- The fixture tests above, in the normal `cargo test` run.

## Scope limits

- **Downgrades are not promised.** An older wineo refuses a newer key
  (question 1); the promise stays forward-only.
- **Internal files stay unpromised.** They get forward-read tests because
  upgrades depend on them, not a format guarantee.
- **One unreadable trash manifest blocks every environment load**
  (`trash.rs:416-436`), on purpose: wineo can't tell which environment an
  unreadable interrupted removal belongs to. Under question 4's rule a newer
  wineo never writes a manifest an older one can't read, so only damage
  causes it.
- **GOG branch passwords** in `gog-branches.key` that don't parse are
  dropped on the next edit, as designed: they cost only entering them
  again.

## Found along the way

For the maintainer to triage; not steps of this plan.

- An environment named `imports` has the state directory Heroic import
  logs use (`paths.rs:361-367`), so `wineo rm imports` deletes every
  environment's import log. Moving the logs to a dot directory, like
  `.locks` and `.icons`, fixes it, since environment names can't start
  with a dot. Fixed as a patch fix (439a8d08); validate still skips the old
  `imports` dir so logs written before the fix aren't offered for removal.
