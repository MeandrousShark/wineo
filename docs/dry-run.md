# Dry runs

```sh
wineo new -n hades -t gaming --exe ~/Games/Hades/Hades.exe -d --dry-run
wineo import prefix ~/old/hades -n hades --runner wine --dry-run
wineo import heroic --game 1234 -n hades --runner latest --dry-run
wineo import heroic --all --dry-run
```

`--dry-run` shows what `new` or `import` would create, copy, adopt, or
register, then stops. It checks everything the real command checks before
its first write and prints the same review, followed by `Dry run, nothing
was created.` or `Dry run, nothing was imported.`

A dry run never:

- writes anything: configs, prefixes, desktop entries, icons, art, logs, lock
  files, run state, or the Heroic id of an existing environment
- downloads anything: runners, DXVK, vkd3d-proton, art images, or the Steam
  Linux Runtime
- starts Wine, umu, an installer, or winetricks
- asks a question, so it works without a terminal

It needs the flags that answer everything, the same ones a run without a
terminal needs. Without them it prints the usual "needs ..." message.

## New

```
$ wineo new -n hades -t gaming --exe ~/Games/Hades/Hades.exe -d --dry-run
Would create environment hades
  prefix: ~/.local/share/wineo/prefixes/hades (new)
  runner: latest -> GE-Proton10-4 (Steam)
  template: gaming, win64
  components: dxvk 2.6, vkd3d-proton 2.14
  target: hades -> ~/Games/Hades/Hades.exe
  desktop entry: Hades
Dry run, nothing was created.
```

`--native` prints `Would create native environment NAME` with its target
and desktop entry. A prefix that already exists shows as `(existing prefix,
adopted)`.

A missing `--exe` fails with `PATH was not found.` and exits 1, since the real
run couldn't add the target either. A Windows path like `Z:\Games\setup.exe`
can't be checked before the prefix exists, because Wine makes the drive
links while building it, so it shows `(checked once the prefix exists)` and
the dry run still exits 0.

## Import

`import prefix` prints the same review as the real import, and `import
heroic --game` prints the game's summary.

`import heroic --all` previews the run the same flags would give:

- Without `--yes`, it shows what happens if every offer is accepted:
  `Would offer to download the latest DXVK for 1 game. Declining skips it.`
  With no Proton runner installed and no `--runner`, it previews the native
  games and says how many Windows games aren't shown.
- With `--yes`, it matches the real run exactly. No Proton runner and no
  `--runner` fails the same way, and a game missing a build just shows its
  `skip:` reason, since `--yes` never downloads.
- An environment that already has a matching Heroic game shows
  `Would record the Heroic id for 1 existing environment: NAME`.
- A game that would fail prints `NAME: would fail: REASON`. The rest still
  preview, and the dry run exits 1, like the real run's summary.

## Art

With `--art`, a dry run searches SteamGridDB the way the real run would,
but never saves the match or downloads images. It lists the top three and
marks the one that would be picked automatically:

```
Art from SteamGridDB, searching "Hades":
  1. Hades (2020), game 5248410 <- would be picked automatically
  2. Hades II (2024), game 5370392
  3. Hades: Battle Out of Hell (2003), game 5250117
```

`import heroic --all` prints one line per game with its top match. A missing
key or a failed search prints the real run's note and doesn't change the exit
code. This search is the only network use a dry run makes.

## Limits

A dry run can't promise the real run succeeds. Building the prefix,
downloads, and installers can still fail, and things can change on disk
between the two runs.
