# Agent Guidelines

For how work flows (plans, roles, landing, acceptance, releases), see `docs/workflow.md`.

## Code boundaries

- `crates/wineo-core` owns environment/config/process logic; `crates/wineo-cli` owns clap commands and interactive UI, and its binary is `wineo`; `crates/wineo-gui` owns the iced desktop app, calls core directly, and its binary is `wineo-gui`.
- `crates/wineo-cli/src/main.rs` handles the private supervisor entrypoint and dynamic completion before clap parsing. Background `start` re-enters the binary through `wineo-core::supervisor`.
- Root `src/`, `pkg/`, and `wineo/` are ignored Arch packaging work directories, not workspace source.
- Use core `Paths` for storage resolution: `--portable` overrides `WINEO_PORTABLE_DIR`; portable run state and cross-root locks still live in host XDG state.
- Config edits preserve TOML comments/formatting through `toml_edit`. Reads and no-op edits must not rewrite legacy files; see `docs/config-files.md` for schema rules.

## Build and verification

- Build on Linux with stable Rust supporting edition 2024, `pkg-config`, and D-Bus development files (`dbus` on Arch, `libdbus-1-dev` on Debian/Ubuntu).
- Build: `cargo build --release -p wineo-cli`. Run locally: `cargo run -p wineo-cli -- <args>`.
- Checks: `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test`. Both crates inherit `unsafe_code = "forbid"`.
- Focused tests: `cargo test -p wineo-core <filter>` or `cargo test -p wineo-cli --test lifecycle <test_name> -- --exact`.
- CLI integration tests reuse `crates/wineo-cli/tests/common/mod.rs`: fake Wine/umu runners, temporary XDG roots, isolated keyring access, and process cleanup on fixture drop.
- `cargo test -- --ignored` uses real programs and temporary prefixes. Some tests open windows; others require winetricks, `umu-run` plus `WINEO_TEST_PROTON` (directory containing `proton`), or `WINEO_TEST_DXVK` (directory containing `x64` and `x32`). umu may download the Steam Linux Runtime on first use. Select individual ignored tests when only some prerequisites are available.
- Snapshot and some portable tests probe reflink support and return early if unavailable. Their fixtures live under the crate directory, so coverage depends on the checkout filesystem, not just `/tmp`.
- `PKGBUILD` checks use `TERM=xterm-256color cargo test --workspace --frozen -- --test-threads=1` after `cargo fetch --locked`. `makepkg` builds remote `main` from its own checkout, not local uncommitted edits; see README for the `pkgver` Git filter setup.

## Rules

- Never use unicode dashes (em dash or en dash) in code, comments, docs, or commit messages. Use a plain hyphen (`-`).
- Commit every logical change with a short message and no attribution trailers.
- Comments should explain why, not what.
- Plan heavyweight features before implementing. Draft a plan in `docs/plans/` and get it approved by the project maintainer before writing any implementation code. Plans are for work where design decisions matter.
- Patch fixes and hotfixes need no plan. The primary session may implement them directly. If a fix turns up a design question, ask the maintainer or write a plan.
- Design decisions belong to the maintainer. When something is ambiguous or unspecified, ask instead of guessing.

## Releases

- Coding subagents may stage and commit their own logical changes. Only the primary session handles release work: pushes, merges, version bumps, changelogs, tags, releases, publishing, and deployments. Never delegate it.
- The version lives in `[workspace.package]` in `Cargo.toml`, and wineo stays on `0.x` until the maintainer calls it stable.
- Marking a plan `Status: done` bumps the minor version (`0.2.0` to `0.3.0`). Fixes that land between plans bump the patch version.
- Each bump gets a section in `CHANGELOG.md` with the date and a short list of changes, linking the plan when there is one, and an annotated `vX.Y.Z` tag.
- Push tags with the commits.

## Plans

- One file per plan: `docs/plans/NNNN-short-name.md`, numbered in order.
- Each plan starts with a status line: `Status: draft`, `Status: approved`, or `Status: done`.
- Only the maintainer approves a plan. Feature work beyond an approved plan needs a new or amended plan.
