Status: draft

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
  one fix, including `Fix::RecoverTrash` for interrupted trash work.
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

Nothing else in core changes. Restore, prune, and validate already have the
shapes a frontend needs.

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
  10 seconds. Undo calls `trash::restore` with the entry ID. If restore
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
- **Fix N problems** applies the checked fixes in order. Downloads and
  winetricks installs show progress in the panel's status line, as Play
  does, with **Cancel** and **Force stop**. When the fixes finish, the
  check runs again and the view shows what's left, plus any fix that
  failed.
- A fix that hits a busy environment fails with the usual error, and the
  others still run.
- Hints without a fix, like "download a runner", show as plain text.

### Checkup page

The same view for `validate::check_all`, grouped like the CLI: the global
config, each template, each environment, and leftovers from environments
that no longer exist. Healthy environments aren't listed. Clicking an
environment's heading opens its panel in the library.

**Fix N problems**, **Check again**, progress, and failures work as in the
panel. Interrupted trash operations appear here too, as they do in
`wineo validate`.

### Keyboard

| Key | Does |
|---|---|
| Delete | Open Remove for the selected card |
| Ctrl+1, Ctrl+2, Ctrl+3 | Library, Trash, Checkup |
| Ctrl+Z | Undo, while the toast shows |
| Esc | Close the remove dialog or confirm; on the panel's results view, go back to the details |

## Decisions for the maintainer

1. **Gear placement.** Recommended: keep the global settings gear in the
   top bar for 0039 and decide in the polish pass. The alternative is a
   **Settings** row at the bottom of the sidebar now.
2. **Prune controls.** Recommended: all three CLI selectors (one entry,
   older than N days, and Empty trash), so the GUI has full parity. The
   lighter option is per-entry and Empty trash only.
3. **Undo window.** Recommended: 10 seconds, and the toast stays while the
   pointer is over it. Undo is also always available as Restore on the
   Trash page.
4. **Checkup on startup.** Recommended: no. It runs when the page opens.
   Interrupted trash work, the one thing that blocks other commands, already
   gets a badge through the cheap pending check.

## Implementation sequence

1. **Core cancellable remove.** Token on the wine wait, CLI unchanged.
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
  prefix, and desktop entries untouched; the CLI `rm` tests pass unchanged.
- GUI `update` tests without a window: the prefix box defaults off and maps
  to `include_prefix`; Remove is disabled for running, busy, and lost
  environments; Undo restores by the entry's ID and reports a refusal; the
  toast expires; the trash badge counts restorable entries and flags
  pending ones; prune confirms hold the reviewed selection; fix checkboxes
  map to the applied fixes; a failed fix doesn't stop the rest; the sidebar
  asks before leaving unsaved settings.
- Acceptance: remove a Wine game with and without its prefix, Undo one of
  them, restore one from Trash, try restoring over a name that's been
  reused, delete one forever, empty the trash, prune by age, interrupt a
  removal and recover it from Trash and from Checkup, check an environment
  with a missing exe and a missing icon and fix the icon, and run a full
  Checkup with a leftover desktop entry.

## Scope limits

- No restore under a different name. Core refuses collisions, and the GUI
  reports that the same way.
- No automatic trash expiry, matching the CLI.
- No trash entry sizes on the Trash page. That's a polish pass candidate.
- Snapshots, portable copy, art management, downloads, exec, and winetricks
  are 0040.
