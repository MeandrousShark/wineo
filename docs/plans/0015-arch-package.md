Status: approved

# 0015: Arch package for Wineo and shell completions

## Goal

Install Wineo and its live Bash, Zsh, and Fish completions as one locally built
Arch package on the maintainer's machine. Package updates should follow the
latest pushed `main` commit, and Fish should find the packaged completion.

## Decisions

The maintainer chose a `wineo-git` package sourced from pushed Gitea `main`,
with Bash, Zsh, and Fish completions and migration of the current manual binary
and Fish completion. This package is for local use; publishing to the AUR is
outside this plan. Clone an environment remains queued after completion work
and packaging, with its own plan.

## Existing state

- The workspace version is 0.1.0 and has no Git tags. The `wineo` binary is
  built by `wineo-cli`; the repository has `Cargo.lock` and `LICENSE`.
- `origin` is `gitea:mphannon/wineo.git`. The maintainer's SSH alias `gitea`
  points to the Gitea server. The package can use this alias to fetch `main`.
- `~/.local/bin/wineo` and
  `~/.config/fish/completions/wineo.fish` are manual files. Fish searches that
  completion directory before `/usr/share/fish/vendor_completions.d`, and the
  local binary wins over `/usr/bin/wineo` on this machine.
- Plan 0014 supplies live Bash, Zsh, and Fish completion snippets from
  `wineo completions <shell>`. Packaging starts after 0014 is implemented,
  tested, committed, and pushed.

## Package behavior

- Add a root `PKGBUILD` for `wineo-git` using
  `git+ssh://gitea/mphannon/wineo.git#branch=main`. It uses a monotonic
  revision-count Git version because the repository has no tags. Git sources
  use `SKIP` for the source checksum; Cargo uses the committed lockfile and
  frozen builds after dependency fetch.
- Build the `wineo` release binary from the CLI workspace member and run the
  normal non-ignored Rust test suite in `check()`. Record direct shared-library
  runtime dependencies and optional Wine/Proton utilities according to the
  built binary and actual commands.
- Install `/usr/bin/wineo`, the license, and generated completion files at
  `/usr/share/bash-completion/completions/wineo`,
  `/usr/share/zsh/site-functions/_wineo`, and
  `/usr/share/fish/vendor_completions.d/wineo.fish`. The snippets ask the
  installed binary for its current registration script at shell load time.
  Do not modify shell startup files or add a hard dependency on any shell.
- Ignore local `makepkg` build directories and package archives in Git.
  Document `makepkg -si` for local updates after pushing changes to `main`.

## Migration on the maintainer's machine

1. Verify that the current manual binary and Fish completion are the expected
   Wineo-generated files, then keep temporary backups of both.
2. Build and inspect the package, run ShellCheck on `PKGBUILD`, and install it
   with pacman. Package installation may require the maintainer's sudo prompt.
3. Remove the old manual binary and Fish completion only after the packaged
   files are present. Confirm bare `wineo` resolves to `/usr/bin/wineo`, its
   version works, and Fish loads the vendor completion with real environment
   and target candidates. Restore backups if migration fails.

## Acceptance

- `makepkg` builds a package from the pushed 0014 commit; package contents
  include the binary, license, and all three completion files in the paths
  above. The Rust suite passes in `check()`.
- `shellcheck` passes with only documented PKGBUILD-specific exclusions, and
  package metadata and dependencies match the built binary.
- On the maintainer's machine, pacman owns `/usr/bin/wineo` and the completion
  files. The manual files no longer shadow them. Fish completion offers live
  environment and selected target names.
