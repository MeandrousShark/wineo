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
  A tag also builds release binaries for a GitHub Release (but see question
  7).
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
  since the Gitea SSH source only works for the maintainer. A local
  PKGBUILD can keep using Gitea.

## Existing foundation

- **1.0 criteria 1 to 8 are done or closed** (features.md). The contract
  is written down in `stability.md`, exit codes in `exit-codes.md`, JSON
  schemas in `json-output.md`, upgrade fixtures and tests from 0053, and the
  deprecation path from 0053. Criterion 9, a stretch of daily use, is the
  maintainer's call (question 1).
- **`gitleaks git` over the full history found nothing** on 2026-10-10.
  About 1,400 commits have landed since.
- **`crates/target` in history.** It was tracked from `ab93c8f2`
  (2026-10-15) to `efcf688a` (2026-10-22): 2,854 files, about 2.4 GB
  uncompressed, against about 240 MB for everything else in history. The
  biggest other blobs are the two bundled Noto Sans fonts at about 620 KB
  each. Rewriting from `ab93c8f2` changes 511 commits and 10 tags (v0.33.0
  onward). Commits before it keep their hashes.
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

- `library.toml` and `themes/` aren't on either list. The proposal is
  "Not promised": the desktop app owns them, they're read leniently, and an
  older wineo ignores what it doesn't know (question 2).
- The priority scale in `features.md` is defined around 1.0 ("1 = next: on
  the road to 1.0"). It gets a new anchor, the first 1.x releases, with
  priority 1 meaning "next" and the rest unchanged.

The "1.0 criteria" section in `features.md` becomes a short "1.0" note
saying when the criteria were met, with the details left to git history.

### History scrub

Done on a fresh `git clone --mirror` of Gitea, never on the working
checkout:

1. Keep a backup: `git bundle create wineo-pre-scrub.bundle --all`, on
   disk outside the repo, until 1.0 has been out for a while.
2. `git filter-repo --path crates/target --invert-paths`. filter-repo also
   rewrites abbreviated hashes in commit messages and writes a commit map.
3. Check the result: no `crates/target` path in any commit
   (`git rev-list --objects --all | rg crates/target` is empty), every tag
   still points at a commit with the same tree apart from `crates/target`,
   and the pack is far smaller than today's (about 551 MiB).
4. Run `gitleaks git` over the rewritten history.
5. **Maintainer-present:** check that Gitea's branch protection allows a
   force-push to main, then push branches and tags with `--force`. Not
   `--mirror`, which would also delete refs Gitea has that the clone
   doesn't.
6. Re-sync every local clone: the main checkout resets to the new
   `origin/main` with the maintainer present, and `./wineo/` (makepkg's
   checkout) is deleted so the next makepkg clones fresh.

The `0054-step0` branch and the `../.worktrees/wineo/step6-bench` results
folder are deleted beforehand, after the maintainer confirms.

Hashes inside plan files go stale. Question 3 asks whether to rewrite them
from the commit map.

### Private docs

`docs/plans/`, `docs/workflow.md`, and `docs/design/` are three sibling
paths, and one `.git` folder can't sit under all three. A `.git` inside
`docs/` would also make the public repo see `docs/` as an embedded repo.
The proposal (question 4, A) is a second git directory at the repo root:

- `.private.git/` holds the private repo. It runs with the repo root as its
  work tree, through `git --git-dir=.private.git --work-tree=.` or a
  `pgit` alias.
- Its `info/exclude` ignores everything except those three paths.
- The public `.gitignore` ignores `.private.git/` and the three paths.
- The public repo drops them with `git rm --cached`, so the files stay on
  disk.
- **Maintainer-present:** create the private repo (`wineo-private`) in
  Gitea's web UI first, since push-to-create is off.

Agents keep finding everything at the same paths. `workflow.md` gains a
line on committing plans with `pgit`.

Public docs then lose their links into private files:

- Links to plans become plain "plan NNNN" text.
- `README.md` drops "Work is planned in `docs/plans/`" and gains a short
  Contributing section: open a PR on GitHub, the maintainer merges it
  locally, and `AGENTS.md` has the code rules.
- `AGENTS.md` gets the settled note at the top.
- What the sketches become is question 8.

### GitHub mirror

- **Maintainer-present:** create an empty public repo (question 6), with
  no README or license so the first mirror push lands cleanly.
- **Maintainer-present:** create a fine-grained token limited to that repo,
  with contents and workflows write access (workflows, so the mirror can
  push `.github/workflows/`). Then add a push mirror in Gitea's repo
  settings with "sync when commits are pushed" on.
- Check that every branch and tag hash on GitHub matches Gitea.
- `Cargo.toml` gains `repository`, and `README.md`'s clone URL points at
  GitHub.

### CI

`.github/workflows/ci.yml` runs on `pull_request`, on pushed `v*` tags, and
on `workflow_dispatch`, so the maintainer can try it on main before the
tag. It runs on `ubuntu-latest` with stable Rust:

- the system packages the build needs (`pkg-config` and the libraries iced
  links against)
- `cargo fmt --check`
- both clippy runs from `AGENTS.md`
- `cargo test --workspace --locked`

There are no secrets, so fork PRs run the same as anything else. Since
Ubuntu isn't Arch, the first manual run may turn up test assumptions about
the host. Those get fixed as patch fixes before the tag.

If question 7 says yes, `release.yml` runs on `v*` tags. It builds
`wineo` and `wineo-gui` in release mode on `ubuntu-22.04`, for a glibc
2.35 floor, packs them with `LICENSE`, `THIRD_PARTY.md`, and the README
into `wineo-X.Y.Z-x86_64-linux.tar.gz`, and attaches that to a GitHub
Release whose notes are that version's `CHANGELOG.md` section.

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
   `git.hannoncloud.com` URLs, which change in step 2.
1. **CI workflows** (code): `ci.yml`, plus `release.yml` if question 7
   says yes.
2. **Public docs cleanup** (docs): plan links to plain text across the
   files listed above, the README Contributing section and clone URL,
   `Cargo.toml`'s `repository`, the `AGENTS.md` note, `stability.md`'s
   two gaps (question 2), and the priority anchor in `features.md`.
3. **Private repo split** (**maintainer-present** for the Gitea repo):
   set up `.private.git`, commit the three paths there and push, then the
   public commit that untracks and ignores them.
4. **History scrub** (**maintainer-present** for the force-push): as
   above, including the plan-hash rewrite if question 3 says so.
5. **GitHub mirror** (**maintainer-present**): repo, token, push mirror,
   and a check that the hashes match.
6. **First CI run** through `workflow_dispatch` on main, plus patch fixes
   for anything Ubuntu turns up.
7. **Release 1.0.0**: changelog, `stability.md` tense, the `features.md`
   1.0 note and queue, the version, the tag, and the push. The mirror
   carries the tag to GitHub and Actions runs.
8. **PKGBUILD and AUR**: switch the published PKGBUILD's `source` to the
   GitHub HTTPS URL and its `url` to the GitHub repo, then question 9.
9. **Upgrade fixture set** for `v1.0.0`.

## Testing and acceptance

- Before the force-push: the checks in the scrub section on the rewritten
  mirror clone, plus a fresh clone of it that builds and passes
  `cargo test`.
- After the mirror: GitHub's branches and all tags match Gitea hash for
  hash, and a clone from GitHub has no `crates/target` in any commit and
  no private paths at `HEAD`.
- CI: the manual run on main passes. The 1.0.0 tag's run passes, and if
  question 7 says yes, the Release has the tarball, and its binaries run
  `wineo --version` on a non-Arch system or container.
- Docs: `rg 'plans/|workflow\.md|docs/design'` over the public files finds
  nothing, and every relative link in `README.md` and `docs/` resolves.
- Private: a fresh clone of `wineo-private` into a fresh public clone
  restores the three paths, and `git status` in the public repo stays
  clean.
- makepkg from the published PKGBUILD builds 1.0.0 from GitHub.

## Scope limits

- No features, CLI changes, or config changes beyond `stability.md`'s
  wording and its two gaps.
- deb, rpm, and AppImage packages are the next plan (queue item 2).
- No CONTRIBUTING.md, issue templates, or code of conduct. The README's
  Contributing section and `AGENTS.md` cover it.
- No email rewrite and no scrub beyond `crates/target`.
- CI doesn't run on pushes to main, doesn't run `--ignored` tests, and
  holds no secrets.

## Open questions

1. 🧐 **Is criterion 9 met?** It asks for daily use across at least two
   releases where upgrading a real library needs no manual fixes. The
   clock started at 0.30, nine minor releases ago. The one near miss is
   0.38.1's reboot fix, where wineo distrusted its own trash after a
   reboot until the patch shipped. A: yes, it's met. B: give it one more
   release, and 1.0 follows the next one that upgrades cleanly.
2. 🧐 **Are `library.toml` and `themes/` part of the contract?** A: no,
   add them to "Not promised" as files the desktop app owns, read leniently
   (recommended; the GUI isn't promised, and an older wineo already ignores
   what it doesn't know there). B: yes, add them to "Stable", which means
   key renames there need the deprecation path.
3. 🧐 **Rewrite commit hashes in plan files after the scrub?** A: yes,
   map each hash in `docs/plans/` through filter-repo's commit map in the
   private repo, so plans keep pointing at real commits (recommended; it's
   one script and the map comes for free). B: leave them, since the plans
   are private and old hashes stay findable in the backup bundle.
4. 🧐 **How does the private repo sit in the tree?** A: `.private.git` at
   the root with the repo root as its work tree, so the three paths stay
   where they are (recommended; it's what was settled, and agents and links
   keep working). B: move all three under one ignored `docs/private/`
   folder with an ordinary `.git`, which is simpler git but changes every
   path that points at a plan.
5. 🧐 **Does the private repo keep the plans' history?** A: yes, extract
   it with `git filter-repo --path docs/plans --path docs/workflow.md --path
   docs/design` on a copy, so `git log` on a plan still works
   (recommended). B: start fresh with today's files. The full history stays
   in the public repo either way.
6. 🧐 **Where does it live on GitHub?** A: `MeandrousShark/wineo`, the
   account `gh` is logged in as (recommended). B: somewhere else, like an
   organization.
7. 🧐 **Does a tag build release binaries?** A: yes, an x86_64 tarball of
   `wineo` and `wineo-gui` built on Ubuntu 22.04 (recommended; it's in the
   settled design and gives non-Arch users something before the packages
   plan). B: not yet, and the packages plan adds binaries with the
   packages.
8. 🧐 **Do the sketches stay public?** A: yes, as backlog detail that
   features.md links to, with their plan links turned into text
   (recommended; they read as design notes, not process). B: private,
   with the private docs, and features.md rows keep only their summaries.
9. 🧐 **Publish to the AUR at 1.0?** A: yes, the maintainer publishes a
   `wineo-git` package from the GitHub-sourced PKGBUILD after the tag
   (recommended; the settled design already points it at GitHub). B: not
   yet, and the packages plan adds the AUR alongside deb and rpm, maybe as
   a tagged `wineo` package.
