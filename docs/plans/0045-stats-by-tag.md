Status: approved

# 0045: Tag breakdown in `wineo stats`

## Goal

See playtime and membership by tag, like how much time went into every
`game`, or which `work` programs never get opened. Second in the queue in
[features.md](../features.md), so it lands before store games bring their
own playtime.

## Settled by the maintainer

From the backlog row (2026-09-27):

- Show playtime and membership by tag.
- `stats [NAME]` already takes an environment name, so a bare `tags` word
  would clash with an environment called `tags`. The shape has to avoid that.

On 2026-10-06 the maintainer answered the open questions with each
recommendation:

1. Both flags: `--tag` filters the overview, `--by-tag` gives one row per
   tag.
2. `--by-tag` shows an `(untagged)` row.
3. `--by-tag` columns are played, sessions, played by, and last played.
4. No GUI changes in this plan.

## Existing foundation

- `wineo_core::stats::overview` (`crates/wineo-core/src/stats.rs`) reads each
  environment's playtime log without parsing configs, so a broken config
  still shows in `stats`. It returns rows sorted by name, a `played` index
  sorted by time, totals, warnings, and per-environment errors.
- Tags live in `environment.config.tags`. `report::overview` filters `list`
  by tags (`OverviewFilter.tags`). Several `--tag` flags mean all of them. A
  config that can't be read is an error only when a tag filter is given,
  since it can't be matched.
- `completion::tags` completes the tags in use, and `list --tag` uses it.
- `stats --json` prints the collection envelope (`environments`, `errors`),
  built from `StatsOutput` in `crates/wineo-core/src/view.rs`.
- The GUI calls `stats::overview` in `crates/wineo-gui/src/library.rs` for
  library cards.

## Proposed design

Two flags. The examples use the same library: hades and celeste tagged
`game`, hades also `co-op`, a never-played `game`, two `work` programs, and
one untagged environment.

### `--tag`: filter the overview

Works like `list --tag`, repeatable, with all tags required:

```text
$ wineo stats --tag game
         played      sessions  last played
hades    97 h               12  Sep 4
celeste  5 h 10 m            3  today
1 never played. Total 102 h 10 m.
```

The total and the "never played" count cover only the filtered environments.
When nothing matches, it prints `No environments are tagged game` and exits
0, like `list`.

### `--by-tag`: one row per tag

```text
$ wineo stats --by-tag
            played      sessions  played by  last played
game        102 h 10 m        15  2 of 3     today
co-op       97 h              12  1 of 1     Sep 4
work        0 m                0  0 of 2
(untagged)  3 h                1  1 of 1     Sep 17
An environment with several tags counts under each.
```

- Rows are sorted by time played, then by tag. `(untagged)` comes last, and
  can't clash with a real tag since tags can't contain parentheses.
- `played by` is how many of the tag's environments have been played, out of
  how many have the tag.
- The closing note only shows when some environment has more than one tag,
  since only then do the rows add up to more than the real total.
- `--by-tag --tag work` limits the rows to environments that also have every
  `--tag`, so `--by-tag --tag game` shows how `game` splits across the other
  tags.

### Both flags

- They conflict with `NAME` and `--import-heroic`.
- An environment whose config can't be read is reported as an error and the
  command exits 1, like `list --tag`, since its tags are unknown. Without
  either flag, `stats` keeps today's behavior and never parses configs.
- Heroic's imported time counts toward a tag's played time, like it counts
  toward the overview total, but not toward its sessions.

### JSON

`stats --tag game --json` prints the existing envelope with only the matching
environments.

`stats --by-tag --json`:

```json
{
  "tags": [
    {
      "tag": "game",
      "seconds": 367800,
      "sessions": 15,
      "environments": 3,
      "played_environments": 2,
      "last_played": "2026-10-06T19:00:00Z"
    }
  ],
  "untagged": {
    "seconds": 10800,
    "sessions": 1,
    "environments": 1,
    "played_environments": 1,
    "last_played": "2026-09-17T22:00:00Z"
  },
  "errors": []
}
```

`untagged` is `null` when every environment has a tag. `tags` is sorted like
the text rows.

### Core shape

`stats::overview` takes an optional tag filter and, when one is given or a
breakdown is asked for, loads each config for its tags. A new
`stats::by_tag(&Overview) -> TagBreakdown` groups the rows, so the GUI can use
it later without the CLI.

## Implementation sequence

1. Core: the tag filter in `stats::overview`, `stats::by_tag`, and the JSON
   view types in `view.rs`, with unit tests for grouping, several tags,
   untagged, imported time, a running session, and a broken config.
2. CLI: `--tag` and `--by-tag` on `stats`, with completion for `--tag`,
   text and JSON output, and integration tests.
3. Docs: `stats.md`, `json-output.md`, the `features.md` row, and the
   acceptance checklist.

## Testing and acceptance

- Core tests build logs and configs in temporary XDG roots, like the
  existing stats tests.
- CLI tests check the text tables, the no-match message, the closing note
  only appearing with several tags, both JSON shapes, the conflicts with
  `NAME`, and exit 1 with a broken config.
- Acceptance: tag a few real environments, compare `stats --tag` with `list
  --tag`, and check that the `--by-tag` totals match the plain overview when
  nothing has two tags.

## Acceptance checklist

1. Tag a few real environments if they aren't already, with at least one
   having two tags: `wineo set NAME 'tags=["game", "co-op"]'`.
2. `wineo stats --tag game` lists the same environments as
   `wineo list --tag game`, and its total is only theirs.
3. `wineo stats --tag nope` prints `No environments are tagged nope` and
   exits 0 (`echo $status` in fish).
4. `wineo stats --by-tag`: one row per tag, most played first,
   `(untagged)` last, and the "several tags" note at the bottom.
5. `wineo stats --by-tag --tag game`: only environments tagged `game` count,
   so `game`'s row matches step 2's total.
6. `wineo stats --by-tag --json | jq .` has a `tags` array and `untagged`.
7. `wineo stats` with no flags looks the same as before.
8. `wineo stats hades --tag game` is refused, since the flags don't take a
   name.

## Scope limits

- No GUI changes.
- No time ranges (this week, this month) per tag. They can be a later row.
- Tags stay as they are. No tag renaming or tag management commands.
