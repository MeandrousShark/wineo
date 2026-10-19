# macOS port

Run wineo and wineo-gui on Apple Silicon Macs with Wine, CrossOver-style
Wine builds, and Game Porting Toolkit. Backlog row in
[features.md](../features.md): Compatibility, priority 4. The row keeps
its priority and status; this page keeps the detail.

Requested by the maintainer (2026-09-30).

Spike on an Apple Silicon Mac with macOS 27.2 (2026-10-17, on a throwaway
branch, since deleted): stubbing about 20 Linux-only spots in core (pidfd,
eventfd, prlimit, `ioctl_ficlone`, `syncfs`, `statx`, and Darwin's
narrower `st_mode` and `st_dev` types) and gating the GUI's Wayland
`application_id` makes both binaries build, and `wineo-gui` ran for 15
seconds without a panic. iced, wgpu, and every other dependency build
unchanged; tests add about 6 more spots (the pidfd Steam harness,
`mknodat`).

With Heroic's Wine Staging 11.4 on `PATH`, `wineo new` made a prefix in 20
seconds and `start` and `status` worked, but `stop` timed out: process
identity reads `/proc` (`boot_id`, `/proc/<pid>/stat`, and group scans in
`process.rs` and `steam/process.rs`) and holds pidfds, so ownership is
never confirmed and wineo refuses to signal. It fails safe, but that layer
is the core of the port: `kern.bootsessionuuid` for the boot, libproc or
`sysctl KERN_PROC` for start times and group members, and rustix's safe
kqueue `EVFILT_PROC` exit events for waits. Darwin has no pidfd, so
signalling by pid reopens the reuse race pidfds close; that is a design
question.

rustix maps `RenameFlags::EXCHANGE` and `NOREPLACE` to `RENAME_SWAP` and
`RENAME_EXCL` (snapshot swaps should work on APFS, untested) and has
`fclonefileat` and `fcntl_fullfsync`. `clonefile` needs a destination that
doesn't exist yet, unlike `FICLONE` into an open file, so the copy
engine's order changes; snapshots require reflinks, so they wait on it.
Trash mount identity needs `statfs` or `getmntinfo`. The Secret Service
keyring builds (zbus is pure Rust) but has no session bus to reach on
macOS (not exercised: with no saved login, `import gog --list` stops
before asking it), and callers name `SecretServiceKeyring` directly, so
Keychain needs a factory behind the existing `Keyring` trait.

Also Linux-only: `.desktop` entries and hicolor icons (CrossOver writes a
per-program `.app` in `~/Applications/CrossOver/<bottle>/`; a script
`.app` running `wineo start` would match), the GUI's ashpd portals (file
picker, dark mode), `xdg-open`, Steam and Heroic default paths (both under
`~/Library/Application Support`; `import heroic --config-dir` already
reads the Mac one, but not Heroic's `toolkit` runner type), external
runner discovery (Heroic's Mac Wine bundles under `tools/wine` and
`tools/game-porting-toolkit` aren't found, and GPTK ships `wine64` without
`wine`), GOG's `osx` platform, and the downloads gate (only Linux x86_64
Proton, DXVK, and vkd3d). umu/Proton, gamemode, gamescope, MangoHud, and
DXVK/vkd3d don't apply; a Mac needs D3DMetal/DXMT runners instead.

Engines: every Wine on the spike Mac (Gcenx Wine Staging, its DXMT
variant, GPTK 1.1 on Wine 7.7) is x86_64 under Rosetta 2. Apple's
developer notice (2026-09-01) calls macOS 27 the "final release to support
Rosetta" and says Intel-only apps stop running after it, keeping Rosetta
only for "older, unmaintained gaming titles that rely on Intel-based
frameworks". It doesn't mention Wine or translation layers, so don't count
on the exception; x86 Wine likely stops with macOS 28 (expected fall 2027)
and lives on only for users who stay on 27. The macOS 26.4 Rosetta warning
never appeared in the spike, likely because Wine started from a command
line isn't an app bundle. The engine-independent work (process layer,
Keychain, paths, `clonefile`, launchers, GUI portals) carries over to any
engine; only runner discovery and launch depend on one.

ARM64 status (2026-10-17): CodeWeavers previewed an ARM64 CrossOver on
2026-07-31 (ARM64 Wine with FEX in place of Rosetta, macOS 26.5 or later),
but the preview needs a paid license and the maintainer's trial is used
up, so it can't be tested here. CrossOver 27 drops Intel Macs and 32-bit
bottles. CodeWeavers' source page only has 26.3.0, with nothing for the
preview, ARM64, or FEX. A third-party tracker reports the Wine side as
ARM64EC with sources due with CrossOver 27 in early 2027; unconfirmed by
CodeWeavers.

Upstream: CodeWeavers' Brendan Shanks wrote on wine-devel (2026-08-07)
that macOS 26.5 has every piece ARM64 Wine needs and that merge requests
would follow, but as of 2026-10-10 only preparatory ones had landed.
Upstream FEX has no macOS support; a community `FEX-darwin` port
(2026-10-07) and an ARM64 Wine, FEX, and DXMT bundle called `neutron`
exist, both unverified. DXMT builds ARM64EC in CI.

Signing: Gatekeeper ("Open Anyway" in Privacy & Security) only checks
quarantined downloads. Heroic's Wine is ad-hoc signed, has no
entitlements, and has no quarantine flag (Heroic downloaded it), so it
runs even though `spctl` rejects it. ARM64 Wine with FEX needs a
restricted entitlement (named in that post as
`com.apple.developer.cross-architecture-support`; not in Apple's public
docs), which AMFI enforces at launch against an Apple provisioning
profile. An ad-hoc binary claiming it is killed, and only disabling SIP
and AMFI gets around that. The post says a free Apple account can get an
`-unmanaged` variant, but profiles are tied to registered Macs, so an
open-source build can't be signed once for everyone: each user signs for
their own Mac (wineo could guide that, like `import gog --login`), or a
paid account registers a capped number of devices. How long a free profile
lasts on macOS is unverified. CrossOver ships it signed.

From the maintainer (2026-10-17): once an ARM64 Wine exists, document
disabling SIP and AMFI as an unrecommended option for users who don't want
to connect an Apple account. It means booting into Recovery and lowers the
whole system's protection, so the docs should say so plainly; wineo could
check `csrutil status` to explain why an ARM64 runner was killed.

Using CrossOver as a runner: Heroic finds `/Applications/CrossOver.app` or
`CrossOver Preview.app` (falling back to `mdfind` on its bundle id), runs
`Contents/SharedSupport/CrossOver/bin/wine`, and uses a CrossOver bottle
(`~/Library/Application Support/CrossOver/Bottles/<name>/`, with a
`cxbottle.conf`) instead of a prefix. wineo would need a runner kind that
maps an environment to a bottle, which is a design question.

Matching CrossOver (discussed 2026-10-01): its Wine changes are LGPL and
published, and community macOS builds use them, so wineo should reuse
existing builds like Heroic does instead of building Wine. D3DMetal comes
from Apple's Game Porting Toolkit; point at the user's own install rather
than bundling it until Apple's redistribution terms are checked. A
per-game graphics backend choice (D3DMetal, DXMT, or DXVK over MoltenVK)
fits templates and per-target settings. The real gap is per-game fixes:
Linux gets protonfixes through umu, and macOS has no open equivalent,
which is what CrossOver's recipes and testing provide. Whisky's 2025
discontinuation shows the maintenance load.

Parked by the maintainer (2026-10-17): no plan or code until there's a
route to an ARM64 Wine that is free to use and runs signed. Check again
when CrossOver 27's sources publish or upstream Wine merges the macOS
ARM64 work, whichever comes first. Then it needs a plan.
