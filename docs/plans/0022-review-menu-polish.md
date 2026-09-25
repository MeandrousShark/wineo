Status: approved

# 0022: Review menu polish

## Goal

Review menus put their accept action first in the list, as `1) Add it`, and
Enter picks it through `Choice [1]`. The keys line sits at the bottom, so the
way to accept is easy to miss. Move the accept action out of the list and into
the keys line, right above the prompt, so every review menu says what Enter
does in the same place. Also, after adding a target, go back to the menu
that opened the program list instead of showing the list again, and let
Finish save without asking a second time.

## Menus that change

- Target review in `picker.rs`: `Add it` when adding, `Done` when editing.
- Guided `new` review in `wizard.rs`: `Create it`.
- `setup` save review: `Save them`, with `Keep editing` staying in the list.
- `setup` unsaved changes before the installer: `Save and run the installer`.
- Heroic import review in `import_heroic.rs`, which already uses Enter to
  import but says so in the prompt label instead of the keys line.

## Proposed behavior

- The accept action leaves the numbered list, and the other entries are
  renumbered from 1.
- The keys line starts with what Enter does: `Enter add it, Esc back, Ctrl+C
  cancel`, or `Enter save them, Ctrl+C cancel` in a menu without back. The
  phrases are `add it`, `done`, `create it`, `save them`, `save and run the
  installer`, and `import`.
- The prompt becomes `Choice: ` with no default in brackets. An empty answer
  accepts, and typed answers work as before, including the `q` and `b`
  fallbacks.
- A note printed between the list and the prompt, like the import review's
  `Close Heroic and the game first.`, moves above the keys line so the keys
  line stays right above the prompt.
- A menu whose only entry was the accept action, like `Create it`, shows no
  list, just the keys line and the prompt.

```
Adding "sims-4-updater-v2.6.6":
  target       sims-4-updater
  exe          C:\users\steamuser\Documents\sims-4-updater-latest\sims-4-updater-v2.6.6.exe
  arguments    none
  working dir  the executable's folder
  desktop      no
  default      no
  1) Change the target name
  2) Change the arguments
  3) Change the working directory
  4) Change the desktop entry
  5) Make it the default
Enter add it, Esc back, Ctrl+C cancel
Choice: 
```

Typing `1` used to accept and now picks the first edit. That is the cost of
the change, and Enter is still the fastest way to accept.

## Leaving the program list after an add

- Today `picker::pick` shows the program list again after each add, until the
  user picks `s) done adding`. Instead, it returns as soon as one round adds
  at least one target. Picking several at once, like `1 3`, still adds them
  all in that round.
- A round that adds nothing, because the card was backed out of or "Add it
  again as another target?" was declined, shows the list again as today.
- The callers need no change. `setup`'s Add targets and its after-installer
  pick return to "What next?", which defaults to Finish once there are
  targets. Guided `new` after an installer goes on to "Save these changes?",
  where Keep editing opens the setup menu.
- Since the list is never shown again after an add, `s)` always reads `skip
  for now`, and `done adding` goes away.

## Saving without a second confirm

- `setup::save` asks "Save these changes?" after listing the changes. Finish
  on the "What next?" menu already means save, and the installer path has
  already asked "Save and run the installer", so both now save straight away
  and print `Saved <env>.` without listing the changes.
- Guided `new` keeps its review, since its Keep editing is the way into the
  setup menu. It gets the Enter hint above: `Enter save them`.
- The default target question stays everywhere. When there are several
  targets and none is the default, it is still asked before saving, and Esc
  still goes back to "What next?".
- If the config changed meanwhile and the changes were rebased, the review
  and its confirm come back once, since the result has not been seen.

## Implementation sequence

1. Give `Prompt` one menu form for this, built from `choose_or_enter`, that
   takes the Enter phrase and prints it in the keys line. Move the import
   review onto it.
2. Move the target review, guided `new` review, and both `setup` reviews onto
   it, and update the tests that expect `Choice [1]: `.
3. Return from `picker::pick` after a round that adds a target, and drop
   `done adding`.
4. Let `setup::save` skip the confirm for Finish and the installer path,
   keeping it for guided `new` and after a rebase.

This starts after plan 0021 step 4 lands, since both touch `prompt.rs`.

## Verification and acceptance

- Unit tests for the keys line with and without back, a note above the keys
  line, an empty list, Enter accepting, and a typed number picking an edit.
- Picker tests: an add returns to the caller, several picks in one round are
  all added, and a backed out card shows the list again.
- Save tests: Finish and the installer path save with no confirm, the
  default target question is still asked when needed, guided `new` still
  reviews, and a rebase brings the confirm back.
- The pty tests in `crates/wineo-cli/tests/guided.rs` pass with the new
  output.
- The maintainer adds a target in `wineo setup`, lands back on its menu, and
  runs guided `new` to check the look.
- Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo
  test`.

## Scope limits

Menus that are not reviews, like the template or runner pickers, keep their
default in brackets. No keys change meaning beyond the renumbering above.
