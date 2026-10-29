Status: draft

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

Public docs then lose their links into private files:

- Links to plans become plain "plan NNNN" text.
- `README.md` drops "Work is planned in `docs/plans/`" and gains a short
  Contributing section: open a PR on GitHub, the maintainer merges it
  locally, and `AGENTS.md` has the code rules.
- `AGENTS.md` gets the settled note at the top.
- The sketches stay public (answer 8), with their plan links turned into
  text like everything else.

### GitHub mirror

- **Maintainer-present:** create an empty public repo,
  `MeandrousShark/wineo` (answer 6), with no README or license so the
  first mirror push lands cleanly.
- **Maintainer-present:** create a fine-grained token limited to that repo,
  with contents and workflows write access (workflows, so the mirror can
  push `.github/workflows/`). Then add a push mirror in Gitea's repo
  settings with "sync when commits are pushed" on.
- Check that every branch and tag hash on GitHub matches Gitea.
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

`release.yml` runs on pushed tags matching `v[0-9]+.[0-9]+.[0-9]+`, so a
stray tag like `v-test` doesn't start it (answers 7 and 11). Separate
workflows on the same trigger run in parallel with no ordering, so the
checks and the release live in one workflow, chained with `needs:`:

1. `check` calls `ci.yml`, so the release runs exactly the checks a PR
   does, plus a check that the tag matches `[workspace.package] version`
   in `Cargo.toml`.
2. `build` needs `check`. It builds `wineo` and `wineo-gui` in release
   mode on `ubuntu-22.04`, for a glibc 2.35 floor, and packs them with
   `LICENSE`, `THIRD_PARTY.md`, and the README into
   `wineo-X.Y.Z-x86_64-linux.tar.gz`.
3. `publish` needs `build`, and it's the only job with `contents: write`.
   It publishes a GitHub Release, not a draft, with that tarball and that
   version's `CHANGELOG.md` section as its notes.

A red `check` skips the other two, so nothing is published. The tag itself
is public either way once it's on Gitea, and a red tag is fixed with a
patch release. The mirror's first push sends every existing tag at once,
and GitHub creates no events when more than three tags are pushed at once.
Those tags also predate the workflow files, so they can't start a release.

### The release

- `CHANGELOG.md`'s 1.0.0 section opens with what 1.0 promises, linking
  `stability.md`, then sums up the headline changes since 0.30. The
  per-release sections below it already hold the detail.
- The version goes to `1.0.0`, with the tag message `Wineo 1.0.0: A stable
  contract`.
- After the tag: the upgrade fixture set for `v1.0.0`, per
  `workflow.md`'s Releasing section.

## Implementation sequence

The order follows one constraint: the scrub has to happen before GitHub
ever sees a commit, since a mirror that has the old blobs can't be cleaned
by rewriting Gitea.

0. **Prerequisites** (local): the maintainer installs `git-filter-repo`;
   delete the `0054-step0` branch and the step6-bench folder once
   confirmed; rerun `gitleaks git`; sweep current public files for
   personal details (paths under `/home`, hostnames) beyond the
   `git.hannoncloud.com` URLs, which change in steps 2 and 5.
1. **CI workflows** (code): `ci.yml`, and `release.yml` with its checks,
   build, and publish chained by `needs:`.
2. **Public docs cleanup** (docs): plan links to plain text across the
   files listed above, the README Contributing section and clone URL,
   `Cargo.toml`'s `repository`, the `AGENTS.md` note, `stability.md`'s
   two gaps, and the priority anchor in `features.md`.
3. **Private repo split** (**maintainer-present** for the Gitea repo):
   extract the three paths' history into `.private.git` and push it, then
   the public commit that untracks and ignores them.
4. **History scrub** (**maintainer-present** for the freeze and the
   force-push): as above, including the plan-hash rewrite. Gitea stays
   frozen from the mirror clone until every kept clone is re-synced.
5. **GitHub mirror** (**maintainer-present**): repo, token, push mirror,
   a check that the hashes match, then the `PKGBUILD` source switch.
6. **First CI run** through `workflow_dispatch` on main, plus patch fixes
   for anything Ubuntu turns up.
7. **Release 1.0.0**: changelog, `stability.md` tense, the `features.md`
   1.0 note and queue, the version, the tag, and the push. The mirror
   carries the tag to GitHub and Actions runs.
8. **Upgrade fixture set** for `v1.0.0`.

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
  `act`, so no test tag reaches the real one:
  a tag whose checks fail leaves `build` and `publish` skipped and creates
  no Release, a tag that doesn't match the version fails `check`, and a
  non-version tag starts nothing.
- Docs: every relative link in `README.md` and `docs/` resolves, and
  `rg 'plans/|workflow\.md|docs/design'` over the public files finds only
  the `AGENTS.md` note and `.gitignore`.
- Private: a fresh clone of `wineo-private` into a fresh public clone
  restores the three paths, and `git status` in the public repo stays
  clean. A new plan file is added with `pgit add -f`, committed, pushed,
  and restored the same way, and never shows in the public repo's
  `git status`.
- Tags: in every clone kept after the scrub, no tag reaches a commit with
  `crates/target`.
- makepkg from the repo's `PKGBUILD` builds 1.0.0 from GitHub.

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
