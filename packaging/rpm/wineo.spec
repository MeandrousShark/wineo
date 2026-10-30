# SPDX-License-Identifier: 0BSD
# Packaging written with AI assistance and reviewed by the maintainer.
#
# One spec for Fedora (COPR) and openSUSE (OBS). Only the runtime library
# names differ between them, and they sit under the suse_version test below.

# The release profile strips the binaries, so there is nothing for a
# debuginfo package to hold, and rpm would fail on the empty one.
%global debug_package %{nil}

Name:           wineo
Version:        1.1.0
Release:        1%{?dist}
Summary:        Launcher for Wine and Proton programs, native games, Steam, and GOG
# The crates compiled into the binaries, from `cargo deny list`, with each
# "OR" resolved to a choice. 0BSD is the patched hexf-parse in third_party/.
License:        GPL-3.0-or-later AND 0BSD AND Apache-2.0 AND BSD-2-Clause AND BSD-3-Clause AND ISC AND MIT AND Unicode-3.0 AND Zlib
URL:            https://github.com/MeandrousShark/wineo
Source0:        %{url}/releases/download/v%{version}/%{name}-%{version}-vendored.tar.xz
Packager:       Michael Hannon <mphannon@pm.me>
ExclusiveArch:  x86_64

# The vendored tarball carries every crate, so a build needs no network.
BuildRequires:  cargo >= 1.90
BuildRequires:  rust >= 1.90
BuildRequires:  gcc
BuildRequires:  pkgconfig
BuildRequires:  pkgconfig(liblzma)

# wineo-gui loads everything after liblzma with dlopen, so rpm can't find
# these libraries. The names are the README's Fedora list, and openSUSE's
# library packages carry the soname in their names. The last entry in each
# branch is the system certificate store that HTTPS requests read; openSUSE
# ships the roots in ca-certificates-mozilla.
Requires:       hicolor-icon-theme
%if 0%{?suse_version}
Requires:       libX11-6
Requires:       libX11-xcb1
Requires:       libxcb1
Requires:       libxkbcommon0
Requires:       libxkbcommon-x11-0
Requires:       libXcursor1
Requires:       libXi6
Requires:       libwayland-client0
Requires:       libwayland-egl1
Requires:       libglvnd
Requires:       libvulkan1
Requires:       ca-certificates-mozilla
%else
Requires:       libX11
Requires:       libX11-xcb
Requires:       libxcb
Requires:       libxkbcommon
Requires:       libxkbcommon-x11
Requires:       libXcursor
Requires:       libXi
Requires:       libwayland-client
Requires:       libwayland-egl
Requires:       libglvnd-egl
Requires:       vulkan-loader
Requires:       ca-certificates
%endif
Recommends:     wine
Recommends:     winetricks
# openSUSE's repositories don't have umu-launcher.
%if !0%{?suse_version}
Recommends:     umu-launcher
%endif

%description
A launcher and library for Linux, written in Rust, for Windows programs and
games run with Wine or Proton, native Linux games, your Steam library, and
the GOG games you own. Each one gets its own named environment. For Windows
software that's a prefix, a runner (Wine or Proton through umu), launch
targets, and settings, all in plain TOML you can read and edit. Use it from
the command line or the desktop app.

%prep
%autosetup -n %{name}-%{version}

%build
# Inside the tree so nothing touches the build user's home.
export CARGO_HOME="$PWD/.cargo-home"
cargo build --release --frozen %{?_smp_mflags} \
    --package wineo-cli --package wineo-gui

# Package builds don't run the test suite; a tagged release has passed CI.

%install
scripts/stage-install.sh --license-dir %{_datadir}/licenses/%{name} \
    %{buildroot} target/release

%files
%license %{_datadir}/licenses/%{name}/LICENSE
%license %{_datadir}/licenses/%{name}/THIRD_PARTY.md
%{_bindir}/wineo
%{_bindir}/wineo-gui
%{_mandir}/man1/wineo*.1*
%{_datadir}/applications/wineo-gui.desktop
%{_datadir}/icons/hicolor/scalable/apps/wineo-gui.svg
%{_datadir}/bash-completion/completions/wineo
%{_datadir}/zsh/site-functions/_wineo
%{_datadir}/fish/vendor_completions.d/wineo.fish
%{_datadir}/doc/%{name}/

%changelog
* Fri Oct 30 2026 Michael Hannon <mphannon@pm.me> - 1.1.0-1
- Package the 1.1.0 release from the vendored source tarball.
