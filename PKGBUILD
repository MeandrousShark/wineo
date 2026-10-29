# shellcheck disable=SC2034,SC2154
pkgname=wineo-git
pkgver=0
pkgrel=1
pkgdesc='Wine environment manager with live shell completions'
arch=('x86_64')
url='https://github.com/MeandrousShark/wineo'
license=('GPL-3.0-or-later')
depends=('glibc' 'libgcc' 'xz')
makedepends=('cargo' 'git')
optdepends=(
  'bash-completion: load Bash completions'
  'gamemode: enable GameMode for configured targets'
  'umu-launcher: run Proton environments'
  'wine: run Windows programs using Wine'
  'winetricks: install Windows components'
)
provides=('wineo')
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
  cargo fetch --locked
}

build() {
  cd "$srcdir/wineo" || return
  cargo build --package wineo-cli --package wineo-gui --release --frozen
}

check() {
  cd "$srcdir/wineo" || return
  TERM=xterm-256color cargo test --workspace --frozen -- --test-threads=1
}

package() {
  cd "$srcdir/wineo" || return

  install -Dm755 target/release/wineo "$pkgdir/usr/bin/wineo"
  install -Dm755 target/release/wineo-gui "$pkgdir/usr/bin/wineo-gui"
  install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
  install -Dm644 crates/wineo-gui/assets/wineo-gui.desktop \
    "$pkgdir/usr/share/applications/wineo-gui.desktop"
  install -Dm644 crates/wineo-gui/assets/wineo-gui.svg \
    "$pkgdir/usr/share/icons/hicolor/scalable/apps/wineo-gui.svg"
  install -Dm644 crates/wineo-gui/THIRD_PARTY.md \
    "$pkgdir/usr/share/licenses/$pkgname/THIRD_PARTY.md"
  install -Dm644 docs/environment.toml.example \
    "$pkgdir/usr/share/doc/wineo/environment.toml.example"
  install -Dm644 docs/native-environment.toml.example \
    "$pkgdir/usr/share/doc/wineo/native-environment.toml.example"
  install -Dm644 docs/proxied-environment.toml.example \
    "$pkgdir/usr/share/doc/wineo/proxied-environment.toml.example"

  "$pkgdir/usr/bin/wineo" completions bash > "$srcdir/wineo.bash"
  "$pkgdir/usr/bin/wineo" completions zsh > "$srcdir/wineo.zsh"
  "$pkgdir/usr/bin/wineo" completions fish > "$srcdir/wineo.fish"

  install -Dm644 "$srcdir/wineo.bash" \
    "$pkgdir/usr/share/bash-completion/completions/wineo"
  install -Dm644 "$srcdir/wineo.zsh" \
    "$pkgdir/usr/share/zsh/site-functions/_wineo"
  install -Dm644 "$srcdir/wineo.fish" \
    "$pkgdir/usr/share/fish/vendor_completions.d/wineo.fish"

  "$pkgdir/usr/bin/wineo" manpages "$pkgdir/usr/share/man/man1"
}
