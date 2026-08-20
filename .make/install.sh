#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN_DIR="${BUILD_DIR:-$REPO_ROOT/bin}"

# Prefer the invoking user's home when running under sudo.
user_home() {
  if [ "$(id -u)" -eq 0 ] && [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != "root" ]; then
    getent passwd "$SUDO_USER" | cut -d: -f6
    return
  fi
  echo "${HOME}"
}

# SYSTEM=1 or PREFIX=… → system-wide; else ~/.local/bin
if [ -n "${PREFIX:-}" ]; then
  INSTALL_DIR="${PREFIX%/}/bin"
elif [ "${SYSTEM:-0}" = "1" ]; then
  INSTALL_DIR="/usr/local/bin"
else
  INSTALL_DIR="$(user_home)/.local/bin"
fi

for name in elara elarac; do
  if [ ! -x "$BIN_DIR/$name" ]; then
    echo "error: missing binary: $BIN_DIR/$name" >&2
    echo "hint:  run make build first" >&2
    exit 1
  fi
done

UH="$(user_home)"

install_one() {
  local src="$1"
  local dest="$2"
  if [ "$(id -u)" -eq 0 ] && [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != "root" ]; then
    case "$INSTALL_DIR" in
      "$UH" | "$UH"/*)
        sudo -u "$SUDO_USER" install -D -m 755 "$src" "$dest"
        ;;
      *)
        install -D -m 755 "$src" "$dest"
        ;;
    esac
  elif [ -w "$(dirname "$INSTALL_DIR")" ] 2>/dev/null || [ -w "$INSTALL_DIR" ] 2>/dev/null; then
    install -D -m 755 "$src" "$dest"
  else
    echo "Destination not writable; using sudo for install only..."
    sudo install -D -m 755 "$src" "$dest"
  fi
  echo "Installed: $dest"
}

for name in elara elarac; do
  install_one "$BIN_DIR/$name" "$INSTALL_DIR/$name"
done
