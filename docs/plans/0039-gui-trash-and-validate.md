Status: done

# 0039: GUI removal, trash, and validate

## Goal

Remove environments, browse and restore the trash, prune it, and run
`validate` and its fixes from `wineo-gui`. This is the third GUI parity
plan, after [0037](0037-gui-settings.md) and
[0038](0038-gui-new-import-clone.md). Snapshots, portable copy, art,
download, exec, and winetricks follow in 0040.

It also adds a sidebar, the home for prefix management. 0039 puts Trash and
Checkup there, and 0040 adds Snapshots.

The look stays plain. A full UI polish pass comes after parity, so this plan
favors working controls over final layout.

## Settled by the maintainer

Decided on 2026-09-30, before drafting:

- **Remove** and **Check for problems** go in the environment's actions
  menu, which is both the card's right-click menu and the panel's
  **Actions** button.
- Prefix management gets its own place, separate from the library: a left
  sidebar with **Library**, **Trash**, and **Checkup**. Snapshots join it in
  0040.
- The remove dialog matches the CLI: a plain remove keeps the prefix, and
  an unchecked **Also trash the prefix and snapshots** box adds them.
- After a remove, a toast offers **Undo** for a short time.
- Validating one environment shows its results in the panel, like **Log**
  does, not in an overlay.

Added at the maintainer's suggestion on 2026-09-30, after the first draft:

- A small core change so validate's winetricks fix reports progress the way
  Play does.
- Cancel's reach during a batch of fixes is spelled out.
- Navigation and closing the window keep or wait for running changes.
- The library and trash refresh after every change, failed ones included,
  and partial results stay on screen.
- Tests for unreadable trash, interrupted removal and restore, and a fix
  shared by several environments.

Settled on 2026-09-30, from the draft's open decisions:

- The global settings gear stays in the top bar. Settings isn't rearranged
  for this release.
- Prune offers all three CLI selectors: one entry, older than N days, and
  Empty trash.
- The Undo toast lasts 10 seconds and pauses while the pointer is over it
  or it has keyboard focus. Restore on the Trash page stays available
  afterward.
- Checkup runs only when asked. The pending-trash badge already surfaces
  interrupted work.
- One management action runs at a time, which keeps cancellation, closing,
  and results simple.

Settled on 2026-09-30, during step 2:

- Ctrl+N opens Add only on the Library section. It does nothing on Trash
  or Checkup.
- Opening the global settings gear from Trash or Checkup returns to Library
  on Back. The polish pass may revisit this.

Settled on 2026-10-01, during step 5:

- Closing the panel leaves the results view, like Log. Reopening the
  environment shows its details, and a running fix batch still shows its
  status line there. **Check for problems** runs a fresh check, and the
  last batch's results stay above it until then.

## Existing foundation

- `trash::remove_with(paths, name, include_prefix, on_wait) -> Entry` moves
  the config, art, and playtime log (and optionally the prefix and its
  snapshots) into a new trash entry. It refuses a running environment and a
  lost run, and waits for wine to exit through `wineserver -w`, which can't
  be canceled.
- `trash::list`, `trash::inspect`, and `trash::pending_entries` read the
  trash. `Entry` carries the ID, name, removal time, whether the prefix was
  included or is still present, the phase, and what was left in place.
- `trash::restore(paths, id)` returns an entry to its original name and
  paths and refuses collisions.
- `trash::review_prune(paths, Selector)` builds a `PruneReview` for one ID,
  everything older than N days, or all entries. `trash::prune_reviewed`
  rechecks it under the locks and returns a result per entry.
- `validate::check_environment(paths, name)` and `validate::check_all(paths)`
  return a `Report` of `Problem`s with levels, codes, and optional `Fix`es.
  `validate::apply(paths, fix, &token, winetricks_events, download)` applies
  one fix, including `Fix::RecoverTrash` for interrupted trash work. Its
  winetricks fix always writes to stderr (`EnsureOutput::Stderr`), so it
  never emits the `Progress`, `OutputLine`, or `LogStarted` events the
  panel's install status and **Install log** depend on.
- A fix shared by several problems, like two environments missing the same
  DXVK build, is collapsed into one by the CLI's `fixes()` in
  `commands/validate.rs`. Core's `Report` doesn't do that itself.
- `Runner::wait_idle_for_copy(prefix, &token)` is a cancellable wait for
  wine to exit, added for 0038's prefix copies.
- The GUI already has the actions menu (`grid.rs`), a panel that swaps to
  a log view, background workers with cancel and **Force stop**, modal
  overlays, and the keep working or cancel and close prompt.

## Proposed design

### Core: a cancellable remove

`trash::remove_with` gains a token, or a sibling takes one, and waits with
`Runner::wait_idle_for_copy` instead of `wineserver -w`. The wait happens
before anything is written, so canceling it leaves the environment exactly
as it was. The CLI keeps its current behavior and output.

### Core: validate fixes for a frontend

- `validate::apply` takes the winetricks output policy as a parameter
  instead of hardcoding stderr. The CLI passes `EnsureOutput::Stderr` and
  prints exactly what it does today. The GUI passes
  `EnsureOutput::Log(install_log_path)`, the same as Play, which turns on
  the progress and log events.
- The CLI's `fixes()` moves into core as `Report::fixes()`, every distinct
  fix in report order, so both frontends collapse shared fixes the same way.

Nothing else in core changes. Restore and prune already have the shapes a
frontend needs.

### Sidebar

A narrow left sidebar, always visible:

```
+----------+--------------------------------+
| Library  |                                |
| Trash  3 |   (selected section)           |
| Checkup  |                                |
+----------+--------------------------------+
```

- **Library** is today's grid and panel. Settings and the Add flows still
  open from it as they do now.
- **Trash** shows a count of restorable entries, and a warning dot when an
  operation was interrupted and needs recovery. Both come from
  `trash::list` and `trash::pending_entries`, which are cheap, so they
  refresh with the library's 2 second tick.
- **Checkup** has no badge, since a full `validate` on every tick is too
  much work. It runs when the page opens.
- The global settings gear stays in the top bar for now. Whether it moves
  into the sidebar is a polish pass question.
- Leaving a settings page through the sidebar asks first when it has
  unsaved edits, like Back does.

### Removing

**Remove...** in the actions menu opens a modal:

```
Remove hades?
Its config, art, and playtime go to the trash.
You can restore them from Trash.

[ ] Also trash the prefix and snapshots
    ~/Games/hades (12.4 GB)

                         [Cancel] [Remove]
```

- The prefix path and its size show under the box. The size is measured
  off the UI thread with a cancellable walk and shows "measuring..." until
  it's ready. Native environments have no prefix, so the box is hidden.
- A running or busy environment has **Remove...** disabled, with "Stop it
  first" as the reason. A lost run gets the same treatment, pointing at the
  panel's existing **Clean up** button.
- While it runs, the dialog shows a spinner and "Waiting for wine to exit"
  when it gets there, with **Cancel**. Canceling only works during the wait;
  once data starts moving, the button is disabled until it finishes.
- On success the dialog and the panel close, the card disappears, and a
  toast at the bottom says "Removed hades to the trash." with **Undo** for
  10 seconds, paused while the pointer is over it or it has keyboard focus.
  Undo calls `trash::restore` with the entry ID. If restore
  refuses, for example because a new environment took the name, the toast
  shows the error instead.
- Desktop entries removed and anything left in place (the prefix after a
  plain remove) go in the toast's text, kept to one line.
- A failure leaves the dialog open with the error.

### Trash page

A list of entries, newest first:

```
Trash

hades     removed 2 days ago    prefix included
          [Restore] [Delete forever]
oldgame   removed 40 days ago
          [Restore] [Delete forever]

Delete entries older than [30] days   [Delete]
[Empty trash]
```

- Each row shows the name, how long ago it was removed, and whether the
  prefix was included. Hovering shows the ID and the prefix path left in
  place, if any.
- **Restore** restores that entry. Collisions and a replaced prefix come
  back as the row's error. A restored environment shows up in the library
  on the next tick.
- **Delete forever**, **Delete** (older than), and **Empty trash** build a
  `PruneReview` and show a confirm dialog listing what it will delete. The
  confirm freezes that review, like the CLI's prompt. Results show per
  entry; one failure doesn't stop the others.
- A plain-remove entry's prefix isn't in the trash, and the confirm says so
  ("hades's prefix at ~/Games/hades stays where it is.").
- Pruning shows a spinner on the affected rows. It can't be canceled, like
  the CLI. Closing the window during it offers **Keep working** or
  **Cancel and close**, and the latter waits for it to finish.
- Interrupted operations show as a banner at the top of the page with a
  **Recover** button per entry, which applies `Fix::RecoverTrash`. Those
  rows have Restore and Delete disabled until recovered.
- An empty trash shows "The trash is empty."

### Validate in the panel

**Check for problems** in the actions menu opens the panel (if it isn't
already) and swaps it to a results view, like **Log**:

```
1 error, 1 warning

x  target hades: Hades.exe not found
!  art: icon.png is recorded but missing
   [x] Drop the missing icon record

[Fix 1 problem]  [Check again]  [Back]
```

- It runs `validate::check_environment` on a worker. A clean result is one
  line: "No problems found."
- Problems use the CLI's messages and levels. A problem with a fix gets a
  checkbox under it with `Fix::label()`, checked by default, matching the
  CLI's "Enter fixes them all".
- A fix shared by several problems shows under each of them, and the boxes
  toggle together. It's applied once and counted once in **Fix N
  problems**.
- **Fix N problems** applies the checked fixes in order, each on its own,
  so one failing doesn't stop the rest. Downloads show their progress, and
  winetricks shows "Installing gdiplus (2 of 3)..." with an **Install log**
  button that follows the log live, exactly like Play.
- **Cancel** stops the fix that's running and skips the ones after it.
  Fixes that already finished stay done. Pressing it again as **Force
  stop** kills winetricks right away, as in Play. Only downloads and
  winetricks can be interrupted; the other fixes are quick file changes, so
  Cancel takes effect once the current one ends. This differs a little from
  the CLI, where Ctrl+C during a winetricks fix moves on to the next fix.
- When the batch ends, finished, failed, or canceled, the check runs again.
  The view shows what's left, followed by a results list that stays until
  the next check: "Fixed 2 problems.", each failed fix with its error, and
  "Canceled. 1 fix skipped." when it applies.
- A fix that hits a busy environment fails with the usual error, and the
  others still run.
- Hints without a fix, like "download a runner", show as plain text.

### Checkup page

The same view for `validate::check_all`, grouped like the CLI: the global
config, each template, each environment, and leftovers from environments
that no longer exist. Healthy environments aren't listed. Clicking an
environment's heading opens its panel in the library.

**Fix N problems**, **Check again**, progress, Cancel, shared fixes, and
results work as in the panel. Interrupted trash operations appear here too,
as they do in `wineo validate`.

### Running changes, navigation, and closing

A remove, restore, prune, recovery, or batch of fixes is a running change.

- One runs at a time. While one runs, the buttons that would start another
  are disabled with "Busy: removing hades" as the reason. Launching games
  and opening pages still work.
- The remove dialog is modal, so nothing else can be clicked while it runs.
- The others keep running when you switch sidebar sections, close the
  panel, or open Settings. Their status stays where they started, on the
  Trash row, the Checkup page, or the environment's panel, and the result
  is there when you come back. The sidebar shows a spinner next to Trash or
  Checkup while one of theirs runs.
- Closing the window during a running change offers **Keep working** or
  **Cancel and close**, as in 0038. Cancel and close cancels what can be
  canceled (the remove's wine wait, a download, winetricks) and waits for
  the rest, with **Force stop** for winetricks. Data that's already moving
  in a remove, restore, prune, or recovery is never cut off by closing.

### After a change

- When any running change ends, successfully or not, the library and the
  trash reload right away instead of waiting for the next tick. A failure
  can leave state behind, like a `removing` entry or a half-pruned one, and
  it should show up at once.
- An interrupted removal or restore that left a pending entry shows the
  error, then the recovery banner on the Trash page and the badge. The
  error says so: "Removing hades didn't finish. Recover it from Trash."
- Partial results stay on screen until the next action in that place: a
  prune lists which entries were deleted and which failed and why, and a
  remove that worked but couldn't rewrite a desktop entry says both.
- When the trash can't be read, for example unavailable portable storage
  or a bad pending marker, the Trash page shows that error instead of an
  empty list, the badge shows a warning dot, and nothing on the page can be
  clicked except **Check again**. A failed check on the Checkup page or in
  the panel shows its error the same way.

### Keyboard

| Key | Does |
|---|---|
| Delete | Open Remove for the selected card |
| Ctrl+1, Ctrl+2, Ctrl+3 | Library, Trash, Checkup |
| Ctrl+Z | Undo, while the toast shows |
| Esc | Close the remove dialog or confirm; on the panel's results view, go back to the details |

## Implementation sequence

1. **Core.** Token on the remove's wine wait, the winetricks output
   parameter on `validate::apply`, and `Report::fixes()`. CLI output
   unchanged.
2. **Sidebar shell.** Library, Trash, and Checkup sections with empty
   pages, the badge, navigation keys, and the unsaved settings prompt.
3. **Remove.** Menu item, dialog, prefix size, worker with cancel, toast
   with Undo, the Delete key.
4. **Trash page.** List, restore, the three prune paths with confirms,
   recovery banner, close-window prompt during prune.
5. **Validate in the panel.** Menu item, results view, fix selection,
   progress, and recheck.
6. **Checkup page.** Global report and fixes, headings that open a panel.
7. **Docs.** `docs/gui.md`, `docs/features.md`, `docs/trash.md` and
   `docs/validate.md` pointers, and `docs/core-api.md` for the remove token.

## Testing and acceptance

- Core: canceling during the wine wait leaves the config, art, playtime,
  prefix, and desktop entries untouched. A winetricks fix with log output
  emits `LogStarted` and `Progress`. `Report::fixes()` returns a shared
  download fix once. The CLI `rm` and `validate` tests pass unchanged.
- GUI `update` tests without a window: the prefix box defaults off and maps
  to `include_prefix`; Remove is disabled for running, busy, and lost
  environments; Undo restores by the entry's ID and reports a refusal; the
  toast expires; the trash badge counts restorable entries and flags
  pending ones; prune confirms hold the reviewed selection; fix checkboxes
  map to the applied fixes; a failed fix doesn't stop the rest; the sidebar
  asks before leaving unsaved settings.
- GUI `update` tests for the additions: Cancel skips the remaining fixes
  and keeps finished ones; a shared fix's boxes toggle together and it's
  applied once; a second running change is refused while one runs; switching
  sections keeps a running change and its result; every change, failed ones
  included, reloads the library and trash; partial prune results stay until
  the next action; an unreadable trash shows its error, not an empty list;
  a fixture with a `removing` and a `restoring` pending entry shows the
  banner, and Recover clears it.
- Acceptance adds: two environments pinning the same missing DXVK build in
  Checkup (one fix, one download); a winetricks fix with install progress
  and **Install log**, canceled partway; switching to Library during a
  prune and coming back to its result; closing the window during a remove
  wait and during a prune; and a GUI killed in the middle of removing a
  large prefix, then recovered.
- Acceptance: remove a Wine game with and without its prefix, Undo one of
  them, restore one from Trash, try restoring over a name that's been
  reused, delete one forever, empty the trash, prune by age, interrupt a
  removal and recover it from Trash and from Checkup, check an environment
  with a missing exe and a missing icon and fix the icon, and run a full
  Checkup with a leftover desktop entry.

## Acceptance checklist

Implementation landed on 2026-10-01 (through 69b045f); the workspace tests
pass (1,777). Maintainer acceptance in a real window, using throwaway
environments or a `WINEO_PORTABLE_DIR` folder, run with
`cargo run --release -p wineo-gui`. The maintainer confirmed all nine items
on 2026-10-01:

1. Remove a Wine game with and without its prefix, then Undo one of them.
2. Restore one from Trash, and try restoring over a name that's been
   reused (it should refuse on the row).
3. Delete forever, Empty trash, and Delete entries older than N days.
4. Interrupt a removal and recover it from Trash and from Checkup. Closing
   the app mid-remove of a large prefix on another filesystem from the
   config is the easy way.
5. Check for problems on a game with a missing exe and a missing icon, and
   fix the icon.
6. A full Checkup with a leftover desktop entry, and two games pinned to the
   same missing DXVK build (one fix, one download).
7. A winetricks fix with Install log, canceled partway.
8. Switch to Library during a prune and come back; close the window during a
   remove's wine wait and during a prune.
9. Look at Checkup with a long report, since its layout hasn't been seen in a
   window.

## Scope limits

- No restore under a different name. Core refuses collisions, and the GUI
  reports that the same way.
- No automatic trash expiry, matching the CLI.
- No trash entry sizes on the Trash page. That's a polish pass candidate.
- Snapshots, portable copy, art management, downloads, exec, and winetricks
  are 0040.
