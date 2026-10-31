# Packaging written with AI assistance and reviewed by the maintainer.
# SPDX-License-Identifier: 0BSD

EAPI=8

# The revision of the release ebuild derived from this file. Bump it for a
# packaging fix under the same wineo version, and publish-packages.sh writes
# wineo-VERSION-rN.ebuild. Back to 0 for a new version.
# Release revision: 0

# Keep in step with scripts/rust-floor.sh.
RUST_MIN_VER="1.90.0"

inherit cargo git-r3 xdg

DESCRIPTION="Launcher for Wine and Proton programs, native games, Steam, and GOG"
HOMEPAGE="https://github.com/MeandrousShark/wineo"
EGIT_REPO_URI="https://github.com/MeandrousShark/wineo.git"
EGIT_BRANCH="main"

LICENSE="GPL-3+"
# pycargoebuild adds only crates.io licenses to the release ebuild. hexf-parse
# is built from third_party/, and wineo-gui embeds the Noto Sans fonts.
LICENSE+=" 0BSD OFL-1.1"
SLOT="0"

# wineo-gui loads X11, xkbcommon, wayland, EGL, and Vulkan with dlopen, so no
# tool can find them. libX11 includes libX11-xcb, dev-libs/wayland includes
# libwayland-egl, and libxkbcommon[X] is libxkbcommon-x11. vulkan-loader's USE
# flags turn on its X11 and Wayland surfaces, without which it can't draw.
# ca-certificates is the system certificate store that HTTPS requests read.
DEPEND="app-arch/xz-utils"
RDEPEND="
	app-arch/xz-utils
	app-misc/ca-certificates
	dev-libs/wayland
	media-libs/libglvnd
	media-libs/vulkan-loader[X,wayland]
	x11-libs/libX11
	x11-libs/libXcursor
	x11-libs/libXi
	x11-libs/libxcb
	x11-libs/libxkbcommon[X]
	x11-themes/hicolor-icon-theme
"
BDEPEND="virtual/pkgconfig"

# CI runs the suite, which needs a terminal and one test thread.
RESTRICT="test"

# Rust binaries ignore the C toolchain flags.
QA_FLAGS_IGNORED="usr/bin/wineo usr/bin/wineo-gui"

src_unpack() {
	git-r3_src_unpack
	cargo_live_src_unpack
}

src_compile() {
	cargo_src_compile --package wineo-cli --package wineo-gui
}

src_install() {
	# One script lays out every package format's files, so they can't drift.
	scripts/stage-install.sh --license-dir "/usr/share/doc/${PF}" \
		"${D}" "$(cargo_target_dir)" || die

	# The script puts the example configs in /usr/share/doc/wineo, but Gentoo
	# keeps a package's docs in a directory with its version.
	mv "${ED}"/usr/share/doc/wineo/* "${ED}/usr/share/doc/${PF}/" || die
	rmdir "${ED}/usr/share/doc/wineo" || die
}
