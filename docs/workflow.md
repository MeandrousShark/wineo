# Workflow

How work flows in wineo, for AI agents starting a fresh session. The rules
themselves live in [`AGENTS.md`](../AGENTS.md); this page describes how they
fit together. If the two disagree, `AGENTS.md` wins.

## Plan lifecycle

- A heavyweight feature, where design decisions matter, gets a plan before any
  code. Patch fixes and hotfixes need no plan; the primary session can
  implement them directly. If a fix turns up a design question, ask the
  maintainer or write a plan.
- One file per plan: `docs/plans/NNNN-short-name.md`, numbered in order. The
  first line is `Status: draft`, `Status: approved`, or `Status: done`.
- The usual shape: goal, "Settled by the maintainer", existing foundation,
  proposed design, implementation sequence (numbered steps), testing and
  acceptance, scope limits. Decisions made after approval are appended to the
  plan ("Decisions during implementation", amendments), not left in chat.
- The primary drafts the plan with open questions. The maintainer settles them,
  the plan is amended to record the answers, and only the maintainer sets
  `Status: approved`.
- Work beyond an approved plan needs a new or amended plan.
- Once implementation lands, the plan gets an acceptance checklist (see
  below). `Status: done` is set only after the maintainer accepts.
- Ideas for later are parked as rows in [`features.md`](features.md), not built.
  Don't add them ad hoc.

## Who does what

- **Primary session:** coordinates, talks to the maintainer, drafts plans,
  reviews and integrates agent work, writes the docs, and does all release
  work (pushes, merges, version bumps, changelog, tags). Release work is never
  delegated.
- **Coding subagents:** implement one scoped step of a plan with clear
  acceptance criteria. Harder or more ambiguous steps go to a stronger tier.
- **QA agents:** review a non-trivial change independently before it ships.
  Findings that are real get fixed; pre-existing problems found along the way
  go to the maintainer to decide (patch fix, plan note, or backlog row).
- **Model families:** both Claude and GPT (Codex) agents fill the coder and QA
  roles. The maintainer picks the pairing per task or period, and prefers the
  implementer and reviewer to be on different families. Follow the pairing the
  maintainer or the session's instructions give; don't assume one.
- **Practical differences:** Claude coders may commit their own logical
  changes. GPT coders don't commit, so the primary reviews and commits their
  work in logical chunks. The Codex sandbox is offline and denies socket
  binds, so the primary adds new crates and runs `cargo fetch` first and
  reruns tests that need local HTTP.
- **Docs:** the primary writes them, since it already holds the context.
  Don't delegate docs to a docs-writer agent unless the maintainer asks.

## Landing a change

- Check before committing: `cargo fmt`, `cargo clippy --all-targets -- -D
  warnings`, `cargo test`. Use focused tests while iterating, then the full
  suite. See `AGENTS.md` for filters and the ignored real-program tests.
- Verify agent work yourself with targeted test runs. Don't trust a report.
- Commit every logical change with a short message and no attribution
  trailers. No unicode dashes anywhere.
- Update docs in the same stretch of work: the matching page under `docs/`,
  `README.md` links, and the `features.md` line for the plan.
- Push reviewed, green commits to `origin/main` as you go, even mid-plan with
  no release. It doubles as a backup. Never push a red or unreviewed state.
- Check `git status -sb` before assuming something is pushed.
- Some tests are load-sensitive flakes; rerun a failure alone before blaming
  the change. Snapshot and reflink tests need a btrfs checkout, so put test
  worktrees on the same filesystem as the repo, not tmpfs.

## Acceptance

- Every plan ends with an acceptance checklist of concrete steps to try in a
  real window, ideally with throwaway environments or a `WINEO_PORTABLE_DIR`
  folder.
- The maintainer runs it. Fixes and polish found during acceptance land and
  are recorded in the plan. Only then does the plan become `Status: done`.
- Don't mark a plan done, or release it, on your own say-so.

## Releasing

- Finished plan: minor bump (`0.20.x` to `0.21.0`). Fix between plans: patch
  bump. wineo stays on `0.x` until the maintainer calls it stable.
- Edit `[workspace.package] version` in `Cargo.toml`, build once so
  `Cargo.lock` picks up the new crate versions, and add a `CHANGELOG.md`
  section with the date and a short list of changes linking the plan.
- Commit as `Release X.Y.Z and mark plan NNNN done` (it also flips the plan's
  status), or `Release X.Y.Z` for a patch.
- Create an annotated tag `vX.Y.Z` with the message `Wineo X.Y.Z: short
  summary`. Check `git tag -n1` on the latest tag and match its shape.
- Push the tag with the commits.
- Call out behavior changes in the changelog, including ones in the CLI that
  rode along with a GUI plan.

## Working with the maintainer

- Design decisions are theirs. When something is ambiguous, ask instead of
  guessing, and record the answer in the plan.
- When asking them to pick between options for how something looks or feels
  to use (CLI syntax, config shape, layout), show the same scenarios in every
  option side by side first, with a recommendation.
- UI copy and errors are terse: one short sentence, like "Invalid input:
  spaces not allowed." wineo is for regular software as well as games, so
  generic copy stays neutral ("Add something").
- Ask before opening GUI windows for smoke tests; they may be mid-game or
  working over SSH with no display. Never run a bare full-screen `grim`.
  Capture only the app window, or use a headless or nested compositor. A
  plain `timeout` run checking for no panic needs no screenshot.
- Don't put the maintainer's personal details in requests, such as a
  `User-Agent` or API calls.

## Where things are documented

- `AGENTS.md`: authoritative agent rules. `CLAUDE.md`: Claude-only additions.
- `README.md`: install, usage, and an index of the docs below.
- `CHANGELOG.md`: one section per released version.
- `docs/plans/`: numbered plans and their decisions.
- `docs/features.md`: backlog, completed-plan links, and GUI entry criteria.
- `docs/gui.md`: the desktop app. `docs/core-api.md`: core API for frontends.
- `docs/config-files.md`: config schema and versioning rules, with
  `docs/environment.toml.example` and `docs/native-environment.toml.example`.
- Per-feature pages: `native.md`, `launch-options.md`,
  `downloads-and-imports.md`, `clone.md`, `trash.md`, `snapshots.md`,
  `portable.md`, `art.md`, `validate.md`, `stats.md`, `json-output.md`,
  `shell-completion.md`.
