Status: approved

# 0058: 1.0

## Goal

Release wineo 1.0.0: make the [stability promise](../stability.md) official
and publish wineo on GitHub, with the maintainer still working in Gitea.
This is queue item 1 in [features.md](../features.md), plus the "GitHub
mirror and CI" row.

1.0 adds no features. Most of this plan is operations rather than code: a
history rewrite, a private repo for the maintainer's docs, a GitHub mirror,
CI, and the release itself. Steps that need the maintainer present (web UI
work, tokens, force-pushes) are marked **maintainer-present**.

## Settled by the maintainer

- **Boring 1.0** (2026-10-12). 1.0 is the promise, not a feature count.
  The release notes can headline everything since 0.30.
- **Gitea stays primary and push-mirrors to GitHub** (2026-10-14),
  tags included, syncing on every push. Changes still land by merging into
  main directly. An outside PR is merged locally and pushed to Gitea, never
  with GitHub's merge button, since the mirror force-pushes GitHub to match
  Gitea. Don't reopen GitHub-primary. Details are in the
  [sketch](../sketches/github-mirror.md).
- **CI on GitHub Actions for outside PRs and tags only:** fmt, clippy, and
  the normal tests, with no secrets for forks. `--ignored` tests stay local.
  A tag also builds release binaries for a GitHub Release.
- **Full history goes public** (2026-10-10). Commit emails stay as they are
  and there's no rewrite for them. Old plans staying in public history is
  fine.
- **Scrub `crates/target` from history at 1.0** (2026-10-22), as part of
  the public push, with the maintainer present for the force-push.
- **The maintainer's docs go private** (2026-10-10): `docs/plans/`,
  `docs/workflow.md`, and `docs/design/` move to a private repo nested in
  the working tree at the same paths, ignored by the public repo, and not a
  submodule. Public links to plans become plain "plan NNNN" text.
- **`AGENTS.md` and `CLAUDE.md` stay public,** with a note at the top of
  `AGENTS.md` that `workflow.md` and the plans are the maintainer's private
  files, and contributors handle their own workflow. Code boundaries,
  checks, and style rules still apply to everyone.
- **`features.md` stays public,** cleaned up so it doesn't point at the
  private docs.
- **The published AUR PKGBUILD clones the GitHub mirror over HTTPS,**
  since the Gitea SSH source only works for the maintainer. Publishing it
  waits for the packages plan (answer 9). The repo's own `PKGBUILD` moves
  to that source too (answer 10).

Answers to the plan's questions (2026-10-28):

1. **Criterion 9 is met.** The maintainer has used wineo as their only game
   launcher since 0.30.
2. **`library.toml` and `themes/` aren't promised.** They go under "Not
   promised" in `stability.md` as files the desktop app owns, read
   leniently.
3. **Plan files get their hashes rewritten** from filter-repo's commit
   map, so they keep pointing at real commits.
4. **The private repo is `.private.git` at the repo root,** with the repo
   root as its work tree, so the three paths stay where they are.
5. **The private repo keeps the plans' history,** extracted with
   filter-repo on a copy.
6. **The GitHub repo is `MeandrousShark/wineo`.**
7. **A tag builds release binaries:** an x86_64 tarball built on Ubuntu
   22.04.
8. **The sketches stay public,** with their plan links turned into text.
9. **No AUR package at 1.0.** It comes with the packages plan, alongside
   deb and rpm.

Answers after the plan review (2026-10-28):

10. **The repo's `PKGBUILD` switches to the GitHub HTTPS source,** so the
    README's makepkg steps work for anyone. Gitea mirrors on every push, so
    the maintainer's builds see the same commits.
11. **A green tag publishes its release automatically.** Publishing waits
    on the checks in the same workflow, so a red tag never gets a Release
    or a tarball.
12. **The GitHub repo starts private.** It stays private through the CI
    shakedown and goes public in step 7, after `gitleaks` and the
    `crates/target` check pass on GitHub's own copy. Until then, deleting
    it is a complete rollback.

## Existing foundation

- **1.0 criteria 1 to 8 are done or closed** (features.md). The contract
  is written down in `stability.md`, exit codes in `exit-codes.md`, JSON
  schemas in `json-output.md`, upgrade fixtures and tests from 0053, and the
  deprecation path from 0053. Criterion 9, a stretch of daily use, is met
  (answer 1).
- **`gitleaks git` over the full history found nothing** on 2026-10-10.
  About 1,400 commits have landed since.
- **`crates/target` in history.** It was tracked from `ab93c8f2`
  (2026-10-15) to `efcf688a` (2026-10-22): 2,854 files, about 2.4 GB
  uncompressed, against about 240 MB for everything else in history. The
  biggest other blobs are the two bundled Noto Sans fonts at about 620 KB
  each. Rewriting from `ab93c8f2` changes every commit from there on (516
  as of 0.39.1) and every tag from v0.33.0 on (11 as of 0.39.1). Commits
  before it keep their hashes.
- **Commit hashes in docs.** `CHANGELOG.md` has none. About 23 plan files
  cite hashes, and some of them are in the rewritten range. The one hash in
  the upgrade fixtures (`b73dcfc8`, in `fixtures/legacy/README.md`) is
  older than `ab93c8f2`, so it stays valid. The fixture sets are keyed by
  tag name, which the rewrite keeps.
- **Links into the private docs** from public files: `CHANGELOG.md` (about
  40), `features.md` (about 23), `gui-style.md`, `gui.md`, `core-api.md`,
  `config-files.md`, `README.md`, `AGENTS.md`, and
  `sketches/store-library-import.md`.
- **Local clones the rewrite strands:** the main checkout, the makepkg
  checkout in `./wineo/` (plus `src/` and `pkg/`), the leftover `0054-step0`
  branch, and any clone on another machine.
- **Tools.** `gh` is logged in as `MeandrousShark` with `repo` and
  `workflow` scopes. `gitleaks` is installed. `git-filter-repo` isn't.
- **The remote** is `gitea:mphannon/wineo.git`. Push-to-create is off on
  that Gitea, so a new repo has to be made in the web UI before the first
  push.

## Proposed design

### The contract at 1.0

`stability.md` changes from future to present tense at release ("This is
the promise wineo makes", with no "until then"). Two gaps get closed first:

- `library.toml` and `themes/` aren't on either list. They go under "Not
  promised" (answer 2): the desktop app owns them, they're read leniently,
  and an older wineo ignores what it doesn't know.
- The priority scale in `features.md` is defined around 1.0 ("1 = next: on
  the road to 1.0"). It gets a new anchor, the first 1.x releases, with
  priority 1 meaning "next" and the rest unchanged.

The "1.0 criteria" section in `features.md` becomes a short "1.0" note
saying when the criteria were met, with the details left to git history.

### History scrub

Done on a fresh `git clone --mirror` of Gitea, never on the working
checkout, while Gitea is frozen:

0. **Maintainer-present: freeze Gitea.** The maintainer confirms that
   nothing else will push to `mphannon/wineo` until the freeze ends: no
   other Claude sessions or coding agents running, no pushes from other
   machines, and every clone's work either pushed or saved outside git. A
   commit pushed after the mirror clone would be lost to the force-push,
   and one pushed from an unscrubbed clone afterward would bring the old
   history back. Gitea can't enforce this, since every push comes from the
   maintainer's account, so it's a checkpoint plus a check:
   `git ls-remote origin > frozen-refs.txt` records every branch and tag
   on Gitea at that moment.
1. Keep a backup: `git bundle create wineo-pre-scrub.bundle --all`, on
   disk outside the repo, until 1.0 has been out for a while.
2. `git filter-repo --path crates/target --invert-paths`. filter-repo also
   rewrites abbreviated hashes in commit messages and writes a commit map.
3. Check the result: no `crates/target` path in any commit
   (`git rev-list --objects --all | rg crates/target` is empty), every tag
   still points at a commit with the same tree apart from `crates/target`,
   and the pack is far smaller than today's (about 551 MiB).
4. Run `gitleaks git` over the rewritten history, and
   `git bundle verify` on the backup.
5. **Maintainer-present:** check that Gitea's branch protection allows a
   force-push to main, and that `git ls-remote origin` still matches
   `frozen-refs.txt` exactly. Any difference means something was pushed
   during the freeze, and the scrub starts over from a new mirror clone.
   Then push branches with `--force-with-lease` against the frozen hashes,
   so a push that slipped in after the check is refused rather than
   overwritten, and push tags with `--force`. Not `--mirror`, which would
   also delete refs Gitea has that the clone doesn't.
6. Re-sync every local clone, tags included. Resetting a branch leaves its
   old tags behind, and they still hold `crates/target`, so a later
   `push --tags --force` from that clone would undo the scrub. The main
   checkout is re-cloned with the maintainer present, after its untracked
   work (the `.private.git` folder and the private paths) is moved across.
   A clone that has to stay deletes its tags and fetches them again
   (`git tag -l | xargs git tag -d && git fetch --tags`). Either way, it
   passes the same `rg crates/target` check before anything is pushed from
   it. `./wineo/` (makepkg's checkout) is deleted so the next makepkg clones
   fresh.
7. **Maintainer-present: end the freeze** once every kept clone has passed
   that check and the main checkout is the fresh clone. Pushing to Gitea
   resumes from there.

The `0054-step0` branch and the `../.worktrees/wineo/step6-bench` results
folder are deleted beforehand, after the maintainer confirms.

Hashes inside plan files would go stale, so they're rewritten (answer 3).
A script reads `filter-repo/commit-map` from the rewritten clone, resolves
each hash it finds in `docs/plans/` to its full old hash in the backup,
replaces it with the new one at the same length, and leaves hashes from
before `ab93c8f2` alone, since those don't change. It then checks that every
rewritten hash resolves in the new history. The result is a commit in the
private repo.

### Private docs

`docs/plans/`, `docs/workflow.md`, and `docs/design/` are three sibling
paths, and one `.git` folder can't sit under all three. A `.git` inside
`docs/` would also make the public repo see `docs/` as an embedded repo.
It uses a second git directory at the repo root (answer 4):

- `.private.git/` holds the private repo. It runs with the repo root as its
  work tree, through `git --git-dir=.private.git --work-tree=.` or a
  `pgit` alias.
- Its `info/exclude` ignores everything except those three paths.
- The public `.gitignore` ignores `.private.git/` and the three paths.
  Both repos share the work tree, and with it every `.gitignore`, which
  outranks `info/exclude`. So the private repo doesn't see a new plan
  until it's added with `pgit add -f`, and `pgit status` doesn't list
  untracked private files. The rules stay in the committed `.gitignore`
  anyway, so a fresh public clone can never pick up private files, with no
  setup step to forget. `workflow.md` says to add new plans with
  `pgit add -f`, and a `pgit-status` alias runs `status --ignored` over the
  three paths.
- The public repo drops them with `git rm --cached`, so the files stay on
  disk.
- Its history comes from the public repo (answer 5). Run `git filter-repo
  --path docs/plans --path docs/workflow.md --path docs/design` on a fresh
  clone, then move that clone's `.git` to `.private.git` with
  `core.worktree` set to the repo root.
- **Maintainer-present:** create the private repo (`wineo-private`) in
  Gitea's web UI first, since push-to-create is off.

Agents keep finding everything at the same paths. `workflow.md` gains a
line on committing plans with `pgit`, and on adding new ones with `-f`.

`workflow.md` also gets a "Setting up a checkout" section with the exact
first-use steps for a fresh machine: clone the public repo, clone
`wineo-private` bare into `.private.git`, set `core.bare` off and
`core.worktree` to the repo root, write its `info/exclude`, check out the
three paths, and set up `pgit` and `pgit-status`. `workflow.md` is itself
private, so the section is read on Gitea's web UI before the first
checkout. The steps are tested from scratch before the freeze (see
Testing).

Public docs then lose their links into private files:

- Links to plans become plain "plan NNNN" text.
- `README.md` drops "Work is planned in `docs/plans/`" and gains a short
  Contributing section: open a PR on GitHub, the maintainer merges it
  locally, and `AGENTS.md` has the code rules.
- `AGENTS.md` gets the settled note at the top.
- The sketches stay public (answer 8), with their plan links turned into
  text like everything else.

### GitHub mirror

- **Maintainer-present:** create an empty repo, `MeandrousShark/wineo`
  (answer 6), with no README or license so the first mirror push lands
  cleanly. It starts **private** and only goes public once every check in
  step 6 has passed (answer 12). Until then, deleting it is a complete
  rollback.
- **Maintainer-present:** create a fine-grained token limited to that repo,
  with contents and workflows write access (workflows, so the mirror can
  push `.github/workflows/`). Then add a push mirror in Gitea's repo
  settings with "sync when commits are pushed" on.
- Check that every branch and tag hash on GitHub matches Gitea.
- Actions keeps GitHub's default of asking the maintainer to approve
  workflow runs for first-time contributors, so a new contributor's PR
  runs only after an "Approve and run". Forks get a read-only token and no
  secrets either way.
- The mirror prunes branches that aren't on Gitea, so the maintainer's own
  PR branches are pushed to Gitea. Outside contributors' branches live in
  their forks and aren't touched.
- `Cargo.toml` gains `repository`, and `README.md`'s clone URL points at
  GitHub.
- The repo's `PKGBUILD` takes the GitHub HTTPS `source` and the GitHub
  `url` (answer 10), once the mirror is up, so makepkg never points at a
  repo that doesn't exist yet.

### CI

`.github/workflows/ci.yml` runs on `pull_request`, on `workflow_dispatch`,
so the maintainer can try it on main before the tag, and on
`workflow_call`, so the release workflow reuses it. It runs on
`ubuntu-latest` with stable Rust:

- the system packages the build needs (`pkg-config` and the libraries iced
  links against)
- `cargo fmt --check`
- both clippy runs from `AGENTS.md`
- `cargo test --workspace --locked`

There are no secrets, so fork PRs run the same as anything else. Since
Ubuntu isn't Arch, the first manual run may turn up test assumptions about
the host. Those get fixed as patch fixes before the tag.

The runtime package list for Ubuntu 22.04 is worked out once, in step 6,
from `readelf -d` on the built binaries plus whatever the GUI start in
`verify` turns up missing. It's kept in the README's install section and
in `release.yml`'s `verify` job, so the two can't drift without `verify`
going red.

`release.yml` runs on pushed tags matching the glob `v*` (answers 7 and
11). Workflow filters are globs, not regular expressions, so the filter
only narrows things down. The real check is a guard script,
`scripts/release-guard.sh`, which refuses the release unless every one of
these holds:

- The event is a tag push, and the ref matches
  `^refs/tags/v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$`.
- The version is at least `1.0.0`, the first release with this workflow.
- `[workspace.package] version` in the tagged commit's `Cargo.toml` equals
  the tag without its `v`.
- The tag is annotated, it points at the commit this run is for, and that
  commit is on main.
- `CHANGELOG.md` at that commit has a `## X.Y.Z - ` section.
- No GitHub Release exists for this tag yet, so nothing is ever
  overwritten, and the version is higher than every existing Release's, so
  an old tag can't publish even if its run starts again.

Separate workflows on the same trigger run in parallel with no ordering,
so the checks and the release live in one workflow, chained with `needs:`:

1. `check` runs the guard, then calls `ci.yml`, so the release runs
   exactly the checks a PR does.
2. `build` needs `check`. It builds `wineo` and `wineo-gui` in release
   mode inside an `ubuntu:22.04` container, for a glibc 2.35 floor, so the
   floor doesn't depend on GitHub keeping its `ubuntu-22.04` runner label.
   It packs them with `LICENSE`, `THIRD_PARTY.md`, and the README into
   `wineo-X.Y.Z-x86_64-linux.tar.gz`.
3. `verify` needs `build`. In a fresh `ubuntu:22.04` container with only
   the documented runtime packages installed, it unpacks the tarball and
   checks that `ldd` finds every library both binaries link, that no
   symbol needs a glibc newer than 2.35, that `wineo --version` runs, and
   that `wineo-gui` starts under the container's own `xvfb-run` and stays
   up for a few seconds without a panic. `ldd` alone isn't enough, since
   iced and wgpu load the Wayland, X11, xkbcommon, and graphics libraries
   at runtime with `dlopen`. The GUI start catches those, through X11 and
   the software renderer, since the container has no GPU.
4. `publish` needs `verify`, and it's the only job with `contents: write`.
   It runs the guard again as its first step, since a rerun can come long
   after `check` passed and another release may have landed in between.
   Then it publishes a GitHub Release, not a draft, with that tarball and
   that version's `CHANGELOG.md` section as its notes, including the
   runtime package list.

A red `check`, `build`, or `verify` skips everything after it, so nothing
is published. The tag itself is public either way once it's on Gitea, and
a red tag is fixed with a patch release.

Old tags can't publish, and the plan doesn't count on GitHub's event rules
for that. Two things hold on their own:

- **No existing tag contains the release workflow.** `.github/workflows/`
  first lands in step 1, after v0.39.1, so every tag up to then has no
  workflow to run. That's checked explicitly before the mirror goes up
  (see Testing). Old tags are never moved or re-created, and `v1.0.0` is
  the first tag on a commit that has the workflow.
- **The guard refuses them anyway.** A version below `1.0.0`, or one not
  higher than every published Release, fails the guard, whatever event
  started the run.

### The release

- `CHANGELOG.md`'s 1.0.0 section opens with what 1.0 promises, linking
  `stability.md`, then sums up the headline changes since 0.30. The
  per-release sections below it already hold the detail.
- The version goes to `1.0.0`, with the tag message `Wineo 1.0.0: A stable
  contract`.
- After the tag: the upgrade fixture set for `v1.0.0`, per
  `workflow.md`'s Releasing section.

### Rollback

Each stage has its own way back. The maintainer makes every rollback call.
The backup bundle and `frozen-refs.txt` are kept until 1.0 has been out
for a while.

- **Before the force-push** (scrub steps 0 to 4): Gitea hasn't changed.
  Delete the mirror clone and end the freeze.
- **The private split** (step 3): revert the public commit that untracked
  the three paths (the files are still on disk) and remove `.private.git`.
  The `wineo-private` repo on Gitea can stay or go. Before the scrub this is
  an ordinary revert.
- **After the force-push, before GitHub** (scrub steps 5 to 7): Gitea is
  still frozen. Make a mirror clone of the backup bundle, then push its
  branches with `--force-with-lease` against the scrubbed hashes and its
  tags with `--force`. Gitea's `git ls-remote` must then match
  `frozen-refs.txt` exactly. Clones that were already re-synced get
  re-synced back the same way, tags included, and the private repo's
  hash-rewrite commit is reverted.
- **While GitHub is private** (steps 5 and 6): turn off the push mirror in
  Gitea, revoke its token, and delete the GitHub repo. Then fix forward, or
  roll Gitea back as above. The `PKGBUILD` source switch is reverted to
  the Gitea SSH source. Nothing has been public yet.
- **After going public** (step 7 on): published history can't be recalled,
  since clones, forks, and caches may already have it. That's why
  `gitleaks` and the `crates/target` checks run on GitHub's own copy before
  the flip. From here on, rollback means fixing forward: revert a bad
  commit, and rotate anything that leaked.
- **A bad release:** a red check already blocks publishing. A Release that
  published but turns out broken is deleted or marked as broken on
  GitHub, and the fix ships as a patch release. The tag stays.
- **Mirror trouble later:** if the mirror stops syncing, makepkg builds a
  stale commit. The `PKGBUILD` source goes back to Gitea SSH until the
  mirror is fixed.

## Implementation sequence

The order follows one constraint: the scrub has to happen before GitHub
ever sees a commit, since a mirror that has the old blobs can't be cleaned
by rewriting Gitea.

0. **Prerequisites** (local): the maintainer installs `git-filter-repo`;
   delete the `0054-step0` branch and the step6-bench folder once
   confirmed; rerun `gitleaks git`; sweep current public files for
   personal details (paths under `/home`, hostnames) beyond the
   `git.hannoncloud.com` URLs, which change in steps 2 and 5.
1. **CI workflows** (code): `ci.yml`, `release.yml` with its checks,
   build, verify, and publish chained by `needs:`, and
   `scripts/release-guard.sh`, checked with shellcheck and run by `check`
   and `publish`.
2. **Public docs cleanup** (docs): plan links to plain text across the
   files listed above, the README Contributing section and clone URL,
   `Cargo.toml`'s `repository`, the `AGENTS.md` note, `stability.md`'s
   two gaps, and the priority anchor in `features.md`.
3. **Private repo split** (**maintainer-present** for the Gitea repo):
   extract the three paths' history into `.private.git` and push it, then
   the public commit that untracks and ignores them. Then test the
   first-use steps from `workflow.md` on a fresh clone, before the freeze.
4. **History scrub** (**maintainer-present** for the freeze and the
   force-push): as above, including the plan-hash rewrite. Gitea stays
   frozen from the mirror clone until every kept clone is re-synced.
5. **GitHub mirror** (**maintainer-present**): first the check that no
   existing tag contains `.github/workflows/`, then the private repo,
   token, push mirror, a check that the hashes match, and the `PKGBUILD`
   source switch.
6. **CI shakedown**, all while the GitHub repo is private:
   - a `workflow_dispatch` run on main, with patch fixes for anything
     Ubuntu turns up
   - the Ubuntu 22.04 runtime package list, then a `verify` run against it
   - the release guard and gate tests
   - a real PR round trip, described under Testing
   CI doesn't count as working until all four pass.
7. **Go public** (**maintainer-present**): rerun `gitleaks git` and the
   `crates/target` check on a fresh clone from GitHub, then flip the repo
   to public.
8. **Release 1.0.0**: changelog, `stability.md` tense, the `features.md`
   1.0 note and queue, the version, the tag, and the push. The mirror
   carries the tag to GitHub and Actions runs.
9. **Upgrade fixture set** for `v1.0.0`.

## Testing and acceptance

- Before the force-push: the checks in the scrub section on the rewritten
  mirror clone, plus a fresh clone of it that builds and passes
  `cargo test`, and Gitea's refs still matching `frozen-refs.txt`.
- After the mirror: GitHub's branches and all tags match Gitea hash for
  hash, and a clone from GitHub has no `crates/target` in any commit and
  no private paths at `HEAD`.
- CI: the manual run on main passes. The 1.0.0 tag's run passes, the
  Release has the tarball, and its binaries run `wineo --version` on a
  non-Arch system or container.
- The release gate, checked in step 6 on a throwaway GitHub repo or with
  `act`, so no test tag reaches the real one, even while it's private:
  a tag whose checks fail leaves `build`, `verify`, and `publish` skipped
  and creates no Release, a tag that doesn't match the version fails
  `check`, a `verify` with a runtime package left out fails and publishes
  nothing, and a tag that passes the glob but not the guard (like `v-test`)
  fails `check`.
- The release guard, run locally against made-up inputs before the gate
  tests: it refuses a non-version tag, a leading zero (`v01.2.3`), a
  version below `1.0.0`, a lightweight tag, a tag that doesn't point at the
  run's commit, a commit not on main, a `Cargo.toml` version that doesn't
  match, a missing changelog section, a tag that already has a Release, and
  a version that isn't higher than the newest Release. It passes a correct
  `v1.0.0`.
- Old tags, before the mirror goes up: `git ls-tree -r <tag> -- .github`
  is empty for every existing tag.
- A PR round trip, in step 6. First, a throwaway PR from a branch pushed to
  Gitea, with a commit that breaks `cargo fmt`: CI runs on it and goes red.
  It's closed unmerged, and its branch is deleted on Gitea, which the
  mirror prunes from GitHub. Then a PR with a real small change: CI goes
  green, the maintainer fast-forwards it into main locally and pushes to
  Gitea, and GitHub marks the PR merged once the mirror syncs.
- Ubuntu 22.04: `verify` passes on the 1.0.0 tag, and the README's runtime
  package list matches the one `verify` installs.
- Docs: every relative link in `README.md` and `docs/` resolves, and
  `rg 'plans/|workflow\.md|docs/design'` over the public files finds only
  the `AGENTS.md` note and `.gitignore`.
- Private first use, in step 3 before the freeze: follow
  `workflow.md`'s "Setting up a checkout" steps word for word in a fresh
  folder on disk, from a fresh public clone. The three paths come back,
  and `git status` in the public repo and `pgit status` are both clean. A
  new plan file added with `pgit add -f` shows in `pgit-status` and never
  in the public repo's `git status`. It's committed and pushed to a
  throwaway branch of `wineo-private`, comes back in a second fresh
  setup, and then the branch is deleted. Any step that needed something
  the section doesn't say gets added to the section, and the test starts
  over.
- Tags: in every clone kept after the scrub, no tag reaches a commit with
  `crates/target`.
- makepkg from the repo's `PKGBUILD` builds 1.0.0 from GitHub.

## Decisions during implementation

- **Step 0 done** (2026-10-28). `git-filter-repo` is installed. The
  `0054-step0` branch, already merged into main, and the
  `../.worktrees/wineo/step6-bench` results are deleted. The personal
  details sweep of files that stay public found only the
  `git.hannoncloud.com` URLs in `README.md` and `PKGBUILD`, which steps 2
  and 5 change. Everything else is made-up test paths like `/home/me`.
- **One gitleaks finding, a false positive.** `gitleaks git` over 2,271
  commits flagged `generic-api-key` in `cb67d49e`, at
  `crates/wineo-core/src/key_store.rs:325`: a test's temp file names
  (`"key"`, `"key.123-0.tmp"`), whose content is the literal `"secret"`.
  That commit predates `ab93c8f2`, so its hash survives the scrub, and step
  2 adds its fingerprint to a `.gitleaksignore` so the reruns in steps 4
  and 7 come back clean.

- **Steps 1 and 2 done** (2026-10-28), reviewed by qa-gpt. Both of its
  findings are fixed: the guard pages through every Release through
  `gh api --paginate` and refuses if the list can't be read, and the
  workflow-level concurrency group is gone.
- **No concurrency group.** GitHub keeps only one pending run per group,
  so a third tag push would cancel the release waiting in line. Instead,
  the guard's second run is the last step before `gh release create`, which
  shrinks the race between two tags to seconds, and the newest-Release
  check still refuses the older one once the newer is published. Push one
  version tag at a time.
- **Two helper scripts beyond the plan:** `scripts/changelog-section.sh`
  prints a version's changelog section for the guard and the release
  notes, so the two can't disagree. `scripts/readme-runtime-packages.sh`
  reads the README's marked package block for `verify` and the notes.
  `ci.yml` also gained a `scripts` job that runs shellcheck and the guard's
  32 tests, so the guard can't break unnoticed between releases.
- **CI installs only `pkg-config`.** The windowing and graphics crates
  load their libraries with `dlopen`, and `secret-service` uses its
  pure-Rust crypto, so the build links no `-dev` library. The build
  container leaves out `liblzma-dev` on purpose, so `lzma-sys` builds its
  own copy and the binary has no `liblzma` runtime dependency.
- **Limit: `verify` forces the software renderer** (`ICED_BACKEND=tiny-skia`),
  since the container has no GPU. It proves the X11 and software path, not
  the Vulkan and EGL libraries in the runtime list, which load with
  `dlopen` and fail softly. The README's list is the coder's first guess
  until step 6 settles it.
- **Links to plans** in public docs (93 of them across 7 files) became
  plain "plan NNNN" text, and `AGENTS.md` is the only public file left
  that names the private paths.

## Scope limits

- No features, CLI changes, or config changes beyond `stability.md`'s
  wording and its two gaps.
- deb, rpm, and AppImage packages are the next plan (queue item 2), and
  so is the AUR package (answer 9).
- No CONTRIBUTING.md, issue templates, or code of conduct. The README's
  Contributing section and `AGENTS.md` cover it.
- No email rewrite and no scrub beyond `crates/target`.
- CI doesn't run on pushes to main, doesn't run `--ignored` tests, and
  holds no secrets.
