#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../elara" && pwd)"
CARGO="${CARGO:-cargo}"

$CARGO clippy --manifest-path "$ROOT_DIR/Cargo.toml" --all-targets -- -D warnings

