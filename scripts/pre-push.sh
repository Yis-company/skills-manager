#!/bin/sh
set -eu
cd "$(git rev-parse --show-toplevel)"

bun run format:check
bun run lint
bun run typecheck
bun run test
cargo fmt --manifest-path src-tauri/Cargo.toml --check
