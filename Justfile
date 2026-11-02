name := 'cosmic-ext-applet-active-window'
appid := 'com.github.sammyfischer.ActiveWindowApplet'

rootdir := ''
prefix := env('PREFIX', home_directory() / '.local')

appdata := appid + '.metainfo.xml'
desktop := appid + '.desktop'

# Installation paths
base-dir := absolute_path(clean(rootdir / prefix))
cargo-target-dir := env('CARGO_TARGET_DIR', 'target')
appdata-dst := base-dir / 'share' / 'appdata' / appdata
bin-dst := base-dir / 'bin' / name
desktop-dst := base-dir / 'share' / 'applications' / desktop
icon-dst := base-dir / 'share' / 'icons' / 'hicolor' / '512x512' / 'apps' / appid + '.png'

# Default recipe which runs `just build-release`
default: build

# Runs `cargo clean`
clean:
  cargo clean

# Removes vendored dependencies
clean-vendor:
  rm -rf .cargo vendor vendor.tar

# `cargo clean` and removes vendored dependencies
clean-dist: clean clean-vendor

# Compiles with release profile
build *args:
  cargo build --release {{ args }}

# Compiles release profile with vendored dependencies
build-vendored *args: vendor-extract (build '--frozen --offline' args)

# Formats files with dprint
fmt:
  dprint fmt --diff

# Runs a clippy check
lint *args:
  cargo clippy --all-features {{ args }} -- -W clippy::pedantic

# Runs unit tests
test:
  cargo test

alias ck := check
# Checks for compliance (formats and lints)
check: fmt lint test

# Runs a clippy check with JSON message format
check-json: (lint '--message-format=json')

# Run the application for testing purposes
run *args:
  env RUST_BACKTRACE=full cargo run --release {{ args }}

# Installs files
install:
  install -Dm0755 {{ cargo-target-dir / 'release' / name }} {{ bin-dst }}
  install -Dm0644 {{ 'target' / 'xdgen' / 'app.desktop' }} {{ desktop-dst }}
  install -Dm0644 {{ 'target' / 'xdgen' / 'app.metainfo.xml' }} {{ appdata-dst }}
  install -Dm0644 resources/512x512.png {{ icon-dst }}

# Install for all users
install-sys:
  sudo "$(which just)" prefix=/usr install

# Uninstalls installed files
uninstall:
  rm {{ bin-dst }} {{ desktop-dst }} {{ icon-dst }}

# Uninstalls a system-wide installation
uninstall-sys:
  sudo "$(which just)" prefix=/usr uninstall

view-logs:
  journalctl -f SYSLOG_IDENTIFIER={{ name }}

# Vendor dependencies locally
vendor:
  mkdir -p .cargo
  cargo vendor --sync Cargo.toml | head -n -1 > .cargo/config.toml
  echo 'directory = "vendor"' >> .cargo/config.toml
  echo >> .cargo/config.toml
  rm -rf .cargo vendor

# Extracts vendored dependencies
vendor-extract:
  rm -rf vendor
  tar pxf vendor.tar

# Builds a release package
package: build
  just rootdir=debian/{{ name }} prefix=/usr install

# Bump cargo version, create git commit, and create tag
tag version:
  find -type f -name Cargo.toml -exec sed -i '0,/^version/s/^version.*/version = "{{ version }}"/' '{}' \; -exec git add '{}' \;
  cargo check
  cargo clean
  git add Cargo.lock
  git commit -m 'release: {{ version }}'
  git tag -a {{ version }} -m ''

# Initialize the project by installing the precommit hook
init:
  install -Dm0755 pre-commit.sh .git/hooks/pre-commit
