Status: done

# 0035: Heroic import in core

## Goal

Move the decisions in `wineo import heroic` out of the CLI and into
`wineo-core`, so a GUI can list a Heroic library, show what each game would
become, and import one game or all of them without copying CLI code. This
finishes GUI entry criterion 8, which [plan 0034](0034-core-api-review.md)
split in two.

Nothing changes for the user. Every prompt, summary, table, warning, log
file, and exit code stays the same, and the existing tests are the contract.

## Existing foundation

- Core already has `heroic.rs` (reading Heroic's config into `HeroicImport`
  and `HeroicNativeImport`) and `import.rs` (`inspect`, `execute_reviewed`,
  `register_native`).
- `crates/wineo-cli/src/commands/import_heroic.rs` is 5,708 lines: about
  3,600 of code and 2,040 of tests. `import_heroic/native.rs` adds about 550
  lines of code and 80 of tests.
- Plan 0034 gave core the conventions this plan follows: a `cancel::Token`,
  an `FnMut` event callback per operation, typed errors, no printing. See
  [Core API for frontends](../core-api.md).

## Review findings

From the 0034 review of `import_heroic.rs`:

1. **The runner pipeline is written out three times.** The direct, guided,
   and `--all` flows each call the same five functions in sequence
   (`clear_runner_dependency`, `drop_runner_conflicts`,
   `suppress_covered_settings`, `carry_over_components`,
   `carry_over_wayland`). `ReviewEvidence::resolved` then restates the same
   rules a second way, because the pipeline deletes issues as it goes.
2. **There are three issue policies.** `reject_unresolved` (direct) blocks
   unsupported settings and dropped conflicts. `review_settings` (guided)
   mixes its policy with a launcher-arguments prompt.
   `bulk_review_issues_inner` (`--all`) returns reasons as strings.
3. **Rendering holds classification rules.** `show_game_summary` decides
   which issues are warnings or errors and what's hidden, and the redaction
   and link grouping rules live in rendering too.
4. **The apply sequence exists six times.** Check the source is unchanged,
   build the settings, register, import playtime, fetch art, reconcile
   desktop entries: three copies in `import_heroic.rs` and three in
   `native.rs`.
5. **Decision code names CLI flags,** like "needs --runner" and "see -v".
6. **Planning writes config.** `backfill_match` records `heroic_id` on
   already-imported environments while classifying the library, before the
   user confirms anything.
7. **Some tests exercise a function production doesn't use.**
   `bulk_warning` is test-only, and production uses `bulk_evidence_warning`,
   which behaves differently. The same test-only name exists in `native.rs`.

## Proposed core API

A new `wineo-core/src/heroic_import.rs`:

- **Listing:** `sources(paths, explicit) -> Vec<HeroicSource>` and
  `listing(paths, global, &source) -> HeroicListing`, with each candidate's
  blocker (if any) as a typed reason.
- **One game:** `plan_game(paths, global, &source, id, runner, options) ->
  GamePlan`. It runs the runner pipeline once. The plan keeps the original
  `HeroicImport` untouched and records each issue as a `Finding { issue,
  resolution }`, where the resolution is `Carried`, `Dropped`, `Omitted`,
  `Kept`, `Blocking`, or `NeedsInput(LauncherArgs)`. That replaces
  `ReviewEvidence`.
- **Checking a plan:** `GamePlan::warnings() -> Vec<Warning>` (typed:
  shared prefix, umu disabled, EAC or BattlEye, omitted and dropped counts,
  Wayland unsupported) and `GamePlan::missing_build() -> Option<MissingBuild>`.
- **Editing a plan:** validated setters for the name (moving a default
  destination with it), exe, working directory, arguments, environment,
  launcher arguments, and copy or adopt.
- **Runner choice:** `suggest_runner(...) -> RunnerSuggestion` (use this one,
  offer to copy Heroic's, or choose among Proton builds).
- **The whole library:** `plan_library(paths, global, &source, options) ->
  LibraryPlan`, with each game `Ready`, `Pending` on a missing build, or
  `Skipped` with a typed reason, plus the games already imported.
- **Applying:** `apply(paths, global, &source, &plan, &token, progress) ->
  Registered`, then `Registered::finish(...)` for desktop entries. It's two
  calls because art has to be fetched between them, and art picking is
  interactive in the CLI. `apply_native` works the same way.
- **The import log:** built from the plan in core, since it's a file and not
  terminal output.

The CLI keeps clap arguments, every prompt and menu, the final review
screen, rendering blocks and tables, turning typed warnings and reasons into
today's text, SteamGridDB art picking, and exit codes.

The three flows map onto it like this:

- **Direct (flags only):** `plan_game`, print the summary, check it with the
  strict policy, `apply`, art, `finish`.
- **Guided:** `suggest_runner` drives the runner menu, then `plan_game`, a
  download offer per `missing_build`, a prompt for `NeedsInput`, the final
  review through the setters, then `apply`.
- **`--all`:** `plan_library`, a download offer per missing build and a
  recheck, the table and confirmation, then `apply` and `finish` per game.

## Open questions

The maintainer accepted every recommendation (2026-09-28).

### 1. When `heroic_id` gets backfilled

Today, classifying the library writes `heroic_id` into matching environments
that were imported before the key existed, even if the user then cancels.

**Recommended:** keep that timing, but make it an explicit
`LibraryPlan::record_backfills` call that the CLI makes where it happens
today. The write is harmless metadata that only helps matching later, and a
GUI can decide for itself when to call it.

The alternative is writing it only when the user confirms an import. That
changes behavior, and a user who cancels would see the same games as
"not imported" again next time.

### 2. One issue policy with two modes

**Recommended:** `GamePlan::resolve(Policy::Strict | Policy::Defaults)`.
Strict is the direct flow's rules. Defaults is guided and `--all`: a
`NeedsInput` finding becomes a prompt in the guided flow and a skip with a
reason in `--all`, which is what each does today.

### 3. The test-only warning functions

**Recommended:** delete `bulk_warning` in both files and rewrite its tests
against the production path while moving them. Today those tests pass on
code that never runs.

### 4. Typed reasons instead of flag names

**Recommended:** core returns reasons like `SkipReason::NeedsRunner`, and the
CLI keeps the text "Proton runner, needs --runner". A GUI shows its own
wording.

## Implementation sequence

1. Plan types and the single runner pipeline, with findings and warnings.
   The pure decision tests move to core here.
2. Apply in two phases, the import log, and playtime import, for one game.
   The direct and guided flows switch to it.
3. Library planning, backfills, and pending builds. `--all` switches to it.
4. Native games: the same shape in `apply_native` and native planning.
5. Docs: update [Core API for frontends](../core-api.md).

Each step keeps every existing CLI test green, and the snapshot tests for
the bulk table catch any text drift.

## Verification and acceptance

- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`
  pass, with the CLI tests unchanged apart from moved logic tests.
- About 830 lines of tests move to core (fixtures, pipeline, matching,
  filtering, bulk review, and log tests). Prompt, summary, and table tests
  stay in the CLI.
- Maintainer acceptance: against the real Heroic config, import one game
  with flags and one guided, and run `--all` up to its confirmation and
  answer no. Compare the output and the written configs with the release
  before this plan.

## Scope limits

- No new import features, sources, or flags.
- No change to the Heroic config parsing in `heroic.rs`.
- No change to what gets written, apart from what decision 1 settles.
