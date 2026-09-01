#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../elara" && pwd)"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUILD_DIR="${BUILD_DIR:-$REPO_ROOT/bin}"
CARGO="${CARGO:-cargo}"

mkdir -p "$BUILD_DIR"
echo "Building elara (release)..."
$CARGO build --release --manifest-path "$ROOT_DIR/Cargo.toml"
# Export elara_* from the CLI so a dlopen'd library can call back into it.
# Flags after -- still apply when RUSTFLAGS is set; .cargo/config.toml does not.
$CARGO rustc --release --manifest-path "$ROOT_DIR/Cargo.toml" --bin elara -- \
  -C link-arg=-Wl,--export-dynamic

if [ -n "${CARGO_TARGET_DIR:-}" ]; then
  RELEASE_DIR="$CARGO_TARGET_DIR/release"
else
  RELEASE_DIR="$ROOT_DIR/target/release"
fi
missing=0
for artifact in elara elarac libelara.a libelara.so; do
  if [ ! -e "$RELEASE_DIR/$artifact" ]; then
    echo "error: expected release artifact missing: $RELEASE_DIR/$artifact" >&2
    missing=1
  fi
done
if [ "$missing" -ne 0 ]; then
  exit 1
fi

install -m 755 "$RELEASE_DIR/elara" "$BUILD_DIR/elara"
install -m 755 "$RELEASE_DIR/elarac" "$BUILD_DIR/elarac"
install -m 644 "$RELEASE_DIR/libelara.a" "$BUILD_DIR/libelara.a"
install -m 755 "$RELEASE_DIR/libelara.so" "$BUILD_DIR/libelara.so"

echo "Done:"
echo "  $BUILD_DIR/elara"
echo "  $BUILD_DIR/elarac"
echo "  $BUILD_DIR/libelara.a"
echo "  $BUILD_DIR/libelara.so"
