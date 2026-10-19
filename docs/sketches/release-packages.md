# Release packages (deb, rpm, AppImage)

Install wineo on Debian, Ubuntu, Fedora, and other distros without
building it, shortly after 1.0. Backlog row in
[features.md](../features.md): Distribution, priority 2. The row keeps its
priority and status; this page keeps the detail.

From the maintainer (2026-10-09). deb and rpm first, through `cargo-deb`
and `cargo-generate-rpm` metadata in Cargo.toml, installing what the
PKGBUILD installs (both binaries, desktop file, icon, manpages,
completions, example configs).

AppImage after that, built on an old distro for a low glibc floor, without
bundling GPU libraries. It needs wineo to know where it's installed:
`current_exe()` re-execs (supervisor in `start.rs`, GUI worker,
`supervisor/steam.rs`, winetricks) would point into the AppImage's
temporary mount, so they should use `$APPIMAGE`, and desktop entries'
`Exec=wineo ...` assumes `wineo` is on `PATH`. One AppImage has one
entrypoint, so the CLI needs a dispatch. Flatpak is out: its sandbox
blocks spawning Wine and umu, scanning `/proc` for Steam's reaper, and
reading `~/.steam`. Needs a plan.
