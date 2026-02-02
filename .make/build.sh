#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../elara" && pwd)"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUILD_DIR="${BUILD_DIR:-$REPO_ROOT/bin}"
BINARY_NAME="${BINARY_NAME:-elara}"
CARGO="${CARGO:-cargo}"

mkdir -p "$BUILD_DIR"
echo "Building $BINARY_NAME..."
$CARGO build --release --manifest-path "$ROOT_DIR/Cargo.toml"
install -m 755 "$ROOT_DIR/target/release/$BINARY_NAME" "$BUILD_DIR/$BINARY_NAME"
if [ -f "$ROOT_DIR/target/release/elarac" ]; then
    install -m 755 "$ROOT_DIR/target/release/elarac" "$BUILD_DIR/elarac"
fi
if [ -f "$ROOT_DIR/target/release/libelara.a" ]; then
    install -m 644 "$ROOT_DIR/target/release/libelara.a" "$BUILD_DIR/libelara.a"
fi
if [ -f "$ROOT_DIR/target/release/libelara.so" ]; then
    install -m 755 "$ROOT_DIR/target/release/libelara.so" "$BUILD_DIR/libelara.so"
fi
echo "Done: $BUILD_DIR/$BINARY_NAME"

