# Simplification finds

Places where wineo does avoidable work or repeats itself, found in a code
review and looked over by the maintainer (2026-10-27). Backlog row in
[features.md](../features.md): Perf, priority 3. The row keeps its priority
and status; this page keeps the detail. Line numbers are as of 0.38.1.

None of these changes behavior, so each should land with tests showing
the output is the same, and the reading ones with a check that the file
count or parse count actually drops.

1. **Read GOG folder states once per listing.**
   `crates/wineo-cli/src/commands/import_gog/list.rs:87` calls
   `folder::find` for each installed game, which rereads and parses every
   folder state, while update checking already scans them. Reuse one
   command-scoped snapshot, including games whose update checks failed, and
   index statuses by game ID instead of searching them linearly for every
   row. This looks like the biggest avoidable batch workload.
2. **Remove one TOML parser pass.** Normal config loading parses the text
   three times: schema probing, deprecated-key inspection, and typed
   deserialization. `parse_config` (`crates/wineo-core/src/config.rs:1055`)
   could validate the schema from the table it already parses. Keep the
   final typed parse from the original text so errors keep their line and
   column.
3. **Use the batch trash scanner more consistently.** `environment::tags`
   (`crates/wineo-core/src/environment.rs:618`) loads each environment
   separately, rebuilding the trash scan each time. One `PendingScan` plus
   `load_with_pending` reuses machinery already used elsewhere: a small
   change with less filesystem work during shell completion.
4. **Pass the loaded config into stats summarization.**
   `crates/wineo-core/src/report.rs:364` already has an `Environment` but
   calls `stats::summary`, which rereads and reparses its config. A helper
   that takes the loaded config removes the duplicate read and keeps the
   report internally consistent.
5. **Trim a few clearly unnecessary allocations.**
   - `crates/wineo-core/src/programs.rs:238`: use `sort_by_cached_key`, so
     lowercased names and paths are computed once per program instead of
     on every comparison.
   - `crates/wineo-core/src/launch.rs:895`: native executable checking
     clones whole environment maps just to get `PATH`. Look up the
     environment's `PATH`, then the global one, then the inherited value.
   - `crates/wineo-gog/src/chunks.rs:75`: URL construction clones every
     signed-link parameter to replace only `path`. Let template filling
     take a temporary path override and borrow the other parameters.
6. **Share the TOML "false or table" editing mechanics.** `ensure_hooks`
   (`crates/wineo-core/src/edit.rs:363`) and `ensure_gamescope`
   (`crates/wineo-core/src/edit.rs:442`) repeat the same conversion and
   comment-preservation logic. A small parameterized helper removes the
   duplication while each keeps its own field validation. Empty tables and
   comments must keep their current behavior.
7. **Share Heroic environment-variable parsing.**
   `crates/wineo-core/src/heroic_import/native.rs:143` and
   `crates/wineo-core/src/heroic_import/plan.rs:620` repeat the same
   `KEY=VALUE` parser and managed-variable checks. Extract the parser,
   returning a map, and leave each plan's issue resolution local, so the
   validation rules live in one place.
8. **Consolidate the GUI API-key settings state.** The SteamGridDB
   settings (`crates/wineo-gui/src/settings/steamgriddb.rs:40`) and Steam
   settings (`crates/wineo-gui/src/settings/steam_key.rs:21`) duplicate the
   settings, status, saved, and removed structures, plus a lot of row
   wiring. A shared key-settings model simplifies maintenance, while each
   provider keeps its own checking and storage.
