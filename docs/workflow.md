# Workflow

How work flows in wineo. The primary session reads this at the start of each
session. The rules themselves live in [`AGENTS.md`](../AGENTS.md); this page
only adds what it doesn't say. If the two disagree, `AGENTS.md` wins.

## Plans

- The usual shape: goal, "Settled by the maintainer", existing foundation,
  proposed design, implementation sequence (numbered steps), testing and
  acceptance, scope limits. Decisions made after approval are appended to the
  plan ("Decisions during implementation", amendments), not left in chat.
- The primary drafts the plan with open questions. The maintainer settles them
  and the plan is amended to record the answers before approval.
- Once implementation lands, the plan gets an acceptance checklist (see
  below).
- Ideas for later are parked as rows in [`features.md`](features.md), not built.

## Who does what

- **Primary session:** coordinates, talks to the maintainer, drafts plans,
  reviews and integrates agent work, and writes the docs.
- **Coding subagents:** implement one scoped step of a plan with clear
  acceptance criteria. Harder or more ambiguous steps go to a stronger tier.
- **QA agents:** review a non-trivial change independently before it ships.
  Findings that are real get fixed; pre-existing problems found along the way
  go to the maintainer to decide (patch fix, plan note, or backlog row).
- **Model families:** both Claude and GPT (Codex) agents fill the coder and QA
  roles. The maintainer picks the pairing per task or period, and prefers the
  implementer and reviewer to be on different families. Follow the pairing the
  maintainer or the session's instructions give; don't assume one.
- **GPT coders** don't commit, so the primary reviews and commits their work in
  logical chunks. The Codex sandbox is offline and denies socket binds, so the
  primary adds new crates and runs `cargo fetch` first and reruns tests that
  need local HTTP.

## Landing a change

- Verify agent work yourself with targeted test runs. Don't trust a report.
- Update docs in the same stretch of work: the matching page under `docs/`,
  `README.md` links, and the `features.md` line for the plan.
- Push reviewed, green commits to `origin/main` as you go, even mid-plan with
  no release. It doubles as a backup. Never push a red or unreviewed state.
- Check `git status -sb` before assuming something is pushed.
- Some tests are load-sensitive flakes; rerun a failure alone before blaming
  the change. Snapshot and reflink tests need a reflink-capable checkout, so
  put test worktrees on the same filesystem as the repo, not tmpfs.

## Acceptance

- Every plan ends with an acceptance checklist of concrete steps to try in a
  real window, ideally with throwaway environments or a `WINEO_PORTABLE_DIR`
  folder.
- The maintainer runs it. Fixes and polish found during acceptance land and
  are recorded in the plan. Only then does the plan become `Status: done`.
- Don't mark a plan done, or release it, on your own say-so.

## Releasing

- After bumping the version, build once so `Cargo.lock` picks up the new crate
  versions.
- Commit as `Release X.Y.Z and mark plan NNNN done` (it also flips the plan's
  status), or `Release X.Y.Z` for a patch.
- Tag message: `Wineo X.Y.Z: short summary`. Check `git tag -n1` on the latest
  tag and match its shape.
- Call out behavior changes in the changelog, including ones in the CLI that
  rode along with a GUI plan.

## Working with the maintainer

- Record the answers to design questions in the plan.
- When asking them to pick between options for how something looks or feels
  to use (CLI syntax, config shape, layout), show the same scenarios in every
  option side by side first, with a recommendation.
- UI copy and errors are terse: one short sentence, like "Invalid input:
  spaces not allowed." wineo is for regular software as well as games, so
  generic copy stays neutral ("Add something").
- Terse isn't cryptic. Key hints are short verb phrases, like "Press Enter to
  create it, Esc to go back, Ctrl+C to cancel.", never bare fragments like
  "Enter create it".
- Ask before opening GUI windows for smoke tests; they may be mid-game or
  working over SSH with no display. Never run a bare full-screen `grim`.
  Capture only the app window, or use a headless or nested compositor. A
  plain `timeout` run checking for no panic needs no screenshot.
- Don't put the maintainer's personal details in requests, such as a
  `User-Agent` or API calls.

## Where things are documented

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
