#!/usr/bin/env bash
set -euo pipefail

# Prefer the invoking user's home when running under sudo.
user_home() {
  if [ "$(id -u)" -eq 0 ] && [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != "root" ]; then
    getent passwd "$SUDO_USER" | cut -d: -f6
    return
  fi
  echo "${HOME}"
}

if [ -n "${PREFIX:-}" ]; then
  INSTALL_DIR="${PREFIX%/}/bin"
elif [ "${SYSTEM:-0}" = "1" ]; then
  INSTALL_DIR="/usr/local/bin"
else
  INSTALL_DIR="$(user_home)/.local/bin"
fi

targets=()
for name in elara elarac; do
  targets+=("${INSTALL_DIR}/${name}")
done

removed=0
for target in "${targets[@]}"; do
  if [ ! -e "$target" ]; then
    continue
  fi
  dir="$(dirname "$target")"
  if [ -w "$dir" ] 2>/dev/null && rm -f "$target" 2>/dev/null; then
    :
  else
    sudo rm -f "$target"
  fi
  echo "Removed: $target"
  removed=$((removed + 1))
done

if [ "$removed" -eq 0 ]; then
  echo "Not installed (checked: ${targets[*]})"
fi
