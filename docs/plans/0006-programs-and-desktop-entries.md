Status: draft

# 0006: Installed programs and desktop entries

## Goal

After something is installed into an environment, wineo can say what showed up
and turn a target into a real desktop entry that launches through
`wineo start`, so an app in a prefix feels installed on the desktop.

## Decisions

Settled by the maintainer:

- Detecting installed executables and writing desktop entries land together,
  because both read the same thing out of the prefix: the Start Menu shortcuts
  an installer left behind.
- The `Per-environment process tracking mode` row is dropped from
  `features.md`. 0001's phase 2 already reports the program's exit separately
  from the prefix going idle, which was the real problem. Revisit only if a
  real app forces it.

## Proposed details

Everything in this section is a proposal for review.

### What counts as an installed program

Verified on a fresh win64 prefix: 244 `.exe` files, every one of them under
`drive_c/windows`. Nothing installed puts anything there, so wineo does not
need a before and after diff. It scans `drive_c` and ignores
`drive_c/windows` entirely.

Two signals, listed separately because they are worth different amounts:

- Start Menu shortcuts: `.lnk` files under
  `drive_c/ProgramData/Microsoft/Windows/Start Menu/Programs` and
  `drive_c/users/<user>/AppData/Roaming/Microsoft/Windows/Start Menu/Programs`.
  Verified: a fresh prefix has the first with only `Administrative Tools` and
  `StartUp` in it, and does not have the second until something creates it.
  A shortcut means the installer deliberately pointed at that program.
- Loose executables: `.exe` files under `drive_c`, outside `drive_c/windows`.
  This is the fallback for installers that make no shortcuts, and for prefixes
  where a game was just unzipped in.

A shortcut wins. An exe a shortcut already points at is not listed again as a
loose exe.

### Reading a shortcut

A `.lnk` is a binary Shell Link. wineo needs the target path out of it, and
the shortcut's own file name for a label.

wineo parses the subset it needs by hand: the header's flags, skipping
`LinkTargetIDList` by its length prefix, then `LinkInfo`'s `LocalBasePath`,
preferring the `LocalBasePathUnicode` field when the optional offsets are
present. A shortcut it cannot read is skipped with its reason available, the
same way a directory that is not a runner is skipped in 0003.

Alternatives, in case the maintainer prefers one (see open question 1): a crate
like `parselnk`, or not parsing at all and storing the `.lnk` path as the
target's exe, since `wine start` launches shortcuts.

### Filtering

Loose exes are filtered, shortcuts never are, because filtering something the
installer chose to advertise is second-guessing it.

Skipped by name, case-insensitively: `unins*.exe`, `uninstall*.exe`,
`*crashhandler*.exe`, `crashreportclient.exe`, `vcredist*.exe`, `dxsetup.exe`,
`dotnet*.exe`. Skipped by location: anything under
`Program Files*/Common Files` and under a Start Menu `StartUp` directory.

Everything else is shown. The filter is about noise, not correctness, so a
`--all` flag turns it off.

### `wineo programs <name>`

Its own verb rather than something only `new` does, so it also works after an
installer run through `exec`, after winetricks, and on an environment that was
adopted with `new --dir`.

```
$ wineo programs hades
shortcut  Hades         C:\Program Files\Hades\Hades.exe
shortcut  Hades Config  C:\Program Files\Hades\HadesConfig.exe
exe       Hades         C:\Program Files\Hades\unins000.exe
```

- Same padded column style as `wineo runners`, no header row.
- Windows paths, because that is what a target's `exe` holds.
- `--all` includes the filtered loose exes.
- Nothing found prints a note to stderr and exits 0.
- It does not add a target. It prints the `wineo set` line to copy. Adding one
  from a pick is the guided wizard's job, in a later plan.

`new --install-exe` runs the same scan once the prefix goes idle and prints it
where the bare `Add a target with:` hint is printed now, then the hint.

### The desktop setting

A per-target setting wineo reconciles, matching what 0005 settled for `dxvk`:
`targets.<t>.desktop = true` means an entry for that target exists.

```toml
[targets.hades]
exe = 'C:\Program Files\Hades\Hades.exe'
desktop = true
name = "Hades"        # optional, what the entry is called
```

- Reconciled wherever wineo opens the environment, the same places 0005's
  components are, so a hand-edited config takes effect without remembering a
  command. `wineo set` reconciles immediately, which is what makes the entry
  appear at the moment you ask for it rather than after the first launch.
- Unsetting it, or removing the target, removes the entry.
- `wineo rm` removes an environment's entries. This is new work in `rm`.
- Refused when the target's exe cannot be resolved in the prefix, so a typo
  does not produce an entry that fails on click.

### The entry

`~/.local/share/applications/wineo-<env>-<target>.desktop`, honoring
`XDG_DATA_HOME`.

```
[Desktop Entry]
Type=Application
Name=Hades
Comment=Runs in the wineo environment hades
Exec=wineo start hades --target hades
Icon=wineo-hades-hades
Terminal=false
Categories=Game;
StartupWMClass=Hades.exe
X-Wineo-Environment=hades
X-Wineo-Target=hades
```

- `Exec` runs `wineo start`, not wine directly, which is the whole point:
  every setting the environment carries, gamemode, DXVK, umu, applies. It is
  also why quoting stays simple, since environment and target names are
  already restricted to letters, digits, `-`, and `_`.
- `Name` is `targets.<t>.name` if set, else the target name. The scan can
  suggest a name from the shortcut it found, which is where a pretty one like
  `Hades` comes from.
- `Path` is left out. wineo already resolves the working directory.
- `StartupWMClass` is the exe's base name, which is what wine reports, so the
  launcher matches the window to the entry instead of showing a second icon.
- The `X-Wineo-*` keys are how reconciliation finds entries wineo owns,
  including strays whose target is gone. No separate manifest.
- No `MimeType`, so no `update-desktop-database` run is needed.

### Icons

Getting a real icon means pulling it out of the exe's PE resources. wineo asks
wine to do it: it runs `winemenubuilder.exe -a -r <lnk>` for the one shortcut,
with `WINEDLLOVERRIDES` letting winemenubuilder run for that call only, then
keeps the icon it wrote to the hicolor theme, points `Icon=` at it, and deletes
the `.desktop` wine wrote alongside.

This reuses wine's own extraction instead of adding a PE and ICO reader, at the
cost of one odd call that deliberately turns back on the thing 0001 turns off.
Targets with no shortcut, and any failure, fall back to no `Icon=` key, which
gets the generic application icon. See open question 3.

### `wineo desktop`

A list verb only, no writing, since the setting does the writing.

```
$ wineo desktop
hades      hades   Hades
photoshop  ps      Photoshop
```

Entries carrying `X-Wineo-Environment` for an environment that no longer
exists are listed as stray, with a note on how to clear them.

### `status`

A `desktop` line listing the targets that have an entry, when any do.

### Testing

- Unit: `.lnk` parsing against committed fixture bytes, including a shortcut
  with only the ANSI path, one with the unicode path, and a truncated one.
  Scanning against a fake prefix tree covering both Start Menu locations, the
  shortcut-wins rule, and the filters. Entry rendering compared to exact file
  contents.
- CLI without wine: `wineo programs` on a fake prefix with and without
  `--all`; `set targets.x.desktop=true` writing into a temp `XDG_DATA_HOME`;
  `--unset` and target removal clearing it; a hand-edited config reconciled on
  the next `start`; `rm` clearing entries; `wineo desktop` listing strays.
- Ignored, needing wine: install something with a Start Menu shortcut, confirm
  `programs` finds it, set `desktop = true`, and confirm the entry has an icon
  and launches the app.

## Phases

1. Scanning: the Start Menu and loose exe signals, `.lnk` reading, filters,
   `wineo programs`, and the `new --install-exe` output.
2. Desktop entries: the setting and its reconciliation, writing and removing,
   icons, `wineo desktop`, `rm` cleanup, and `status`.

Commit each logical change separately. Each phase ends with tests, fmt, and
clippy passing, then a push.

## Done when

```
wineo new -n hades --install-exe ~/Downloads/HadesSetup.exe   # lists the shortcut it found
wineo programs hades
wineo set hades targets.hades.exe='C:\Program Files\Hades\Hades.exe'
wineo set hades targets.hades.desktop=true targets.hades.name=Hades
wineo desktop                                                 # lists it
wineo set hades --unset targets.hades.desktop                 # entry is gone
wineo rm hades                                                # any entries go with it
```

The entry appears in the desktop's launcher with an icon, clicking it starts
the app, and the launcher shows one icon for it rather than two.

## Open questions

Proposed answers in parentheses.

1. How to read a `.lnk`: parse the subset by hand, add a crate like
   `parselnk`, or skip parsing and store the shortcut path as the target exe?
   (parse it by hand, roughly 150 lines and no new dependency, since the only
   fields needed are a length prefix and one path)
2. Should `wineo programs` be able to add a target from a pick, like
   `--add 1`, or stay read-only until the wizard? (stay read-only, so there is
   one place that turns a program into a target)
3. Icons: drive wine's `winemenubuilder` for the extraction, add a PE and ICO
   reader to wineo, or ship without icons for now? (drive winemenubuilder, and
   fall back to no icon)
4. `Categories`: always `Game;`, or `Game;` only when the environment is
   Proton or asks for gamemode and something else otherwise? (`Game;` for
   those, `Utility;` otherwise, with no config key until someone wants one)
5. Should a target's `name` also be used anywhere else, such as `status` and
   `list` output? (no, keep it to the entry for now)
6. Does the scan need a way to see what a `--install-exe` run added
   specifically, rather than everything in the prefix? (no, since the scan
   outside `drive_c/windows` is empty in a fresh prefix, which makes
   everything it finds installed by definition)
