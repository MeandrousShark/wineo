Status: done

# 0025: Fetch art during new and Heroic import

## Goal

Offer to fetch SteamGridDB art while creating an environment, so a new game
gets its icon, grid, hero, and logo without a separate `wineo art fetch`.
Covers `wineo new` and `wineo import heroic`.

## Existing foundation

- `commands/art.rs` has the whole fetch: `require_key`, `pick_game` (search
  plus a numbered pick list), `fetch_kinds` (one line per kind), and
  `apply_desktop`. `pick_game` opens its own `Terminal` prompt and fails
  without one.
- Art lives per environment (0021), so nothing can be fetched until the
  environment exists.
- Guided `new` (`wizard.rs`) asks Template, Name, Location, Settings, How,
  then Review. Nothing is written until the review is accepted. `create`
  then registers the environment, runs the installer, and opens the target
  picker. Target display names are only asked in the picker.
- Guided `import heroic` ends in `final_review`, an editable card that
  already has a `desktop` row. The Heroic title is known from the start.
- `import heroic --all` asks one "Create desktop entries for these games?"
  question. `--desktop` answers it ahead of time, and `--yes` without
  `--desktop` means no.
- A keyring that exists but fails (a dismissed unlock prompt) is an error
  in `require_key`, not "no key".

## Decisions (settled with the maintainer, 2026-09-25)

### 1. Key first, then the question

Every guided flow checks for a key before offering art. With no key it
prints one line and asks nothing:

```
note: run `wineo art key` to fetch art from SteamGridDB
```

A key lookup that errors, such as a dismissed keyring prompt, prints a
warning line and skips art the same way. The key is not checked against
the API up front; a bad key shows up as a fetch failure (decision 6).

### 2. Guided new: an Art step right before Review

A new `Step::Art` between How and Review, defaulting to yes. A yes asks for
the search term, prefilled with the environment name, since no display name
exists yet:

```
Fetch art from SteamGridDB?
  1) Yes
  2) No
Art [1]: 1
Search for [the-sims-4]: The Sims 4
```

The review card gains a row, and Back from Review returns here:

```
  art       fetch from SteamGridDB, searching "The Sims 4"
  art       none
```

The search and pick list run right after "Created the-sims-4.", before the
installer, so the user answers everything before walking away. The pick
list gains a last entry, `None of these, skip art`, which prints one line
and carries on:

```
Skipped art. Fetch later with: wineo art fetch the-sims-4 --search <term>
```

Ctrl+C at the pick list still cancels the rest of the flow and keeps the
environment, as it does today. Any targets added later in the picker get
the art icon on their first desktop entry write. Every pick list these
flows show has the skip entry; `wineo art fetch` keeps its list as is.

### 3. Display name as the search

- Guided `new`: the term typed in decision 2.
- Guided `import heroic`: the Heroic title.
- `import heroic --game ... --art` and `--all`: the Heroic title.
- Unguided `new --art`: the environment name, since there is no display
  name. There is no `--art-search` flag; `wineo art fetch --search` fixes a
  poor match afterward.

### 4. Unguided and bulk: off unless asked

- Unguided `new` fetches nothing unless `--art` is given.
- Fully specified `import heroic --game ... -n ...` fetches nothing unless
  `--art` is given.
- `import heroic --all` asks "Fetch art for these games?" next to the
  desktop question, only when a key exists. `--art` answers yes ahead of
  time, and `--yes` without `--art` means no, the same as `--desktop`.

### 5. Top match when nobody can pick

When there is no one to answer the pick list, the first search result is
used and named in one line:

```
hades: art from SteamGridDB game 1234, Hades (2020)
```

That covers unguided `new --art` without a terminal and every game in
`--all`, with or without a terminal, so a bulk import never stops for a
pick list per game. Guided flows, and unguided ones with a terminal, show
the pick list.

### 6. Failures warn and move on

A search error, no matches, a rejected key, or a failed download prints a
warning and the flow carries on. Kinds fetched before an error stay, as in
`art fetch`. Art never changes the exit code of `new` or `import`. In
`--all`, a game's art failure is noted in its own lines and does not count
as a failed game.

### 7. Which kinds: a global config knob

All four kinds by default. A new optional `[art]` table in `config.toml`
narrows it:

```toml
[art]
# "all" (default) or "icon"
fetch = "icon"
```

It only affects `new` and `import`; `wineo art fetch` keeps fetching every
kind unless `--only` says otherwise. The table is additive with a default,
so `schema_version` stays 1.

### 8. Guided Heroic import: a review row

Heroic's final review is already an editable card, so art is a row there
rather than a separate step, beside `desktop`:

```
  art       fetch from SteamGridDB, searching "Hades"
```

Editing it offers yes or no and the search term. The fetch runs after the
import is registered, before `desktop::reconcile`, so the entry is written
with the art icon the first time.

## Implementation sequence

1. **Global config.** `[art] fetch` in `GlobalConfig`, with tests for the
   default, `"icon"`, and an unknown value.
2. **Shared fetch.** Move the fetch out of `art.rs` into a helper the other
   commands call: key check, search, pick (`Pick::Ask(&mut Prompt)` or
   `Pick::Top`), then the kinds from decision 7. It takes the transport and
   prompt as parameters so tests can script both with the existing fake
   API. Errors become warnings here (decision 6). `art fetch` keeps its
   current behavior and exit codes.
3. **Guided new.** `Step::Art`, the review row, the fetch in `create`, and
   the skip entry. Wizard tests for no key, yes, no, Back, and skip.
4. **Unguided new.** `--art` with the pick list or top match.
5. **Heroic.** The review row, `--art` for the fully specified and `--all`
   flows, and the bulk question.
6. **Docs.** `docs/art.md` gains a short section on fetching during new and
   import; `downloads-and-imports.md` and the `--help` text get `--art`;
   `config-files.md` or the README notes `[art] fetch`.

## Verification and acceptance

- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and the full
  `cargo test` under a pty with `--test-threads=1`. No new wine-dependent
  tests.
- The maintainer's acceptance:
  - Guided `wineo new` with a key: the Art step, the review row, the pick
    list after creation, and the desktop entry icon once a target is added.
  - Guided `wineo new` with no key: the one-line note and no question.
  - `wineo import heroic` on one game shows the art row and fetches.
  - `wineo import heroic --all --yes --art` fetches top matches for every
    game without prompting.
  - `fetch = "icon"` limits both flows to the icon.

## Scope limits

- No art step in `wineo setup`, `import` of plain prefixes, `clone`, or
  `portable copy`. `clone` and `portable copy` already copy art.
- No Steam app ID or store ID lookups. Every flow searches by name.
- No per-target art during new or import; art goes to the environment.
