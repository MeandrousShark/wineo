Status: draft

# 0014: Dynamic environment and target completion

## Goal

Complete existing environment names and their target names from the current
Wineo configuration when the user presses Tab. Keep command and flag
completion working, and make the result usable in the maintainer's Fish shell.

## Decisions

Settled by the maintainer while drafting:

- Complete environment and target names in this plan. Runner, template,
  component, and other values stay for later work.
- Provide dynamic completion for Bash, Zsh, and Fish. Elvish and PowerShell
  keep their existing static command and flag scripts.
- Clone an environment is queued after this plan. It needs its own plan and
  maintainer approval before implementation.

The command and behavior details below are proposed for approval with this
plan.

## Existing foundation

- `wineo completions <shell>` currently generates static scripts for Bash,
  Elvish, Fish, PowerShell, and Zsh from the Clap command graph. Those scripts
  know commands and flags but cannot ask Wineo for current environment names.
- `clap_complete` is already pinned to 4.6.11. Its `unstable-dynamic` feature
  provides shell callback adapters and argument value completers for the five
  shells. Its registration protocol is marked unstable, so saved generated
  callback scripts can become stale across upgrades.
- `environment::names(&Paths)` reads sorted environment names from config file
  names without parsing every config. A broken environment config still has a
  name. Targets live in a loaded environment's `config.targets` map.
- The maintainer's shell is Fish, with a saved static script at
  `~/.config/fish/completions/wineo.fish`. Bare `wineo` resolves to
  `~/.local/bin/wineo`.

## Command behavior

`wineo completions bash`, `wineo completions zsh`, and
`wineo completions fish` emit a short shell initialization snippet. At shell
startup, the snippet asks the current `wineo` binary for its registration
script, so the completion protocol follows a Wineo upgrade without saving an
old callback script. The existing command stays available. Elvish and
PowerShell continue to receive the current static script.

For Fish, saving the output remains:

```sh
wineo completions fish > ~/.config/fish/completions/wineo.fish
```

The generated Fish file sources the current binary's registration script when
Fish loads Wineo completions. Bash and Zsh snippets can be sourced from their
startup files. Completions never print diagnostics, progress, or color into
the candidate stream. Ordinary Wineo commands are unaffected when no shell
completion request is active. `__supervise` remains hidden.

## Candidate behavior

- Complete existing environment names at the environment positional argument
  of `setup`, `config`, `start`, `stop`, `restart`, `programs`, `rm`, `status`,
  `logs`, `run`, and `exec`. Also complete the environment position of `set`
  when it is not `--global`, and only the first forwarded position of
  `winetricks`.
- Do not offer existing environment names for creation/import names,
  `set --global`, a `KEY=VALUE` setting, or passthrough program/Winetricks args.
- Complete `--target` for `start`, `run`, and `restart` using only targets in
  the selected environment. If the environment cannot be determined or its
  config cannot be loaded, return no target candidates. Do not offer targets
  from another environment.
- Match the current token's prefix. Preserve the stable, sorted name order.
  Completing a broken config's environment name still works; target
  completion for it returns no candidates.
- Candidate generation reads Wineo config only. It does not start Wine,
  inspect prefixes, download anything, write files, or need a terminal.
  Unreadable config paths yield no candidates instead of breaking the shell.

The target callback has to recover the selected environment from the partial
command line because Clap's value completer receives only the current token.
Keep that argument parsing isolated to completion code and cover option order,
the `--` passthrough boundary, and incomplete input in tests.

## Implementation

1. Enable `clap_complete`'s dynamic feature and process completion requests
   before ordinary Clap parsing and command output.
2. Attach environment name candidates to the existing argument definitions.
   Handle `set` and `winetricks` according to their positional rules.
3. Attach target candidates to the shared launch `--target` argument and
   resolve its selected environment safely from the partial command line.
4. Update `wineo completions` for the three selected shells, preserve static
   output for the other two, and update usage documentation and tests.
5. After verification, install the updated Wineo binary and replace the
   maintainer's saved Fish completion file. The replacement is atomic and
   does not edit unrelated Fish configuration.

## Acceptance

- CLI tests prove environment completion across the listed commands,
  including broken configs, prefix filtering, `set --global`, and the first
  `winetricks` value. Passthrough values are not completed as environments.
- Target tests prove selected-environment filtering for `start`, `run`, and
  `restart`, option order, missing/broken environment behavior, and `--`
  boundaries.
- Generated Bash, Zsh, and Fish snippets register live callbacks; Elvish and
  PowerShell still generate static command and flag scripts. All scripts
  exclude `__supervise`.
- Run real completion requests through Fish and Bash with isolated XDG config.
  Test Zsh in a shell when available; otherwise verify its generated script
  and callback protocol with isolated fixtures.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`
  pass. Run ignored real-Wine tests when their fixtures are configured;
  otherwise record why they were skipped. Independent QA reviews the change.
- Bare `wineo` resolves to the new tested build. Fish Tab offers the current
  environment and target names through the updated completion file.
