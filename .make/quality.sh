#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../elara" && pwd)"
CARGO="${CARGO:-cargo}"

$CARGO fmt --manifest-path "$ROOT_DIR/Cargo.toml" --all -- --check
$CARGO clippy --manifest-path "$ROOT_DIR/Cargo.toml" --all-targets -- -D warnings
$CARGO test --manifest-path "$ROOT_DIR/Cargo.toml"

