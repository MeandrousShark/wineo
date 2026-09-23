Status: done

# 0013: Import prefixes with absolute internal directory links

## Goal

Let copied Heroic prefixes validate and publish when `pfx` is an absolute
symlink back to the source prefix root. This closes the Cyberpunk 2077 and
Dispatch failures found during plan 0009's real-library bulk import.

## Evidence

- The maintainer ran `wineo import heroic --all --config-dir
  ~/.config/heroic/`. Of seven selected games, five imported and two failed
  after copying with `failed to inspect .../.wineo-import-*/pfx/system.reg: No
  such file or directory`.
- In both failed source prefixes, `pfx` is an absolute symlink to that same
  prefix's root, and `pfx/system.reg` resolves to a readable file.
- `copy_entry` rebases an absolute internal link to the final destination.
  During staged validation, `staged_path` inspects the whole
  `staging/pfx/system.reg` path before handling intermediate links. The
  filesystem follows `pfx` to the final destination, which does not exist
  until publication, so inspection fails.
- Existing coverage handles `pfx -> .`, whose relative link works during
  staging, but not the absolute self-root form.
- The maintainer's `~/.local/bin/wineo` was built before plan 0011 and still
  prints the old `<` prompt. The current source has the same staged-path bug.

## Decisions

Approved by the maintainer:

- Keep the copied absolute link pointed at the final destination so it works
  after publication. Do not modify the Heroic source prefix.
- Resolve every path component used by staged validation. When an absolute
  internal link points into the final destination, validate through the
  corresponding staging path. Keep external link behavior unchanged and bound
  link traversal to avoid cycles.
- Apply the same resolution to staged executable checks, so a missing path
  behind an intermediate internal link is not silently treated as absent.
- Continue to publish only after staged validation succeeds. A staged
  validation failure leaves no registered environment or destination.
- After the implementation and checks pass, update the installed
  `~/.local/bin/wineo` and confirm bare `wineo` runs the fixed build. The
  maintainer controls when to retry imports against the real Heroic library.

## Implementation

1. Make staged path resolution walk path components rather than asking the
   filesystem to resolve an intermediate link into the unpublished final
   destination. Preserve existing internal and external link semantics.
2. Use that resolution in prefix validation and executable checks. Keep the
   copy, fingerprint, cancellation, and publication rules unchanged.
3. Add a regression fixture with `pfx` as an absolute link to its own source
   root. Check that copy mode publishes a valid prefix, rebases the link to
   the destination, and leaves the source unchanged. Cover a missing staged
   target and a link cycle so validation fails safely.
4. Add a CLI bulk-import regression using two such candidates alongside a
   normal candidate. Confirm both linked prefixes import, the summary is
   truthful, and a second run recognizes completed imports.

## Acceptance

- Focused core and CLI regression tests pass. Existing relative internal and
  external symlink tests still pass.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`
  pass. Run ignored real-Wine tests when local fixtures are configured;
  otherwise record why they were skipped.
- Independent QA confirms no source-prefix edits, no broken destination on
  failure, and that validation does not follow an unpublished final path.
- Bare `wineo` resolves to the updated local build. The maintainer can then
  retry the two failed real imports and check the remaining 0009 acceptance.

## Acceptance results

- Independent QA passed with no defects.
- Focused core import tests passed: 15/15. CLI import tests passed: 14/14.
- `TERM=linux cargo test --offline`, `cargo fmt --check`, offline Clippy for
  all targets with `-D warnings`, and `git diff --check` passed.
- Ignored real-Wine fixture tests were not run because
  `WINEO_TEST_PROTON` and `WINEO_TEST_DXVK` were unset.
- The release build was installed atomically to `~/.local/bin/wineo`. Its
  checksum matched `target/release/wineo`, bare `wineo` resolves to it, and
  `wineo --version` succeeds.
- The maintainer has not retried the Cyberpunk 2077 and Dispatch imports.
  That real-library retry remains maintainer-controlled.
