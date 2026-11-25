#!/usr/bin/env bash
# The build command for Cloudflare Pages with Git integration (web/README.md):
#
#   Build command:           bash web/cloudflare-build.sh
#   Build output directory:  web/dist
#
# The build image of Pages has Node.js but not Rust, so this installs the
# toolchain first. It was written from the documentation of Pages and has not
# been run there yet; a direct upload of a directory assembled on your own
# machine or in CI (web/README.md) needs none of it.
set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v cargo >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
    sh -s -- -y --profile minimal --default-toolchain stable
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi
rustup target add wasm32-unknown-unknown

# The command wasm-bindgen must be the version of the crate in Cargo.lock.
version=$(awk '/^name = "wasm-bindgen"$/ { getline; gsub(/[^0-9.]/, ""); print; exit }' Cargo.lock)
if [ "$(wasm-bindgen --version 2>/dev/null)" != "wasm-bindgen $version" ]; then
  cargo install wasm-bindgen-cli --version "$version" --locked
fi

cargo build --release --locked -p openkeito-wasm --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir crates/openkeito-wasm/pkg \
  target/wasm32-unknown-unknown/release/openkeito_wasm.wasm
node web/build.mjs
