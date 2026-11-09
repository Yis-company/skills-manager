#!/bin/sh
set -eu
cd "$(git rev-parse --show-toplevel)"

for tool in cargo rustc; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "Pre-push requires $tool on PATH." >&2
    exit 1
  fi
done
minimum_rust=$(awk -F '"' '/^rust-version = / { print $2; exit }' src-tauri/Cargo.toml)
rust_version=$(rustc --version)
if ! awk -v actual="$rust_version" -v minimum="$minimum_rust" 'BEGIN {
  split(actual, words, " ")
  if (words[2] !~ /^[0-9]+\.[0-9]+/ || minimum !~ /^[0-9]+\.[0-9]+/) exit 1
  split(words[2], installed, /\./)
  split(minimum, required, /\./)
  for (i = 1; i <= 3; i++) {
    if (installed[i] + 0 < required[i] + 0) exit 1
    if (installed[i] + 0 > required[i] + 0) exit 0
  }
}'; then
  echo "Pre-push requires Rust $minimum_rust or newer; found $rust_version." >&2
  exit 1
fi

bun run format:check
bun run lint
bun run typecheck
bun run test
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
