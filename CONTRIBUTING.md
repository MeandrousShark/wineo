# Contributing

Thanks for helping with wineo.

## Issues

Report bugs and ask for features in
[issues](https://github.com/MeandrousShark/wineo/issues/new/choose). For a
security problem, see [Security](SECURITY.md).

## Before you start

Small fixes can go straight to a pull request. For a new feature, or anything
with a design choice in it, open an issue first. The maintainer makes the
design decisions, and bigger features get a written plan before any code.

## Pull requests

Pull requests are welcome on GitHub. Before you open one:

- Run the checks. CI runs them too, and a first-time contributor's run starts
  once the maintainer approves it.

  ```sh
  cargo fmt
  cargo clippy --all-targets -- -D warnings
  cargo test
  ```

- Follow [AGENTS.md](AGENTS.md), which has the code boundaries and rules for
  people as well as agents.
- Keep each commit to one logical change with a short message.
- Update the matching docs in the same pull request.

The GitHub repository is a mirror of the one the maintainer works in, so an
accepted pull request is merged there, and GitHub marks it merged when the
mirror syncs.

## AI tools

AI tools are welcome. You're responsible for what you submit, however it was
written:

- A person reviews every change before it's submitted.
- You understand the change well enough to explain it and to answer review
  questions yourself.
- You've run it, and the checks pass.
- You say in the pull request if AI tools wrote part of it.

A pull request its author can't explain gets closed.

wineo is built this way too. AI agents write and review much of the code, and
the maintainer makes the design decisions, tests the results, and decides what
ships.
