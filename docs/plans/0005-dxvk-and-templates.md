Status: draft

# 0005: DXVK, vkd3d, and templates

## Goal

Let a wine environment run Direct3D through DXVK and vkd3d-proton, installed from drop-in builds the way runners are, and let `wineo new` start from a named template instead of a bare prefix.

## Decisions

Settled by the maintainer:

- DXVK and vkd3d builds are drop-in directories, the same model as 0003's runners. Wineo copies DLLs out of them and never downloads, extracts, or verifies anything.
- Downloading them is explicitly wanted, but it belongs to the existing `Runner downloads` row in `features.md`, which grows to cover DXVK and vkd3d alongside GE-Proton. That row is where checksums and extraction get built once, for all three.
- DXVK is a declarative setting wineo reconciles, not an install command. `dxvk = "2.7"` means the prefix has DXVK 2.7 in it: wineo installs it, swaps it when the value changes, and restores what it replaced when the value is unset.
- `dxvk` and `vkd3d` are refused on a Proton environment, the way `gameid` is refused on a wine one. Proton bundles both, and replacing them fights Proton's own updates.
- A template is a named file of environment defaults that `wineo new --template` applies. It sets config values only. Running winetricks verbs from a template is out of scope, because it would make creation take minutes and fail partway.
- `dxvk = "latest"` is a real value meaning the newest installed build. It is what makes a built-in template possible, since a built-in cannot name a version the user may not have extracted.
- A config edited by hand is reconciled the next time wineo opens the prefix. Editing the file is a supported way to change an environment, so `wineo set` is never something the user has to remember to run afterwards.

## Details

Everything in this section is a proposal for review.

### The drop-in roots

- `~/.local/share/wineo/dxvk/` and `~/.local/share/wineo/vkd3d/`, beside `runners/` and `prefixes/`. Overridable with `defaults.dxvk_root` and `defaults.vkd3d_root`, mirroring `defaults.runner_root`.
- One directory per build, named by its directory name, which is what a config's `dxvk` value holds. Extracting `dxvk-2.7.tar.gz` gives `dxvk-2.7/`, so the name a user types is whatever they renamed it to, exactly as runners work.
- A missing root is not an error. It means nothing is installed.
- The name rule is the runner one: ASCII letters, digits, `-`, `_`, and `.`, never starting with a dot.

### What counts as a build

- A DXVK build has `x64/dxgi.dll` and `x32/dxgi.dll`. A vkd3d build has `x64/d3d12.dll` and `x86/d3d12.dll`. The upstream projects differ on the 32-bit directory name, so each is checked for its own.
- Anything else is skipped, with the reason available, the same as a directory in the runners root that has no `bin/wine`.
- Wineo copies whatever `.dll` files it finds in those directories rather than a hardcoded list, so a build that adds or drops a DLL still installs correctly. What it sets overrides for is a fixed list, below.

### `wineo dxvk` and `wineo vkd3d`

- Two list verbs in the same table style as `wineo runners`, listing the installed build names. Which prefixes use a build is left out: it means reading every environment config to add one column, which no other list verb does, and it can be added later if it is missed.
- `--all` also lists skipped directories with what they were missing.
- The root's path is printed when nothing is installed, so it is obvious where to extract to.

### Installing into a prefix

- The DLLs are copied into the prefix, `x64/` into `drive_c/windows/system32` and the 32-bit directory into `drive_c/windows/syswow64`. A win32 prefix gets the 32-bit DLLs in `system32` and has no `syswow64`.
- Each file wineo replaces is kept beside it as `<name>.dll.wineo-orig`. Some wine builds have no PE file for a given DLL at all, in which case there is nothing to back up and removal simply deletes what wineo added.
- Overrides are written to `HKCU\Software\Wine\DllOverrides` as `native`, through the same `wine reg add` path that already disables winemenubuilder. Registry rather than `WINEDLLOVERRIDES` at launch, so the prefix genuinely has DXVK in it and winecfg run outside wineo shows the same thing.
- DXVK sets `d3d8`, `d3d9`, `d3d10core`, `d3d11`, and `dxgi`. vkd3d sets `d3d12` and `d3d12core`.
- What wineo installed is recorded in `<prefix>/.wineo-components.toml`: the component, the build name, and the files it wrote and backed up. The fact belongs to the prefix rather than the environment, so an adopted or shared prefix carries it, and removal can be exact rather than guessed.

### `latest`

- `dxvk = "latest"` means the newest installed build. It stays literal in the config and is resolved every time wineo reconciles, rather than being written back as a version, so dropping in a newer build and launching picks it up. That is the point of asking for it.
- Newest is decided by the dotted run of digits in the directory name, compared number by number, so `2.10` is newer than `2.7` where a plain string sort gets it backwards. A name with no digits in it sorts below every name that has some, and equal numbers fall back to comparing the names, so the answer is always stable.
- `latest` is a reserved name, the way `wine` is reserved in the runners root. A directory actually called `latest` is ignored, with a note saying the name is reserved.
- Comparing only the numbers means a variant build, a gplasync one sitting beside a vanilla one, makes "newest" close to meaningless. Keeping variants side by side is a reason to name the build explicitly instead.
- `status` shows both, since what the config asks for and what is installed are different facts:

  ```
  dxvk    latest (2.7)
  ```

### Reconciling

- `wineo set hades dxvk=2.7` installs it before writing the config, so a failed install leaves the config alone.
- Changing the value removes the old build's files and installs the new one, in that order, so a half-swapped prefix is never left behind.
- `--unset dxvk` restores every backed-up file, deletes the ones with no backup, and removes the overrides.
- `wineo new` applies the setting after the prefix is created, as part of creation, so a template that asks for DXVK produces a ready prefix.
- Installing is an operation like winetricks: it takes the environment's lock and is refused while it is running.
- A config edited by hand is reconciled the next time wineo opens the prefix, which covers `run`, `start`, `exec`, and `winetricks` at once. It happens where the launch is resolved, so `start` reports a failure before handing anything to a supervisor.
- The check is cheap and almost always a no-op: the manifest records the build actually installed, and when that is what the config asks for, nothing is written. Only a real difference touches the prefix. `latest` costs one directory listing on top.
- A launch that cannot reconcile, because the build the config names is not installed, is refused rather than run with the wrong DLLs in place.

### vkd3d needs DXVK

- vkd3d-proton does not ship DXGI. It shares DXVK's, and needs DXVK 2.1 or newer beside it.
- So `vkd3d` set with no `dxvk` is refused, saying vkd3d-proton needs DXVK's DXGI. Wineo does not check the DXVK version against 2.1, because the build's name is not a reliable version.

### Status

```
hades: inactive
  runner  wine-tkg (wine-10.0)
  prefix  ~/.local/share/wineo/prefixes/hades
  dxvk    latest (2.7)
  vkd3d   2.14
```

- Shown only when set. A pinned build is shown as the name alone; `latest` also shows what it resolved to right now.
- A prefix whose manifest disagrees with the config gets a note on the line saying it will be installed on the next launch, so the state is visible before anything writes to the prefix.

### Templates

The built-in `gaming`, which names no version so it works anywhere builds are installed:

```toml
dxvk = "latest"
vkd3d = "latest"
gamemode = true
```

A user template, pinning everything for a machine that needs it:

```toml
# ~/.config/wineo/templates/hades.toml
runner = "wine-tkg"
arch = "win64"
dxvk = "2.7"
vkd3d = "2.14"
gamemode = true

[env]
DXVK_HUD = "fps"
```

- `~/.config/wineo/templates/<name>.toml`, in the config directory because a template is configuration, not data.
- A template may set anything an environment config may, except `prefix` and `targets`, which are per-environment by definition.
- `wineo new -n hades --template gaming` applies it, and explicit flags win over it: `--runner` and `--arch` override what the template says.
- `wineo templates` lists them, in the same table style as the other list verbs.
- A template naming a runner or a DXVK build that is not installed fails before anything is created, the same way `--runner` already does.
- `gaming` is the one built-in, and `latest` is what makes it possible. It fails with a useful message on a machine with no builds dropped in, rather than naming a version nobody has.
- A user file of the same name shadows a built-in, so `gaming` can be replaced without wineo getting in the way.

### Testing

- Discovery: a valid DXVK build, a valid vkd3d build, one missing its 32-bit directory, one with no DLLs, a dotted name, and a hidden directory. Verify which are listed and that `--all` gives a reason for each skip. Fake trees of empty `.dll` files are enough; nothing runs wine.
- Installing: the DLLs land in the right directories for win64 and win32 prefixes, originals are backed up, the manifest records what was written, and the overrides are set. The existing fake wine already answers `reg`, so the override write is covered without wine installed.
- Swapping a version removes the old files before writing the new ones, and unsetting restores every original and leaves no `.wineo-orig` behind.
- A DLL with no original in the prefix is added on install and deleted on removal.
- Refusals: `dxvk` and `vkd3d` on a Proton environment, `vkd3d` with no `dxvk`, an uninstalled build name, and installing while the environment runs.
- `latest`: picks `2.10` over `2.7` rather than sorting them as strings, ignores a directory named `latest`, puts a name with no digits last, is stable when two names carry the same numbers, and re-resolves to a newer build dropped in after the setting was made.
- Reconciling: a config edited by hand takes effect on the next `start` without a `set` first; a config that already matches the manifest writes nothing to the prefix; and a config naming a build that is not installed refuses the launch instead of running with the wrong DLLs.
- Templates: applying one sets every value it names, explicit flags win, an unknown template name fails, a template naming an uninstalled runner or build fails before the prefix is created, and `prefix` or `targets` in a template is rejected. The built-in `gaming` works with builds installed and fails clearly without them, and a user file of the same name shadows it.
- Ignored real tests, needing wine and a real DXVK build: install into a fresh prefix, confirm `wine winecfg` shows the overrides, run something that loads `d3d11.dll`, then unset and confirm the prefix is back to what it was.

## Phases

1. The drop-in roots, build discovery, and the `wineo dxvk` and `wineo vkd3d` list verbs.
2. Installing into a prefix: the settings, reconciliation, backups, overrides, the manifest, and `status`.
3. Templates.

Commit each logical change separately. Each phase ends with tests, fmt, and clippy passing, then a push.

## Done when

With a wine build in `runners/wine-tkg`, DXVK extracted to `dxvk/2.7`, and vkd3d-proton to `vkd3d/2.14`:

```
wineo dxvk                                     # lists 2.7
wineo vkd3d                                    # lists 2.14
wineo new -n hades --template gaming           # creates the prefix with both installed
wineo status hades                             # dxvk latest (2.7), vkd3d latest (2.14)
wineo set hades dxvk=2.7                       # pins it
wineo set hades dxvk=2.8                       # swaps the build
wineo set hades --unset vkd3d                  # restores the originals
```

Extracting a newer DXVK into `dxvk/2.9` and setting `dxvk = "latest"` by editing `hades.toml` directly makes the next `wineo start hades` install it, with no `wineo set` in between.

`wineo set hades vkd3d=2.14` with no `dxvk` set is refused. On a Proton environment both settings are refused, naming the bundled ones.
