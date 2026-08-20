#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN_DIR="${BUILD_DIR:-$REPO_ROOT/bin}"
HEADER_SRC="$REPO_ROOT/elara/include/elara.h"

# Prefer the invoking user's home when running under sudo.
user_home() {
  if [ "$(id -u)" -eq 0 ] && [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != "root" ]; then
    getent passwd "$SUDO_USER" | cut -d: -f6
    return
  fi
  echo "${HOME}"
}

# SYSTEM=1 or PREFIX=… → system-wide; else ~/.local/{lib,include}
if [ -n "${PREFIX:-}" ]; then
  LIB_DIR="${PREFIX%/}/lib"
  INCLUDE_DIR="${PREFIX%/}/include"
elif [ "${SYSTEM:-0}" = "1" ]; then
  LIB_DIR="/usr/local/lib"
  INCLUDE_DIR="/usr/local/include"
else
  UH="$(user_home)"
  LIB_DIR="${UH}/.local/lib"
  INCLUDE_DIR="${UH}/.local/include"
fi

for artifact in libelara.a libelara.so; do
  if [ ! -e "$BIN_DIR/$artifact" ]; then
    echo "error: missing library: $BIN_DIR/$artifact" >&2
    echo "hint:  run make build first" >&2
    exit 1
  fi
done
if [ ! -f "$HEADER_SRC" ]; then
  echo "error: missing header: $HEADER_SRC" >&2
  exit 1
fi

UH="$(user_home)"

can_write_dest_dir() {
  local dest_dir="$1"
  if [ -d "$dest_dir" ]; then
    [ -w "$dest_dir" ]
  else
    [ -w "$(dirname "$dest_dir")" ]
  fi
}

install_file() {
  local src="$1"
  local dest="$2"
  local mode="$3"
  local dest_dir
  dest_dir="$(dirname "$dest")"

  if [ "$(id -u)" -eq 0 ] && [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != "root" ]; then
    case "$dest_dir" in
      "$UH" | "$UH"/*)
        sudo -u "$SUDO_USER" install -D -m "$mode" "$src" "$dest"
        ;;
      *)
        install -D -m "$mode" "$src" "$dest"
        ;;
    esac
  elif can_write_dest_dir "$dest_dir"; then
    install -D -m "$mode" "$src" "$dest"
  else
    echo "Destination not writable; using sudo for install only..."
    sudo install -D -m "$mode" "$src" "$dest"
  fi
  echo "Installed: $dest"
}

install_file "$BIN_DIR/libelara.a" "$LIB_DIR/libelara.a" 644
install_file "$BIN_DIR/libelara.so" "$LIB_DIR/libelara.so" 755
install_file "$HEADER_SRC" "$INCLUDE_DIR/elara.h" 644
