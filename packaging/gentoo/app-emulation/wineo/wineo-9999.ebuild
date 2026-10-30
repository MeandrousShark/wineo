# Packaging written with AI assistance and reviewed by the maintainer.
# SPDX-License-Identifier: 0BSD

EAPI=8

# Keep in step with scripts/rust-floor.sh.
RUST_MIN_VER="1.90.0"

inherit cargo desktop git-r3 xdg

DESCRIPTION="Launcher for Wine and Proton programs, native games, Steam, and GOG"
HOMEPAGE="https://github.com/MeandrousShark/wineo"
EGIT_REPO_URI="https://github.com/MeandrousShark/wineo.git"
EGIT_BRANCH="main"

LICENSE="GPL-3+"
SLOT="0"

# wineo-gui loads X11, xkbcommon, wayland, EGL, and Vulkan with dlopen, so no
# tool can find them. libX11 includes libX11-xcb, dev-libs/wayland includes
# libwayland-egl, and libxkbcommon[X] is libxkbcommon-x11.
DEPEND="app-arch/xz-utils"
RDEPEND="
	app-arch/xz-utils
	dev-libs/wayland
	media-libs/libglvnd
	media-libs/vulkan-loader
	x11-libs/libX11
	x11-libs/libXcursor
	x11-libs/libXi
	x11-libs/libxcb
	x11-libs/libxkbcommon[X]
	x11-themes/hicolor-icon-theme
"
BDEPEND="virtual/pkgconfig"

# Rust binaries ignore the C toolchain flags, and cargo already strips them.
QA_FLAGS_IGNORED="usr/bin/wineo usr/bin/wineo-gui"
QA_PRESTRIPPED="usr/bin/wineo usr/bin/wineo-gui"

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
}
