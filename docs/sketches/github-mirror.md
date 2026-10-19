# GitHub mirror and CI

Publish wineo at 1.0 while the maintainer keeps working in Gitea. Backlog
row in [features.md](../features.md): Distribution, priority 1. The row
keeps its priority and status; this page keeps the detail.

From the maintainer (2026-10-14). Gitea stays primary and push-mirrors to
GitHub, tags included. Changes still land by merging into main directly;
PRs are for other people. Since the mirror force-pushes GitHub to match
Gitea, an outside PR is merged locally and pushed to Gitea, never with
GitHub's merge button; GitHub marks it merged when its commits reach main
unchanged (a squash or rebase means closing it by hand).

GitHub Actions runs fmt, clippy, and the normal tests on outside PRs (no
secrets for forks) and on tags, and a tag also builds release binaries for
a GitHub Release, where the release packages row's packages land later.
Not on every push to main, which is reviewed and tested before merging.
`--ignored` tests stay local: they need Wine, umu, and a display. Tags
from the mirror trigger Actions since Gitea pushes with a personal access
token. The mirror syncs on every push, not just on its interval.

The published AUR PKGBUILD clones the GitHub mirror over HTTPS, since the
Gitea SSH source only works for the maintainer; a local PKGBUILD can keep
using Gitea. Confirmed Gitea primary over GitHub primary on 2026-10-14.
