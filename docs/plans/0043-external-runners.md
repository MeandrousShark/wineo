Status: approved

# 0043: Runners from Steam, Heroic, and Lutris

## Goal

Let environments use Proton and Wine builds that Steam, Heroic, and Lutris
already installed, instead of downloading or copying them again. wineo reads
those folders in place and never changes or deletes what it finds there.

## Settled by the maintainer

From the backlog row in [features.md](../features.md) (2026-10-02):

- Scanning is opt-in. It's offered once, during the first environment add
  when that environment is a game.
- When a name exists in several places, wineo's own runners root wins, then
  Steam, then Heroic, then Lutris.
- Downloads never deletes builds wineo didn't install.
- Copies of the same build with the same contents are interchangeable, so
  the order only matters when same-named builds differ. Hashing whole trees
  is too slow, so this plan picks a cheap identity check.

On 2026-10-05 the maintainer answered every open question below with its
recommendation:

1. One switch: `external_runners = true` in global config, unset until
   asked.
2. The offer shows in guided `new` and the GUI Add flow when the gaming
   template is picked, and in Heroic import. (Later refined: a template
   tagged `game`; see Decisions during implementation.) GUI Settings and `wineo config`
   change it.
3. Folders with unusable names get a derived name (`Proton 9.0` is
   `Proton-9.0`).
4. Lists show each name's source, with a note only when a hidden copy is a
   different build. No validate warning.
5. With scanning on, Heroic import uses Heroic's build in place and copies
   nothing.
6. `runner = "proton"` considers every enabled source; the `*-Latest`
   aliases stay with wineo's own downloads.
7. External builds are never used while a portable folder is active.

## Decisions during implementation

- Step 1 review, 2026-10-05: the highest-priority folder that has a name
  owns it, usable or not. A broken build in wineo's root still errors
  instead of falling through to a working Steam copy, and the same holds
  between and within external sources. Lower copies, broken or not, are
  listed as hidden and compared.
- Within one source the first folder wins a name (Heroic native before
  Flatpak), but later same-named copies are still compared so a different
  build gets the note.
- In Steam's `steamapps/common`, only Proton trees count. Other folders
  there, like `Proton EasyAntiCheat Runtime`, are ignored silently.
- When two folders in one place derive to the same name, the one that
  needed no deriving wins, then alphabetical order.
- Setting `external_runners` inside a portable folder is allowed but has no
  effect, per answer 7.
- Step 2, 2026-10-05: when Heroic import uses a build in place, the CLI and
  GUI import reviews show the source in the runner row, like
  `GE-Proton-latest (Heroic)`, with no extra note or prompt. Reserved names
  (`wine` and the `*-Latest` aliases) are never used in place.

- Steps 3 and 4, 2026-10-05:
  - Builds in a runners root inside a launcher folder are labeled with that
    launcher (`Steam`) in the CLI and GUI, and the GUI hides Delete for
    them.
  - GUI Downloads keeps wineo's deletable builds on top and lists other
    launchers' builds in a read-only "From Steam, Heroic, and Lutris"
    section, each with its source and what uses it.
  - The GUI switch, "Builds from Steam, Heroic, and Lutris", goes in the
    New environments group of global Settings.
  - The GUI asks the one-time question as an inline card, with No and Yes.
    Answering saves it and refreshes the runner list in place. The Add flow
    has no separate runner step, so the card sits right under the Template
    picker (approved over placing it inside Additional wine options or on
    the Review page). In Heroic import it tops the Details step.
  - The CLI changes the setting with `wineo set --global external_runners=`,
    which already works, so answer 2's `wineo config` needs no new command.
  - Step 3 review: `wineo runners` puts the source column right after the
    kind, so long detail text can't push it away, and the note goes last.
  - Guided `new` folds the source into the runner row's existing
    parenthesis: `GE-Proton11-3 (Proton, Steam)`. wineo's own builds read as
    before.
  - The CLI offer is skipped whenever a flag already answered part of the
    flow (`-t gaming`, `-n`, `--runner`, `--game`, `--adopt`, `--dir`,
    `--arch`, `--yes`, `--all`, and the like).
  - "A game" means the picked template is tagged `game`, in both the CLI and
    the GUI, so a custom template tagged `game` asks and a `gaming` template
    edited to drop the tag doesn't.

## Amendment: a runners root inside a launcher folder

Approved by the maintainer on 2026-10-05, from a step 1 review note.

If `runner_root` points at, or inside, a folder this plan knows belongs to
Steam, Heroic, or Lutris (after resolving symlinks), wineo still launches
from it but treats its builds as that launcher's for removal: `runners
remove` and Downloads refuse with the same "GE-Proton11-3 belongs to Steam."
This holds whether `external_runners` is on or not, since the point is to
never delete another launcher's files. A root that merely contains a
launcher folder further down is unaffected.

Cleanup of wineo's own leftovers still runs in such a root: the removal
sweep and abandoned download stages only match `.wineo-remove-*` and
`.wineo-stage-*` folders, which only wineo creates, so they never touch the
launcher's files.

## Existing foundation

- `Runners` ([runners.rs](../../crates/wineo-core/src/runners.rs)) wraps one
  root. `resolve(name)`, `list()`, `newest_proton()`, and the
  `GE-Proton-Latest` / `Proton-CachyOS-Latest` aliases all read that root.
  Nearly every caller goes through `Runners::new(paths, global)`: launch,
  create, import, Heroic import, validate, snapshots, portable copies,
  winetricks, desktop entries, build locks, build removal, and the GUI's
  runner picker. That makes it the one place the new roots plug in.
- `check(dir)` already tells Proton from Wine trees, and `lock_resolved`
  takes the build lock by runner name and the root from the runner's own
  directory, so a runner found elsewhere locks the same way.
- Runner names must be ASCII letters, digits, `-`, `_`, and `.`
  (`name::is_valid_runner`).
- Heroic import copies Heroic's runner into wineo's root
  ([import.rs](../../crates/wineo-core/src/import.rs) `copy_runner`), with a
  receipt recording `"method": "copy"` and the source. `heroic.rs` already
  finds native and Flatpak Heroic config folders.
- The gaming template sets `tags = ["game"]` and `runner = "proton"`, which
  resolves to the newest installed Proton build at creation time.
- Global config is `deny_unknown_fields`; new keys follow the rules in
  [config-files.md](../config-files.md).

What's actually on the maintainer's machine (2026-10-05) shapes the details:

| Folder | Contents |
|---|---|
| `~/.steam/root/compatibilitytools.d` | `GE-Proton11-3`, `Proton-CachyOS-latest` (a symlink into Flatpak Heroic's tools) |
| `~/.local/share/Steam/compatibilitytools.d` | Same folder: `~/.steam/root` and `~/.steam/steam` both resolve to `~/.local/share/Steam` |
| `~/.config/heroic/tools/proton` | `GE-Proton11-1`, `GE-Proton-latest` (GE-Proton10-3), `Proton-CachyOS-latest` |
| Flatpak Heroic `tools/proton` | Same three names |
| `~/.local/share/wineo/runners` | `GE-Proton11-7`, `GE-Proton-latest` (a Heroic import copy of GE-Proton10-3), `proton-cachyos-11.0-20260703-slr-x86_64` |

So deduplication has to go by canonical path, symlinks point across
launchers, and Heroic's `*-latest` folders are real directories whose
contents change when Heroic updates them. Heroic's `Proton-CachyOS-latest`
is the same build as wineo's `proton-cachyos-11.0-20260703-slr-x86_64`
under another name. Valve's own Proton (`steamapps/common/Proton 9.0`,
`Proton - Experimental`) isn't installed here but is common elsewhere.

## Open questions

### 1. How is the setting stored?

Both are global config keys. Unset means "not asked yet", so the offer
knows whether to show.

A. One switch:

```toml
external_runners = true
```

B. A list of sources:

```toml
external_runners = ["steam", "heroic", "lutris"]
```

With B, the offer still writes all three on "yes", and leaving one out by
hand skips it.

Recommendation: **A**. The offer is one yes/no, and the source order is
already fixed, so a list adds a knob nobody asked for. B can come later
without breaking A's files if `true` keeps meaning "all".

### 2. Where is it offered, and where can it be changed?

A. The CLI's guided `new` and the GUI's Add flow show the offer when the
gaming template is picked, before the runner choice, if the key is unset.
Either answer is saved. GUI global Settings gets a switch, and the CLI
changes it with `wineo config`.

```text
Use Proton and Wine builds from Steam, Heroic, and Lutris too? [Y/n]
```

B. Same, but Heroic import also offers it, since importing is often the
first thing a Heroic user does.

Recommendation: **B**. The first game a Heroic user adds is usually an
import, and with question 5's answer the import is where the setting pays
off most.

### 3. What about folder names wineo can't use as runner names?

Valve's Proton folders have spaces (`Proton 9.0`, `Proton - Experimental`).

A. Skip them, and list them under `wineo runners --all` as "invalid name",
like the runners root does today.

B. Give them a derived name: every run of characters wineo doesn't allow
becomes one `-`, so `Proton 9.0` is `Proton-9.0` and
`Proton - Experimental` is `Proton-Experimental`. `runner = "Proton-9.0"`
resolves back to that folder.

Recommendation: **B**. Valve's Proton is the build most Steam users have,
and skipping it would make the Steam source feel broken.

### 4. What do lists show when the same name exists in two places?

The higher priority copy is used either way (settled). The question is what
the hidden copy looks like.

A. Nothing. Lists show one row per name.

B. Lists show one row per name with its source. A same-named copy that's a
different build gets a note on that row. Identical copies stay silent.

C. B, plus `wineo validate` and Checkup warn about any environment whose
runner name has a different build hidden behind it.

```text
A, B, and C on this machine (wineo runners):

A  GE-Proton-latest  proton  GE-Proton10-3
B  GE-Proton-latest  proton  GE-Proton10-3   wineo
C  same as B, and validate: "app: GE-Proton-latest also exists in Heroic as a different build."

With Heroic's copy updated to GE-Proton11-7:

A  GE-Proton-latest  proton  GE-Proton10-3
B  GE-Proton-latest  proton  GE-Proton10-3   wineo  (Heroic has a different build)
C  same as B, and the validate warning above
```

Recommendation: **B**. It shows why a build isn't the one a launcher would
use, at the moment someone is looking at runners. Validate warnings for
something that's working as configured would be noise.

### 5. Does Heroic import still copy runners when scanning is on?

A. With scanning on, an import uses Heroic's build in place by its name
(`runner = "GE-Proton-latest"`) and copies nothing. With scanning off,
it copies as today.

B. Always copy, as today.

Recommendation: **A**. It's the point of the feature, and a game keeps
following Heroic's own updates the way it did in Heroic. If wineo's root
already has the same name, priority means wineo's copy wins, as settled.

### 6. Do `runner = "proton"` and the `*-Latest` aliases look outside?

A. `"proton"` (newest Proton, used by the gaming template) considers every
enabled source. The `GE-Proton-Latest` and `Proton-CachyOS-Latest` aliases
stay with wineo's own downloads, since they're tied to `wineo download`
receipts and stable-release checks.

B. Both consider every source, reading external versions from Proton's
`version` file.

C. Neither does.

Recommendation: **A**. New games pick the newest build on the machine, and
the aliases keep meaning "what `wineo download` last fetched".

### 7. Portable layouts

A. External builds are never used while a portable folder is active, and
the offer never shows there. A portable folder carries what it needs.

B. The portable folder's own global config can turn it on, like any other
key.

Recommendation: **A**. Portable folders move between machines, and an
environment pointing at a Steam folder breaks on the next one.

## Proposed design

### Sources

A new core module, `external_runners.rs`, lists candidate folders in
priority order and drops repeats by canonical path:

1. Steam: `~/.steam/root/compatibilitytools.d`, `~/.steam/steam/compatibilitytools.d`,
   `~/.local/share/Steam/compatibilitytools.d`, Flatpak Steam's
   `~/.var/app/com.valvesoftware.Steam/.local/share/Steam/compatibilitytools.d`,
   and each Steam library's `steamapps/common` (Valve's Proton only:
   folders whose name starts with `Proton` and that pass `check`). Library
   folders come from `steamapps/libraryfolders.vdf`.
2. Heroic: `tools/proton` and `tools/wine` under the native and Flatpak
   config folders that `heroic.rs` already finds.
3. Lutris: `~/.local/share/lutris/runners/wine` and its Flatpak
   counterpart, `~/.var/app/net.lutris.Lutris/data/lutris/runners/wine`.

Within one source, the first folder wins a name. A missing folder is
skipped silently.

### Resolution

`Runners` gains the source list (empty unless the setting is on and no
portable folder is active). `resolve` tries wineo's root, then each source
in order. `list` merges them, one entry per name, each with its source.
`newest_proton` follows question 6.

`Runner` records its source (`Wineo`, `Steam`, `Heroic`, `Lutris`). Nothing
else about launching changes: Proton builds already run through umu with
`PROTONPATH` set to the build's folder.

### Identity

A cheap identity, used to tell whether two same-named builds differ:

- Proton: the `version` file's contents (`<timestamp> <build name>`).
- Wine: the size and SHA-256 of `bin/wine` plus
  `lib/wine/x86_64-unix/ntdll.so` (or the `lib64` layout), a few MB at
  most.
- A build with neither counts as unknown, and is never reported as
  different.

It's computed only when a name appears in more than one place.

### Protection

- `runners remove` and Downloads refuse external builds: "GE-Proton11-3
  belongs to Steam."
- Build locks keep working by name, so wineo's own operations still
  serialize. wineo can't stop Steam or Heroic deleting a build; that shows
  up as the existing "unknown runner" error and in Checkup, as it would if
  someone deleted a wineo build by hand.

### CLI and GUI

- `wineo runners` adds a source column, showing `wineo` for wineo's own
  builds. `--all` lists unusable external folders too.
- The GUI runner picker and Downloads show external builds with their
  source, without Remove. Global Settings gets the switch.
- The offer follows question 2.

## Implementation sequence

1. Core: sources, canonical dedupe, derived names, identity, the setting and
   its config docs, `Runners` resolution and listing, refusal in removal.
2. Core: Heroic import uses Heroic's build in place when scanning is on.
3. CLI: `wineo runners` source column, the offer in guided `new` (and
   Heroic import), `wineo config` support.
4. GUI: runner picker and Downloads labels, the Add-flow offer, the
   Settings switch.
5. Docs: a runners section in the docs, config-files.md, gui.md, features.md.

## Testing and acceptance

- Core tests with fake Steam, Heroic, and Lutris trees under a temp home:
  priority, canonical dedupe through symlinks, derived names round-trip,
  identity equal and different, setting off means nothing external, portable
  means nothing external, removal refused.
- Heroic import test: scanning on writes Heroic's name and copies nothing.
- CLI tests for the runners list and the one-time offer.
- Acceptance checklist on a real machine, below.

### Acceptance checklist

Run from a build of `main` (`cargo run -p wineo-cli --` and
`cargo run -p wineo-gui`). Start with the key unset:
`wineo set --global --unset external_runners`.

1. `wineo runners` looks as before 0043: wineo's own builds only, with
   `wineo` in the new source column.
2. `wineo new`, pick the gaming template. The question "Use Proton and Wine
   builds from Steam, Heroic, and Lutris too? [Y/n]" appears once. Answer
   yes, then Ctrl+C. `~/.config/wineo/config.toml` now has
   `external_runners = true`, with its other lines untouched.
3. `wineo runners` lists `GE-Proton11-3` as Steam, `GE-Proton11-1` as
   Heroic, and wineo's builds as wineo. No "different build" note, since
   wineo's and Heroic's `GE-Proton-latest` are both GE-Proton10-3.
4. `wineo runners remove GE-Proton11-3` prints `GE-Proton11-3 belongs to
   Steam.` without asking, and
   `~/.steam/root/compatibilitytools.d/GE-Proton11-3` is still there.
5. Make a throwaway: `wineo new -n acc-0043 --runner GE-Proton11-3`, then
   `wineo exec acc-0043 winecfg`. It opens on Steam's
   build. Remove it after: `wineo rm acc-0043 --delete-prefix`.
6. `wineo import heroic` for a manually added Windows game whose Heroic
   runner is `GE-Proton11-1`: the review's runner row reads
   `GE-Proton11-1 (Heroic)`, and nothing new appears in
   `~/.local/share/wineo/runners`. Cancel at the review if you don't want
   the game imported.
7. `wineo runners --portable /tmp/acc-0043` shows no Steam or Heroic builds.
8. GUI Downloads: wineo's builds keep Delete. A **From Steam, Heroic, and
   Lutris** section lists the others with "Steam, unused"-style lines and no
   Delete.
9. GUI Settings > New environments: **Builds from Steam, Heroic, and
   Lutris** is on. Turn it off and save: Downloads drops the section and the
   runner pickers lose the Steam and Heroic builds. Turn it back on.
10. Unset the key again, then GUI Add > New environment, pick the gaming
    template. The card shows under Template. Click **Yes**: it goes away and
    the runner override picker now includes "GE-Proton11-3 (Steam)". Close
    Add without creating anything.
11. Unset the key again, then GUI Add > Import from Heroic, open one game.
    The card tops the details step. **Yes** switches the runner to Heroic's
    build in place. Back out without importing.
12. Leave the key set the way you want it.

## Not in this plan

- Installing or updating builds inside Steam, Heroic, or Lutris folders.
- Bottles' runners, which live inside its own Flatpak data and are mostly
  the same builds.
- Store library import, which is its own backlog row.
