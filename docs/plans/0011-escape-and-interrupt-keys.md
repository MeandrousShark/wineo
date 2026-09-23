Status: done

# 0011: Esc to go back, Ctrl+C to cancel

## Goal

Move back and cancel off typed characters and onto keys. Today menus reserve
`b` and `q`, and text and yes/no prompts take `<` to go back. A reserved
character can collide with a real answer: typing `q` at the name prompt
names the environment `q`.

## Decisions

Settled by the maintainer:

- Ctrl+C is the way to cancel a guided flow.
- Esc goes back one step.
- Keep `b`, `q`, and `<` working as quiet fallbacks. The plain stdin reader
  (used when the line editor can't start) has no Esc key, so `<` remains its
  way to go back.
- The maintainer approved implementation of this plan and deferred independent
  QA until plan 0010 is also complete. Run the plan's automated checks now.

## Existing foundation

- `crates/wineo-cli/src/prompt.rs` reads lines through rustyline 18 with
  `custom-bindings`, already binding Ctrl+Backspace.
- Ctrl+C already cancels every prompt. In raw mode it arrives as
  `ReadlineError::Interrupted`, and outside it as a signal. Both become
  `Canceled`, which commands turn into `Import canceled` and exit 130.
- `special` handles `b` and `q` in menus. `text`, `confirm`, and `list`
  handle `<`.
- 0007 settled numbers for list items and letters for fixed actions, with
  `b` / `q` for back and cancel. This plan replaces the `b` / `q` part.
  Fixed action letters like `m) more` stay.

## Keys

- Esc is bound in the line editor to end the read with a back result,
  whatever was typed. A lone Esc waits for rustyline's key sequence timeout,
  since terminals also start Alt+key and arrow sequences with Esc.
- Esc at a prompt with nowhere to go back to does nothing.
- Ctrl+C is unchanged.
- Existing typed back and cancel inputs remain accepted without hints.

## Output

Menus and prompts drop `b) back   q) cancel` and `, or < to go back`. One hint
line replaces them where back is possible:

```text
Proton-CachyOS releases:
  1) cachyos-11.0-20260703-slr  320 MiB  [latest]
  2) cachyos-11.0-20260612-slr  318 MiB
  3) cachyos-10.0-20260520-slr  301 MiB
  m) more
Esc back, Ctrl+C cancel
Choice [1]:
```

Text and yes/no prompts show no hint, so they stay one line:

```text
Environment name [palworld]:
```

Docs that mention `b`, `q`, or `<` are updated.

## Acceptance

- Esc goes back and Ctrl+C cancels in guided `new`, `import`, `import
  heroic`, and download selection.
- Typing `q` or `b` at a text prompt is taken as text.
- The PTY tests in `crates/wineo-cli/tests/guided.rs` cover Esc and Ctrl+C.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test`,
  and `cargo test -- --ignored`.

## Acceptance results

Esc returns to the previous guided step, including when text is already typed.
At a root menu it leaves the question open. Ctrl+C still cancels with exit 130.
The old typed inputs remain working but are no longer shown as prompts.
PTY tests cover guided new, prefix import, and Heroic selection. A fixture-backed
download picker test covers leaving release selection; its provider selector
does not need a network request.

`cargo fmt --check`, `cargo clippy --offline --all-targets -- -D warnings`,
and `TERM=linux cargo test --offline` passed on the host. The sandboxed test
run stalled in the existing installer and signal tests, as recorded in 0012.
Ignored real-runtime tests were deferred because `WINEO_TEST_PROTON` and
`WINEO_TEST_DXVK` are unset. Independent QA was deferred until 0010 was
complete, as requested by the maintainer; see the follow-up below.

## Independent QA follow-up

After 0010 completed, independent QA passed with no code defect found. Three
focused Esc PTY tests, the Ctrl+C PTY test, and the fixture-backed download
back-key test passed.
