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

targets=(
  "$LIB_DIR/libelara.a"
  "$LIB_DIR/libelara.so"
  "$INCLUDE_DIR/elara.h"
)

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
