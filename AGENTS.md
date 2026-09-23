# Agent Guidelines

wineo is a Wine environment manager written in Rust, organized as a Cargo workspace:

- `wineo-core`: library with the environment, config, and process logic
- `wineo-cli`: the `wineo` command, a thin clap front end over core
- `wineo-gui`: planned GUI client built on core

## Rules

- Never use unicode dashes (em dash or en dash) in code, comments, docs, or commit messages. Use a plain hyphen (`-`).
- Keep commit messages short. No co-author trailers or any other attribution.
- Write commits after every logical change.
- Keep code comments concise and only explain why, not what. Leave them out when the code speaks for itself.
- Plan before implementing. Draft a plan in `docs/plans/` and get it approved by the project maintainer before writing any implementation code.
- Design decisions belong to the maintainer. When something is ambiguous or unspecified, ask instead of guessing.

## Release Work

- The primary session handles all release work, including staging, commits, pushes, merges, tags, releases, publishing, and deployments.
- Do not delegate release work to the `release` subagent or any other subagent.

## Plans

- One file per plan: `docs/plans/NNNN-short-name.md`, numbered in order.
- Each plan starts with a status line: `Status: draft`, `Status: approved`, or `Status: done`.
- Only the maintainer approves a plan. Work beyond an approved plan needs a new or amended plan.

## Commands

- `cargo fmt`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- `cargo test -- --ignored` runs the tests that need wine installed. They create throwaway prefixes in temp dirs.
