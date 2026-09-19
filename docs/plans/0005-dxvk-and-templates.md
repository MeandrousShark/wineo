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

- Two list verbs in the same table style as `wineo runners`: name, and the prefixes currently using it.
- `--all` also lists skipped directories with what they were missing.
- The root's path is printed when nothing is installed, so it is obvious where to extract to.

### Installing into a prefix

- The DLLs are copied into the prefix, `x64/` into `drive_c/windows/system32` and the 32-bit directory into `drive_c/windows/syswow64`. A win32 prefix gets the 32-bit DLLs in `system32` and has no `syswow64`.
- Each file wineo replaces is kept beside it as `<name>.dll.wineo-orig`. Some wine builds have no PE file for a given DLL at all, in which case there is nothing to back up and removal simply deletes what wineo added.
- Overrides are written to `HKCU\Software\Wine\DllOverrides` as `native`, through the same `wine reg add` path that already disables winemenubuilder. Registry rather than `WINEDLLOVERRIDES` at launch, so the prefix genuinely has DXVK in it and winecfg run outside wineo shows the same thing.
- DXVK sets `d3d8`, `d3d9`, `d3d10core`, `d3d11`, and `dxgi`. vkd3d sets `d3d12` and `d3d12core`.
- What wineo installed is recorded in `<prefix>/.wineo-components.toml`: the component, the build name, and the files it wrote and backed up. The fact belongs to the prefix rather than the environment, so an adopted or shared prefix carries it, and removal can be exact rather than guessed.

### Reconciling

- `wineo set hades dxvk=2.7` installs it before writing the config, so a failed install leaves the config alone.
- Changing the value removes the old build's files and installs the new one, in that order, so a half-swapped prefix is never left behind.
- `--unset dxvk` restores every backed-up file, deletes the ones with no backup, and removes the overrides.
- `wineo new` applies the setting after the prefix is created, as part of creation, so a template that asks for DXVK produces a ready prefix.
- Installing is an operation like winetricks: it takes the environment's lock and is refused while it is running.
- A prefix whose recorded state does not match the config, because someone edited the config by hand, is reconciled on the next launch. `status` says so before that happens.

### vkd3d needs DXVK

- vkd3d-proton does not ship DXGI. It shares DXVK's, and needs DXVK 2.1 or newer beside it.
- So `vkd3d` set with no `dxvk` is refused, saying vkd3d-proton needs DXVK's DXGI. Wineo does not check the DXVK version against 2.1, because the build's name is not a reliable version.

### Status

```
hades: inactive
  runner  wine-tkg (wine-10.0)
  prefix  ~/.local/share/wineo/prefixes/hades
  dxvk    2.7
  vkd3d   2.14
```

- Shown only when set. A prefix whose recorded state disagrees with the config gets a note on the line saying it will be reconciled.

### Templates

```toml
# ~/.config/wineo/templates/gaming.toml
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

### Testing

- Discovery: a valid DXVK build, a valid vkd3d build, one missing its 32-bit directory, one with no DLLs, a dotted name, and a hidden directory. Verify which are listed and that `--all` gives a reason for each skip. Fake trees of empty `.dll` files are enough; nothing runs wine.
- Installing: the DLLs land in the right directories for win64 and win32 prefixes, originals are backed up, the manifest records what was written, and the overrides are set. The existing fake wine already answers `reg`, so the override write is covered without wine installed.
- Swapping a version removes the old files before writing the new ones, and unsetting restores every original and leaves no `.wineo-orig` behind.
- A DLL with no original in the prefix is added on install and deleted on removal.
- Refusals: `dxvk` and `vkd3d` on a Proton environment, `vkd3d` with no `dxvk`, an uninstalled build name, and installing while the environment runs.
- Templates: applying one sets every value it names, explicit flags win, an unknown template name fails, a template naming an uninstalled runner or build fails before the prefix is created, and `prefix` or `targets` in a template is rejected.
- Ignored real tests, needing wine and a real DXVK build: install into a fresh prefix, confirm `wine winecfg` shows the overrides, run something that loads `d3d11.dll`, then unset and confirm the prefix is back to what it was.

## Open questions

For the maintainer, before this leaves draft:

1. **Built-in templates.** A useful `gaming` template wants `dxvk = "2.7"`, but a built-in cannot name a build the user may not have dropped in, so it would fail out of the box. Options: ship no built-ins and document an example; ship built-ins that only set things that always work, like `gamemode` and `[env]`; or add a `dxvk = "latest"` spelling meaning the newest installed build, which needs an ordering rule over arbitrary directory names.
2. **Reconciling on launch.** Above, a config edited by hand is reconciled at the next launch. That makes a launch write to the prefix, which nothing else in wineo does. The alternative is to refuse the launch and tell the user to run `wineo set` again, which is noisier but keeps launching read-only.
3. **Whether `wineo dxvk` should show which prefixes use a build.** It means reading every environment config to list one thing, which no other list verb does. Dropping it makes the verb a plain name list.

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
wineo status hades                             # dxvk and vkd3d lines
wineo set hades dxvk=2.8                       # swaps the build
wineo set hades --unset vkd3d                  # restores the originals
```

`wineo set hades vkd3d=2.14` with no `dxvk` set is refused. On a Proton environment both settings are refused, naming the bundled ones.
