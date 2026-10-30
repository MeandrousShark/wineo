# Maintainer: Michael Hannon <mphannon@pm.me>
# Packaging written with AI assistance and reviewed by the maintainer.
# SPDX-License-Identifier: 0BSD
# shellcheck disable=SC2034,SC2154
pkgname=wineo-git
pkgver=0
pkgrel=1
pkgdesc='Launcher for Wine and Proton programs, native games, Steam, and GOG'
arch=('x86_64')
url='https://github.com/MeandrousShark/wineo'
license=('GPL-3.0-or-later')
# wineo-gui loads everything after xz with dlopen, so no tool can find them.
# ca-certificates is the system certificate store that HTTPS requests read.
depends=(
  'glibc'
  'libgcc'
  'hicolor-icon-theme'
  'xz'
  'libx11'
  'libxcb'
  'libxkbcommon'
  'libxkbcommon-x11'
  'libxcursor'
  'libxi'
  'wayland'
  'libglvnd'
  'vulkan-icd-loader'
  'ca-certificates'
)
makedepends=('cargo' 'git')
optdepends=(
  'bash-completion: load Bash completions'
  'gamemode: enable GameMode for configured targets'
  'umu-launcher: run Proton environments'
  'wine: run Windows programs using Wine'
  'winetricks: install Windows components'
)
provides=("wineo=$pkgver")
conflicts=('wineo')
options=('!lto' '!debug')
source=('wineo::git+https://github.com/MeandrousShark/wineo.git#branch=main')
sha256sums=('SKIP')

pkgver() {
  cd "$srcdir/wineo" || return

  local base_version
  base_version=$(awk -F '"' '
    /^\[workspace\.package\]$/ { in_workspace = 1; next }
    /^\[/ { in_workspace = 0 }
    in_workspace && /^version = / { print $2; exit }
  ' Cargo.toml)
  local revision
  revision=$(git rev-list --count HEAD)
  local short_commit
  short_commit=$(git rev-parse --short=7 HEAD)

  printf '%s.r%s.g%s\n' "$base_version" "$revision" "$short_commit"
}

prepare() {
  cd "$srcdir/wineo" || return
  export RUSTUP_TOOLCHAIN=stable
  cargo fetch --locked --target "$(rustc --print host-tuple)"
}

build() {
  cd "$srcdir/wineo" || return
  export RUSTUP_TOOLCHAIN=stable
  export CARGO_TARGET_DIR=target
  cargo build --package wineo-cli --package wineo-gui --release --frozen
}

check() {
  cd "$srcdir/wineo" || return
  export RUSTUP_TOOLCHAIN=stable
  export CARGO_TARGET_DIR=target
  TERM=xterm-256color cargo test --workspace --frozen -- --test-threads=1
}

package() {
  cd "$srcdir/wineo" || return

  scripts/stage-install.sh --license-dir "/usr/share/licenses/$pkgname" \
    "$pkgdir" target/release
}
